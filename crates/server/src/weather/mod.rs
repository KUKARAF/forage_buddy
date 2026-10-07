//! Historical daily weather lookup, used to give foraging sightings local
//! weather context (recent rain/temperature affects fruiting).
//!
//! Backed by the [Open-Meteo Historical Weather (Archive)
//! API](https://open-meteo.com/en/docs/historical-weather-api) — free, no
//! API key required, so unlike `llm/mod.rs` this module has no API key /
//! provider-switch plumbing and no `Inner`/`AppState`-held client: every
//! call is a single, self-contained GET, so callers just pass in a
//! `&reqwest::Client` (e.g. `AppState`'s or a short-lived one) rather than
//! this module owning one.
//!
//! Exposes [`fetch_last_14_days`], which fetches the 14 days ending
//! yesterday (today is excluded — the Archive API only has data for
//! completed days) for a given lat/lon and returns one [`DailyWeather`] per
//! day, in ascending date order.

use std::time::Duration as StdDuration;

use anyhow::anyhow;
use axum::extract::{Query, State};
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use time::{Date, Duration as DateDuration, Month, OffsetDateTime};

use crate::auth::session::RequireAuth;
use crate::error::{AppError, AppResult};
use crate::state::AppState;

const ARCHIVE_URL: &str = "https://archive-api.open-meteo.com/v1/archive";
/// Comma-joined `daily=` field list, in the exact order the fields are read
/// back out of the response in [`DailyRaw`] / [`zip_daily`].
const DAILY_FIELDS: &str = "temperature_2m_max,temperature_2m_min,temperature_2m_mean,\
precipitation_sum,rain_sum,wind_speed_10m_max,wind_speed_10m_mean,relative_humidity_2m_mean";
/// 10s is plenty for a single small JSON GET against a free public API; this
/// is a per-request override (like `llm::EMBED_REQUEST_TIMEOUT`) so callers
/// can pass in a client with a different default timeout.
const REQUEST_TIMEOUT: StdDuration = StdDuration::from_secs(10);

/// One day of historical weather for a single location, as returned by
/// [`fetch_last_14_days`].
#[derive(Debug, Clone, PartialEq)]
pub struct DailyWeather {
    pub date: Date,
    pub temp_max_c: f64,
    pub temp_min_c: f64,
    pub temp_mean_c: f64,
    pub precipitation_mm: f64,
    pub rain_mm: f64,
    pub wind_speed_max_kmh: f64,
    pub wind_speed_mean_kmh: f64,
    pub humidity_mean_pct: f64,
}

/// Fetch the 14 days of daily historical weather ending yesterday (UTC) for
/// `lat`/`lon` from the Open-Meteo Archive API. Returns a clear [`AppError`]
/// on a non-2xx response, a malformed body, or mismatched per-field array
/// lengths in the response (the thing most likely to silently misalign
/// dates with values if the provider ever changes its field set).
pub async fn fetch_last_14_days(
    client: &reqwest::Client,
    lat: f64,
    lon: f64,
) -> AppResult<Vec<DailyWeather>> {
    let today = OffsetDateTime::now_utc().date();
    // The archive only has data for completed days, so the window is
    // [today - 14, today - 1], not [today - 13, today].
    let end_date = today - DateDuration::days(1);
    let start_date = today - DateDuration::days(14);

    let url = format!(
        "{ARCHIVE_URL}?latitude={lat}&longitude={lon}&start_date={}&end_date={}&daily={DAILY_FIELDS}&timezone=auto",
        format_date(start_date),
        format_date(end_date),
    );

    let resp = client
        .get(&url)
        .timeout(REQUEST_TIMEOUT)
        .send()
        .await
        .map_err(|e| AppError::Internal(anyhow!("weather request failed: {e}")))?;

    let status = resp.status();
    let raw = resp
        .text()
        .await
        .map_err(|e| AppError::Internal(anyhow!("reading weather response failed: {e}")))?;

    if !status.is_success() {
        return Err(AppError::Internal(anyhow!(
            "weather provider returned {status}: {raw}"
        )));
    }

    let parsed: ArchiveResponse = serde_json::from_str(&raw)
        .map_err(|e| AppError::Internal(anyhow!("unexpected weather response: {e}")))?;

    zip_daily(parsed.daily)
}

/// Format a [`Date`] as `YYYY-MM-DD` for the Open-Meteo query string.
/// `Month` is a `#[repr(u8)]` C-like enum with `January == 1` through
/// `December == 12`, matching the calendar month number directly.
fn format_date(date: Date) -> String {
    format!(
        "{:04}-{:02}-{:02}",
        date.year(),
        date.month() as u8,
        date.day()
    )
}

/// Parse a Open-Meteo `daily.time` entry (`YYYY-MM-DD`) into a [`Date`].
fn parse_date(s: &str) -> AppResult<Date> {
    let mut parts = s.split('-');
    let parsed = (|| {
        let year = parts.next()?.parse::<i32>().ok()?;
        let month = parts.next()?.parse::<u8>().ok()?;
        let day = parts.next()?.parse::<u8>().ok()?;
        Some((year, month, day))
    })();
    let (year, month, day) = parsed.ok_or_else(|| {
        AppError::Internal(anyhow!(
            "weather provider returned an unparseable date: {s}"
        ))
    })?;

    let month = Month::try_from(month).map_err(|e| {
        AppError::Internal(anyhow!(
            "weather provider returned an invalid month in {s}: {e}"
        ))
    })?;
    Date::from_calendar_date(year, month, day).map_err(|e| {
        AppError::Internal(anyhow!(
            "weather provider returned an invalid date {s}: {e}"
        ))
    })
}

/// Zip [`DailyRaw`]'s parallel per-field arrays into one [`DailyWeather`] per
/// index, erroring out if any field's array length doesn't match `time`'s —
/// silently zipping mismatched lengths would misalign dates and values.
fn zip_daily(raw: DailyRaw) -> AppResult<Vec<DailyWeather>> {
    let n = raw.time.len();
    let lens = [
        raw.temperature_2m_max.len(),
        raw.temperature_2m_min.len(),
        raw.temperature_2m_mean.len(),
        raw.precipitation_sum.len(),
        raw.rain_sum.len(),
        raw.wind_speed_10m_max.len(),
        raw.wind_speed_10m_mean.len(),
        raw.relative_humidity_2m_mean.len(),
    ];
    if lens.iter().any(|&len| len != n) {
        return Err(AppError::Internal(anyhow!(
            "weather provider returned mismatched array lengths (time has {n}, fields have {lens:?})"
        )));
    }

    // `.get(i)` (checked) rather than `v[i]` (panics on OOB) — the length
    // check above means these never actually miss, but the workspace denies
    // `clippy::indexing_slicing` outright, so every access goes through a
    // fallible path regardless.
    let field = |v: &[f64], i: usize| -> AppResult<f64> {
        v.get(i).copied().ok_or_else(|| {
            AppError::Internal(anyhow!("weather provider response index {i} out of bounds"))
        })
    };

    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let date_str = raw.time.get(i).ok_or_else(|| {
            AppError::Internal(anyhow!("weather provider response index {i} out of bounds"))
        })?;
        out.push(DailyWeather {
            date: parse_date(date_str)?,
            temp_max_c: field(&raw.temperature_2m_max, i)?,
            temp_min_c: field(&raw.temperature_2m_min, i)?,
            temp_mean_c: field(&raw.temperature_2m_mean, i)?,
            precipitation_mm: field(&raw.precipitation_sum, i)?,
            rain_mm: field(&raw.rain_sum, i)?,
            wind_speed_max_kmh: field(&raw.wind_speed_10m_max, i)?,
            wind_speed_mean_kmh: field(&raw.wind_speed_10m_mean, i)?,
            humidity_mean_pct: field(&raw.relative_humidity_2m_mean, i)?,
        });
    }
    Ok(out)
}

// --- HTTP -------------------------------------------------------------------

/// JSON-serializable mirror of [`DailyWeather`] for the HTTP response —
/// `time::Date` has no `Serialize` impl in this workspace, so `date` goes
/// out as the same `YYYY-MM-DD` string the provider used.
#[derive(Debug, Serialize)]
struct DailyWeatherDto {
    date: String,
    temp_max_c: f64,
    temp_min_c: f64,
    temp_mean_c: f64,
    precipitation_mm: f64,
    rain_mm: f64,
    wind_speed_max_kmh: f64,
    wind_speed_mean_kmh: f64,
    humidity_mean_pct: f64,
}

impl From<DailyWeather> for DailyWeatherDto {
    fn from(d: DailyWeather) -> Self {
        DailyWeatherDto {
            date: format_date(d.date),
            temp_max_c: d.temp_max_c,
            temp_min_c: d.temp_min_c,
            temp_mean_c: d.temp_mean_c,
            precipitation_mm: d.precipitation_mm,
            rain_mm: d.rain_mm,
            wind_speed_max_kmh: d.wind_speed_max_kmh,
            wind_speed_mean_kmh: d.wind_speed_mean_kmh,
            humidity_mean_pct: d.humidity_mean_pct,
        }
    }
}

#[derive(Debug, Deserialize)]
struct WeatherQuery {
    lat: f64,
    lon: f64,
}

/// `GET /api/weather?lat=&lon=` — the last 14 days of daily historical
/// weather for a location. Groundwork for future season/weather-aware
/// foraging suggestions; not surfaced in the UI yet.
async fn get_weather(
    State(state): State<AppState>,
    RequireAuth(_user_id): RequireAuth,
    Query(q): Query<WeatherQuery>,
) -> AppResult<Json<Vec<DailyWeatherDto>>> {
    let days = fetch_last_14_days(&state.http_client, q.lat, q.lon).await?;
    Ok(Json(days.into_iter().map(DailyWeatherDto::from).collect()))
}

pub fn router() -> Router<AppState> {
    Router::new().route("/api/weather", get(get_weather))
}

// --- Open-Meteo Archive API response shape ---

#[derive(Debug, Deserialize)]
struct ArchiveResponse {
    daily: DailyRaw,
}

/// Raw `daily` object: one parallel array per field, index-aligned with
/// `time`. Field names/order here must match [`DAILY_FIELDS`].
#[derive(Debug, Deserialize)]
struct DailyRaw {
    time: Vec<String>,
    temperature_2m_max: Vec<f64>,
    temperature_2m_min: Vec<f64>,
    temperature_2m_mean: Vec<f64>,
    precipitation_sum: Vec<f64>,
    rain_sum: Vec<f64>,
    wind_speed_10m_max: Vec<f64>,
    wind_speed_10m_mean: Vec<f64>,
    relative_humidity_2m_mean: Vec<f64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A realistic 3-day sample response, matching the real field names from
    /// the Archive API docs. This is the deserialize-then-zip path most
    /// likely to have an off-by-one/misalignment bug, so it's tested
    /// directly rather than only via the live API.
    const SAMPLE_RESPONSE: &str = r#"{
        "latitude": 52.52,
        "longitude": 13.41,
        "daily": {
            "time": ["2026-09-23", "2026-09-24", "2026-09-25"],
            "temperature_2m_max": [18.1, 19.4, 15.2],
            "temperature_2m_min": [9.3, 10.1, 8.0],
            "temperature_2m_mean": [13.7, 14.6, 11.5],
            "precipitation_sum": [0.0, 3.2, 1.1],
            "rain_sum": [0.0, 3.2, 1.1],
            "wind_speed_10m_max": [14.5, 22.3, 18.0],
            "wind_speed_10m_mean": [7.2, 11.9, 9.4],
            "relative_humidity_2m_mean": [72.0, 85.5, 80.2]
        }
    }"#;

    #[test]
    fn deserializes_and_zips_sample_response_in_date_order() {
        let parsed: ArchiveResponse = serde_json::from_str(SAMPLE_RESPONSE).unwrap();
        let days = zip_daily(parsed.daily).unwrap();

        assert_eq!(days.len(), 3);

        assert_eq!(
            days[0].date,
            Date::from_calendar_date(2026, Month::September, 23).unwrap()
        );
        assert_eq!(days[0].temp_max_c, 18.1);
        assert_eq!(days[0].temp_min_c, 9.3);
        assert_eq!(days[0].temp_mean_c, 13.7);
        assert_eq!(days[0].precipitation_mm, 0.0);
        assert_eq!(days[0].rain_mm, 0.0);
        assert_eq!(days[0].wind_speed_max_kmh, 14.5);
        assert_eq!(days[0].wind_speed_mean_kmh, 7.2);
        assert_eq!(days[0].humidity_mean_pct, 72.0);

        assert_eq!(
            days[1].date,
            Date::from_calendar_date(2026, Month::September, 24).unwrap()
        );
        assert_eq!(days[1].temp_max_c, 19.4);
        assert_eq!(days[1].rain_mm, 3.2);
        assert_eq!(days[1].humidity_mean_pct, 85.5);

        assert_eq!(
            days[2].date,
            Date::from_calendar_date(2026, Month::September, 25).unwrap()
        );
        assert_eq!(days[2].temp_min_c, 8.0);
        assert_eq!(days[2].wind_speed_max_kmh, 18.0);
        assert_eq!(days[2].humidity_mean_pct, 80.2);
    }

    #[test]
    fn zip_daily_rejects_mismatched_array_lengths() {
        let mut parsed: ArchiveResponse = serde_json::from_str(SAMPLE_RESPONSE).unwrap();
        // Simulate a provider bug/partial response: one field short.
        parsed.daily.rain_sum.pop();
        let err = zip_daily(parsed.daily).unwrap_err();
        assert!(matches!(err, AppError::Internal(_)));
    }

    #[test]
    fn parse_date_round_trips_format_date() {
        let date = Date::from_calendar_date(2026, Month::October, 7).unwrap();
        let formatted = format_date(date);
        assert_eq!(formatted, "2026-10-07");
        assert_eq!(parse_date(&formatted).unwrap(), date);
    }

    #[test]
    fn parse_date_rejects_garbage() {
        assert!(parse_date("not-a-date").is_err());
        assert!(parse_date("2026-13-01").is_err());
        assert!(parse_date("2026-02-30").is_err());
    }
}
