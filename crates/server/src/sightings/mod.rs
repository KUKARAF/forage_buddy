//! Sightings: a user's foraging observations, plus the aggregate detail
//! view the frontend's detail screen renders from a single request.
//!
//! Owns migration `0002_sightings.sql` (`sightings`). Also reads (but does
//! not own) `photos` (the `photos` module, same agent), `triage_results`
//! (the `triage` module) and `deepdive_results` (the `deepdive` module) —
//! those two tables' schemas are fixed by `docs/ARCHITECTURE.md` and are
//! queried directly here to build the detail aggregate without an extra
//! cross-module call.
//!
//! Routes (all require auth, scoped to the caller's own `user_id` — a
//! sighting owned by someone else 404s rather than 403s, so existence is
//! never leaked):
//!   - `POST  /api/sightings`      create a sighting (status `open`)
//!   - `GET   /api/sightings`      list the caller's sightings, newest first
//!   - `GET   /api/sightings/{id}` sighting + photos + latest triage + latest deepdive
//!   - `PATCH /api/sightings/{id}` update `notes`/`status`

use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use axum_extra::extract::WithRejection;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;
use uuid::Uuid;

use forage_buddy_core::domain::SightingStatus;

use crate::auth::session::RequireAuth;
use crate::error::{AppError, AppResult};
use crate::photos::PhotoView;
use crate::state::AppState;

// --- DTOs ---------------------------------------------------------------------

/// A sighting as stored and returned to the client.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Sighting {
    pub id: String,
    pub user_id: String,
    pub status: String,
    pub lat: Option<f64>,
    pub lon: Option<f64>,
    pub location_accuracy_m: Option<f64>,
    pub place_label: Option<String>,
    pub observed_at: String,
    pub notes: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// A cheap per-sighting summary for the list screen: enough to render a row
/// (thumbnail, place/time, status badge) without re-fetching full triage or
/// photo detail per item. Computed with a few correlated subqueries rather
/// than N+1 round-trips — plenty at this app's (personal-project) scale.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct SightingSummary {
    pub id: String,
    pub created_at: String,
    pub observed_at: String,
    pub place_label: Option<String>,
    pub lat: Option<f64>,
    pub lon: Option<f64>,
    pub status: String,
    pub latest_triage_status: Option<String>,
    pub latest_triage_genus: Option<String>,
    /// The top-confidence candidate species name from the latest triage
    /// attempt, if any were named (regardless of `latest_triage_status` —
    /// a `genus_candidate` attempt can still list species-level guesses).
    pub latest_triage_species: Option<String>,
    /// `best_match_species` from the latest deep-dive, if one has been run.
    pub latest_deepdive_best_match_species: Option<String>,
    /// The single highest-severity `danger_level` among the latest
    /// deep-dive's confusants, if any — lets the list screen show a safety
    /// signal (e.g. a red badge for a `deadly_toxic` look-alike) without a
    /// second request per row.
    pub latest_deepdive_danger_level: Option<String>,
    pub photo_count: i64,
    pub thumbnail_photo_id: Option<String>,
}

/// Raw row shape for the list query — carries the two JSON TEXT columns
/// needed to derive `latest_triage_species`/`latest_deepdive_danger_level`
/// before they're reduced into `SightingSummary`.
#[derive(sqlx::FromRow)]
struct SightingSummaryRow {
    id: String,
    created_at: String,
    observed_at: String,
    place_label: Option<String>,
    lat: Option<f64>,
    lon: Option<f64>,
    status: String,
    latest_triage_status: Option<String>,
    latest_triage_genus: Option<String>,
    latest_triage_candidate_species_json: Option<String>,
    latest_deepdive_best_match_species: Option<String>,
    latest_deepdive_confusants_json: Option<String>,
    photo_count: i64,
    thumbnail_photo_id: Option<String>,
}

/// Severity ordering for `DangerLevel` string values, highest-last so a
/// simple `max_by_key` picks the most alarming entry. Unrecognized/missing
/// values sort as `unknown` (lowest) rather than erroring — this is a
/// display nicety, never a source of truth for safety data.
fn danger_severity(level: &str) -> u8 {
    match level {
        "safe" => 1,
        "caution" => 2,
        "toxic" => 3,
        "deadly_toxic" => 4,
        _ => 0, // "unknown" or anything unrecognized
    }
}

/// The top-confidence `species` name out of a `candidate_species_json` TEXT
/// column, or `None` if absent/empty/unparseable.
fn top_candidate_species(raw: Option<&str>) -> Option<String> {
    #[derive(Deserialize)]
    struct Candidate {
        species: String,
        #[serde(default)]
        confidence: f64,
    }
    let raw = raw?;
    let candidates: Vec<Candidate> = serde_json::from_str(raw).ok()?;
    candidates
        .into_iter()
        .max_by(|a, b| a.confidence.total_cmp(&b.confidence))
        .map(|c| c.species)
}

/// The highest-severity `danger_level` out of a `confusants_json` TEXT
/// column, or `None` if absent/empty/unparseable.
fn highest_confusant_danger(raw: Option<&str>) -> Option<String> {
    #[derive(Deserialize)]
    struct ConfusantLevel {
        danger_level: String,
    }
    let raw = raw?;
    let confusants: Vec<ConfusantLevel> = serde_json::from_str(raw).ok()?;
    confusants
        .into_iter()
        .max_by_key(|c| danger_severity(&c.danger_level))
        .map(|c| c.danger_level)
}

impl From<SightingSummaryRow> for SightingSummary {
    fn from(row: SightingSummaryRow) -> Self {
        SightingSummary {
            latest_triage_species: top_candidate_species(
                row.latest_triage_candidate_species_json.as_deref(),
            ),
            latest_deepdive_danger_level: highest_confusant_danger(
                row.latest_deepdive_confusants_json.as_deref(),
            ),
            id: row.id,
            created_at: row.created_at,
            observed_at: row.observed_at,
            place_label: row.place_label,
            lat: row.lat,
            lon: row.lon,
            status: row.status,
            latest_triage_status: row.latest_triage_status,
            latest_triage_genus: row.latest_triage_genus,
            latest_deepdive_best_match_species: row.latest_deepdive_best_match_species,
            photo_count: row.photo_count,
            thumbnail_photo_id: row.thumbnail_photo_id,
        }
    }
}

/// The latest `triage_results` row for a sighting, as returned to clients.
/// `candidate_species`/`missing_info` are parsed from the stored JSON TEXT
/// columns into real JSON (never a doubly-escaped string).
#[derive(Debug, Clone, Serialize)]
pub struct TriageResultView {
    pub id: String,
    pub sighting_id: String,
    pub created_at: String,
    pub model: String,
    pub status: String,
    pub genus: Option<String>,
    pub candidate_species: serde_json::Value,
    pub missing_info: serde_json::Value,
    pub reasoning: String,
    pub photos_considered: i64,
}

#[derive(sqlx::FromRow)]
struct TriageResultRow {
    id: String,
    sighting_id: String,
    created_at: String,
    model: String,
    status: String,
    genus: Option<String>,
    candidate_species_json: String,
    missing_info_json: String,
    reasoning: String,
    photos_considered: i64,
}

impl From<TriageResultRow> for TriageResultView {
    fn from(row: TriageResultRow) -> Self {
        TriageResultView {
            candidate_species: parse_json_or_empty_array(&row.candidate_species_json),
            missing_info: parse_json_or_empty_array(&row.missing_info_json),
            id: row.id,
            sighting_id: row.sighting_id,
            created_at: row.created_at,
            model: row.model,
            status: row.status,
            genus: row.genus,
            reasoning: row.reasoning,
            photos_considered: row.photos_considered,
        }
    }
}

/// The latest `deepdive_results` row for a sighting, as returned to
/// clients. `confusants` is parsed from the stored JSON TEXT column.
#[derive(Debug, Clone, Serialize)]
pub struct DeepDiveResultView {
    pub id: String,
    pub sighting_id: String,
    pub created_at: String,
    pub model: String,
    pub best_match_species: String,
    pub confidence: f64,
    pub wikipedia_title: Option<String>,
    pub wikipedia_url: Option<String>,
    pub wikipedia_extract: Option<String>,
    pub confusants: serde_json::Value,
    pub safety_notes: String,
}

#[derive(sqlx::FromRow)]
struct DeepDiveResultRow {
    id: String,
    sighting_id: String,
    created_at: String,
    model: String,
    best_match_species: String,
    confidence: f64,
    wikipedia_title: Option<String>,
    wikipedia_url: Option<String>,
    wikipedia_extract: Option<String>,
    confusants_json: String,
    safety_notes: String,
}

impl From<DeepDiveResultRow> for DeepDiveResultView {
    fn from(row: DeepDiveResultRow) -> Self {
        DeepDiveResultView {
            confusants: parse_json_or_empty_array(&row.confusants_json),
            id: row.id,
            sighting_id: row.sighting_id,
            created_at: row.created_at,
            model: row.model,
            best_match_species: row.best_match_species,
            confidence: row.confidence,
            wikipedia_title: row.wikipedia_title,
            wikipedia_url: row.wikipedia_url,
            wikipedia_extract: row.wikipedia_extract,
            safety_notes: row.safety_notes,
        }
    }
}

/// The aggregate detail view: the sighting row plus everything its detail
/// screen needs in one request.
#[derive(Debug, Serialize)]
pub struct SightingDetail {
    #[serde(flatten)]
    pub sighting: Sighting,
    pub photos: Vec<PhotoView>,
    pub triage: Option<TriageResultView>,
    pub deepdive: Option<DeepDiveResultView>,
}

#[derive(Debug, Deserialize)]
struct CreateSightingReq {
    #[serde(default)]
    lat: Option<f64>,
    #[serde(default)]
    lon: Option<f64>,
    #[serde(default)]
    location_accuracy_m: Option<f64>,
    #[serde(default)]
    place_label: Option<String>,
    observed_at: String,
    #[serde(default)]
    notes: Option<String>,
}

#[derive(Debug, Deserialize)]
struct PatchSightingReq {
    #[serde(default)]
    notes: Option<String>,
    #[serde(default)]
    status: Option<String>,
}

// --- Helpers ------------------------------------------------------------------

fn now_rfc3339() -> AppResult<String> {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .map_err(|e| AppError::Internal(e.into()))
}

/// Normalize an optional free-text field: trim, and treat empty as absent.
fn normalize(s: Option<String>) -> Option<String> {
    s.map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

/// Parse a JSON-array TEXT column into real `serde_json::Value`. Falls back
/// to an empty array on malformed/missing data rather than failing the
/// whole aggregate response — a sibling module's write bug shouldn't 500 the
/// sighting detail screen.
fn parse_json_or_empty_array(raw: &str) -> serde_json::Value {
    serde_json::from_str(raw).unwrap_or_else(|_| serde_json::Value::Array(Vec::new()))
}

const SIGHTING_COLUMNS: &str = "id, user_id, status, lat, lon, location_accuracy_m, \
     place_label, observed_at, notes, created_at, updated_at";

async fn fetch_sighting(pool: &SqlitePool, id: &str) -> AppResult<Option<Sighting>> {
    sqlx::query_as::<_, Sighting>(&format!(
        "SELECT {SIGHTING_COLUMNS} FROM sightings WHERE id = ?"
    ))
    .bind(id)
    .fetch_optional(pool)
    .await
    .map_err(|e| AppError::Internal(e.into()))
}

/// Fetch a sighting scoped to `user_id`. A missing sighting and one owned by
/// another user are both `NotFound` — never `Forbidden` — so existence is
/// never leaked to a non-owner.
async fn owned_sighting(pool: &SqlitePool, user_id: &str, id: &str) -> AppResult<Sighting> {
    let sighting = fetch_sighting(pool, id).await?.ok_or(AppError::NotFound)?;
    if sighting.user_id != user_id {
        return Err(AppError::NotFound);
    }
    Ok(sighting)
}

#[allow(clippy::too_many_arguments)]
async fn insert_sighting(
    pool: &SqlitePool,
    user_id: &str,
    lat: Option<f64>,
    lon: Option<f64>,
    location_accuracy_m: Option<f64>,
    place_label: Option<&str>,
    observed_at: &str,
    notes: Option<&str>,
) -> AppResult<Sighting> {
    let observed_at = observed_at.trim();
    if observed_at.is_empty() {
        return Err(AppError::BadRequest("observed_at is required".to_string()));
    }
    OffsetDateTime::parse(observed_at, &Rfc3339)
        .map_err(|_| AppError::BadRequest("observed_at must be RFC3339".to_string()))?;

    let id = Uuid::new_v4().to_string();
    let now = now_rfc3339()?;
    sqlx::query(
        "INSERT INTO sightings \
             (id, user_id, status, lat, lon, location_accuracy_m, place_label, observed_at, \
              notes, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(user_id)
    .bind(SightingStatus::Open.as_str())
    .bind(lat)
    .bind(lon)
    .bind(location_accuracy_m)
    .bind(place_label)
    .bind(observed_at)
    .bind(notes)
    .bind(&now)
    .bind(&now)
    .execute(pool)
    .await
    .map_err(|e| AppError::Internal(e.into()))?;

    fetch_sighting(pool, &id).await?.ok_or(AppError::NotFound)
}

/// List a user's sightings, newest first, each with a cheap summary (latest
/// triage status/genus, photo count, a thumbnail photo id) computed via
/// correlated subqueries against `triage_results` and `photos` in the same
/// query.
async fn list_sightings_for(pool: &SqlitePool, user_id: &str) -> AppResult<Vec<SightingSummary>> {
    let rows = sqlx::query_as::<_, SightingSummaryRow>(
        "SELECT s.id, s.created_at, s.observed_at, s.place_label, s.lat, s.lon, s.status, \
                (SELECT t.status FROM triage_results t \
                   WHERE t.sighting_id = s.id ORDER BY t.created_at DESC LIMIT 1) AS latest_triage_status, \
                (SELECT t.genus FROM triage_results t \
                   WHERE t.sighting_id = s.id ORDER BY t.created_at DESC LIMIT 1) AS latest_triage_genus, \
                (SELECT t.candidate_species_json FROM triage_results t \
                   WHERE t.sighting_id = s.id ORDER BY t.created_at DESC LIMIT 1) AS latest_triage_candidate_species_json, \
                (SELECT d.best_match_species FROM deepdive_results d \
                   WHERE d.sighting_id = s.id ORDER BY d.created_at DESC LIMIT 1) AS latest_deepdive_best_match_species, \
                (SELECT d.confusants_json FROM deepdive_results d \
                   WHERE d.sighting_id = s.id ORDER BY d.created_at DESC LIMIT 1) AS latest_deepdive_confusants_json, \
                (SELECT COUNT(*) FROM photos p WHERE p.sighting_id = s.id) AS photo_count, \
                (SELECT p.id FROM photos p \
                   WHERE p.sighting_id = s.id ORDER BY p.sort_order ASC LIMIT 1) AS thumbnail_photo_id \
         FROM sightings s \
         WHERE s.user_id = ? \
         ORDER BY s.created_at DESC, s.id DESC",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await
    .map_err(|e| AppError::Internal(e.into()))?;

    Ok(rows.into_iter().map(SightingSummary::from).collect())
}

async fn latest_triage(
    pool: &SqlitePool,
    sighting_id: &str,
) -> AppResult<Option<TriageResultView>> {
    let row = sqlx::query_as::<_, TriageResultRow>(
        "SELECT id, sighting_id, created_at, model, status, genus, candidate_species_json, \
                missing_info_json, reasoning, photos_considered \
         FROM triage_results WHERE sighting_id = ? ORDER BY created_at DESC LIMIT 1",
    )
    .bind(sighting_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| AppError::Internal(e.into()))?;
    Ok(row.map(TriageResultView::from))
}

async fn latest_deepdive(
    pool: &SqlitePool,
    sighting_id: &str,
) -> AppResult<Option<DeepDiveResultView>> {
    let row = sqlx::query_as::<_, DeepDiveResultRow>(
        "SELECT id, sighting_id, created_at, model, best_match_species, confidence, \
                wikipedia_title, wikipedia_url, wikipedia_extract, confusants_json, safety_notes \
         FROM deepdive_results WHERE sighting_id = ? ORDER BY created_at DESC LIMIT 1",
    )
    .bind(sighting_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| AppError::Internal(e.into()))?;
    Ok(row.map(DeepDiveResultView::from))
}

/// Build the full aggregate detail view for a sighting, scoped to `user_id`.
async fn build_sighting_detail(
    pool: &SqlitePool,
    user_id: &str,
    id: &str,
) -> AppResult<SightingDetail> {
    let sighting = owned_sighting(pool, user_id, id).await?;
    let photos = crate::photos::list_for_sighting(pool, id).await?;
    let triage = latest_triage(pool, id).await?;
    let deepdive = latest_deepdive(pool, id).await?;
    Ok(SightingDetail {
        sighting,
        photos,
        triage,
        deepdive,
    })
}

async fn apply_sighting_patch(
    pool: &SqlitePool,
    user_id: &str,
    id: &str,
    patch: PatchSightingReq,
) -> AppResult<Sighting> {
    let mut sighting = owned_sighting(pool, user_id, id).await?;

    if let Some(notes) = patch.notes {
        sighting.notes = normalize(Some(notes));
    }
    if let Some(status) = patch.status {
        if SightingStatus::parse(&status).is_none() {
            return Err(AppError::BadRequest(format!(
                "invalid sighting status: {status}"
            )));
        }
        sighting.status = status;
    }
    sighting.updated_at = now_rfc3339()?;

    sqlx::query("UPDATE sightings SET notes = ?, status = ?, updated_at = ? WHERE id = ?")
        .bind(&sighting.notes)
        .bind(&sighting.status)
        .bind(&sighting.updated_at)
        .bind(&sighting.id)
        .execute(pool)
        .await
        .map_err(|e| AppError::Internal(e.into()))?;

    Ok(sighting)
}

// --- Handlers -----------------------------------------------------------------

async fn create_sighting(
    State(state): State<AppState>,
    RequireAuth(user_id): RequireAuth,
    WithRejection(Json(body), _): WithRejection<Json<CreateSightingReq>, AppError>,
) -> AppResult<Json<Sighting>> {
    let place_label = normalize(body.place_label);
    let notes = normalize(body.notes);
    let sighting = insert_sighting(
        &state.db,
        &user_id,
        body.lat,
        body.lon,
        body.location_accuracy_m,
        place_label.as_deref(),
        &body.observed_at,
        notes.as_deref(),
    )
    .await?;
    Ok(Json(sighting))
}

async fn list_sightings(
    State(state): State<AppState>,
    RequireAuth(user_id): RequireAuth,
) -> AppResult<Json<Vec<SightingSummary>>> {
    Ok(Json(list_sightings_for(&state.db, &user_id).await?))
}

async fn get_sighting(
    State(state): State<AppState>,
    RequireAuth(user_id): RequireAuth,
    Path(id): Path<String>,
) -> AppResult<Json<SightingDetail>> {
    Ok(Json(build_sighting_detail(&state.db, &user_id, &id).await?))
}

async fn patch_sighting(
    State(state): State<AppState>,
    RequireAuth(user_id): RequireAuth,
    Path(id): Path<String>,
    WithRejection(Json(body), _): WithRejection<Json<PatchSightingReq>, AppError>,
) -> AppResult<Json<Sighting>> {
    Ok(Json(
        apply_sighting_patch(&state.db, &user_id, &id, body).await?,
    ))
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/sightings", post(create_sighting).get(list_sightings))
        .route(
            "/api/sightings/{id}",
            get(get_sighting).patch(patch_sighting),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn top_candidate_species_picks_highest_confidence() {
        let json = r#"[{"species":"Agaricus bisporus","confidence":0.3},{"species":"Amanita phalloides","confidence":0.9}]"#;
        assert_eq!(
            top_candidate_species(Some(json)),
            Some("Amanita phalloides".to_string())
        );
    }

    #[test]
    fn top_candidate_species_handles_missing_or_invalid() {
        assert_eq!(top_candidate_species(None), None);
        assert_eq!(top_candidate_species(Some("not json")), None);
        assert_eq!(top_candidate_species(Some("[]")), None);
    }

    #[test]
    fn highest_confusant_danger_picks_most_severe() {
        let json = r#"[{"danger_level":"caution"},{"danger_level":"deadly_toxic"},{"danger_level":"safe"}]"#;
        assert_eq!(
            highest_confusant_danger(Some(json)),
            Some("deadly_toxic".to_string())
        );
    }

    #[test]
    fn highest_confusant_danger_handles_missing_or_invalid() {
        assert_eq!(highest_confusant_danger(None), None);
        assert_eq!(highest_confusant_danger(Some("not json")), None);
        assert_eq!(highest_confusant_danger(Some("[]")), None);
    }

    async fn test_pool() -> (tempfile::TempDir, SqlitePool) {
        let dir = tempfile::tempdir().expect("tempdir");
        let db_path = dir.path().join("test.db");
        let path = db_path.to_str().expect("utf8 path");
        let pool = crate::db::init_pool(path).await.expect("init pool");
        sqlx::query(
            "INSERT INTO users (id, email, display_name, created_at) \
             VALUES ('u1', NULL, NULL, '2026-01-01T00:00:00Z')",
        )
        .execute(&pool)
        .await
        .expect("insert user");
        (dir, pool)
    }

    /// `triage_results` / `deepdive_results` are owned by sibling agents and
    /// may not have landed as migration files yet; create minimal copies
    /// matching the schema frozen in `docs/ARCHITECTURE.md` so the detail
    /// aggregate can be exercised against real tables now.
    async fn create_sibling_tables(pool: &SqlitePool) {
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS triage_results ( \
                id TEXT PRIMARY KEY, \
                sighting_id TEXT NOT NULL, \
                created_at TEXT NOT NULL, \
                model TEXT NOT NULL, \
                status TEXT NOT NULL, \
                genus TEXT, \
                candidate_species_json TEXT NOT NULL, \
                missing_info_json TEXT NOT NULL, \
                reasoning TEXT NOT NULL, \
                photos_considered INTEGER NOT NULL \
            )",
        )
        .execute(pool)
        .await
        .expect("create triage_results");

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS deepdive_results ( \
                id TEXT PRIMARY KEY, \
                sighting_id TEXT NOT NULL, \
                created_at TEXT NOT NULL, \
                model TEXT NOT NULL, \
                best_match_species TEXT NOT NULL, \
                confidence REAL NOT NULL, \
                wikipedia_title TEXT, \
                wikipedia_url TEXT, \
                wikipedia_extract TEXT, \
                confusants_json TEXT NOT NULL, \
                safety_notes TEXT NOT NULL \
            )",
        )
        .execute(pool)
        .await
        .expect("create deepdive_results");
    }

    #[tokio::test]
    async fn creating_a_sighting_inserts_and_defaults_to_open() {
        let (_dir, pool) = test_pool().await;
        let created = insert_sighting(
            &pool,
            "u1",
            Some(51.5),
            Some(-0.1),
            None,
            Some("Hyde Park"),
            "2026-03-01T10:00:00Z",
            Some("saw some puffballs"),
        )
        .await
        .expect("insert sighting");

        assert_eq!(created.status, "open");
        assert_eq!(created.user_id, "u1");
        assert_eq!(created.place_label.as_deref(), Some("Hyde Park"));

        let (count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM sightings WHERE id = ?")
            .bind(&created.id)
            .fetch_one(&pool)
            .await
            .expect("count");
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn observed_at_must_be_present_and_rfc3339() {
        let (_dir, pool) = test_pool().await;

        let err = insert_sighting(&pool, "u1", None, None, None, None, "", None)
            .await
            .expect_err("empty observed_at must be rejected");
        assert!(matches!(err, AppError::BadRequest(_)));

        let err = insert_sighting(&pool, "u1", None, None, None, None, "not-a-date", None)
            .await
            .expect_err("malformed observed_at must be rejected");
        assert!(matches!(err, AppError::BadRequest(_)));
    }

    #[tokio::test]
    async fn another_users_sighting_is_not_found() {
        let (_dir, pool) = test_pool().await;
        sqlx::query(
            "INSERT INTO users (id, email, display_name, created_at) \
             VALUES ('u2', NULL, NULL, '2026-01-01T00:00:00Z')",
        )
        .execute(&pool)
        .await
        .expect("insert u2");

        let created = insert_sighting(
            &pool,
            "u2",
            None,
            None,
            None,
            None,
            "2026-03-01T10:00:00Z",
            None,
        )
        .await
        .expect("insert sighting");

        let err = owned_sighting(&pool, "u1", &created.id)
            .await
            .expect_err("another user's sighting must be NotFound, not Forbidden");
        assert!(matches!(err, AppError::NotFound));

        let missing = owned_sighting(&pool, "u1", "does-not-exist")
            .await
            .expect_err("missing sighting must be NotFound");
        assert!(matches!(missing, AppError::NotFound));
    }

    #[tokio::test]
    async fn patch_updates_notes_and_status() {
        let (_dir, pool) = test_pool().await;
        let created = insert_sighting(
            &pool,
            "u1",
            None,
            None,
            None,
            None,
            "2026-03-01T10:00:00Z",
            None,
        )
        .await
        .expect("insert sighting");

        let patched = apply_sighting_patch(
            &pool,
            "u1",
            &created.id,
            PatchSightingReq {
                notes: Some("updated note".to_string()),
                status: Some("archived".to_string()),
            },
        )
        .await
        .expect("patch sighting");

        assert_eq!(patched.notes.as_deref(), Some("updated note"));
        assert_eq!(patched.status, "archived");
        assert!(patched.updated_at >= created.updated_at);
    }

    #[tokio::test]
    async fn patch_rejects_invalid_status() {
        let (_dir, pool) = test_pool().await;
        let created = insert_sighting(
            &pool,
            "u1",
            None,
            None,
            None,
            None,
            "2026-03-01T10:00:00Z",
            None,
        )
        .await
        .expect("insert sighting");

        let err = apply_sighting_patch(
            &pool,
            "u1",
            &created.id,
            PatchSightingReq {
                notes: None,
                status: Some("not-a-status".to_string()),
            },
        )
        .await
        .expect_err("invalid status must be rejected");
        assert!(matches!(err, AppError::BadRequest(_)));
    }

    #[tokio::test]
    async fn list_orders_newest_first_and_reports_photo_count() {
        let (_dir, pool) = test_pool().await;
        create_sibling_tables(&pool).await;

        let first = insert_sighting(
            &pool,
            "u1",
            None,
            None,
            None,
            None,
            "2026-03-01T10:00:00Z",
            None,
        )
        .await
        .expect("insert first");
        // Ensure a distinct created_at ordering regardless of clock
        // resolution.
        sqlx::query("UPDATE sightings SET created_at = '2026-01-01T00:00:00Z' WHERE id = ?")
            .bind(&first.id)
            .execute(&pool)
            .await
            .expect("backdate first");

        let second = insert_sighting(
            &pool,
            "u1",
            None,
            None,
            None,
            None,
            "2026-03-02T10:00:00Z",
            None,
        )
        .await
        .expect("insert second");
        sqlx::query("UPDATE sightings SET created_at = '2026-06-01T00:00:00Z' WHERE id = ?")
            .bind(&second.id)
            .execute(&pool)
            .await
            .expect("backdate second");

        sqlx::query(
            "INSERT INTO photos (id, sighting_id, file_path, content_type, width, height, \
                 taken_at, sort_order, created_at) \
             VALUES ('p1', ?, 'p1.jpg', 'image/jpeg', 100, 100, '2026-06-01T00:00:00Z', 0, '2026-06-01T00:00:00Z')",
        )
        .bind(&second.id)
        .execute(&pool)
        .await
        .expect("insert photo");

        let list = list_sightings_for(&pool, "u1").await.expect("list");
        assert_eq!(list.len(), 2);
        assert_eq!(
            list.first().map(|s| s.id.as_str()),
            Some(second.id.as_str())
        );
        assert_eq!(list.first().map(|s| s.photo_count), Some(1));
        assert_eq!(
            list.first().map(|s| s.thumbnail_photo_id.clone()),
            Some(Some("p1".to_string()))
        );
        assert_eq!(list.get(1).map(|s| s.id.as_str()), Some(first.id.as_str()));
        assert_eq!(list.get(1).map(|s| s.photo_count), Some(0));
    }

    #[tokio::test]
    async fn detail_aggregates_photos_triage_and_deepdive() {
        let (_dir, pool) = test_pool().await;
        create_sibling_tables(&pool).await;

        let sighting = insert_sighting(
            &pool,
            "u1",
            None,
            None,
            None,
            None,
            "2026-03-01T10:00:00Z",
            None,
        )
        .await
        .expect("insert sighting");

        // Before any photo/triage/deepdive: detail still succeeds, with
        // empty photos and no triage/deepdive.
        let empty_detail = build_sighting_detail(&pool, "u1", &sighting.id)
            .await
            .expect("detail before any photo");
        assert!(empty_detail.photos.is_empty());
        assert!(empty_detail.triage.is_none());
        assert!(empty_detail.deepdive.is_none());

        sqlx::query(
            "INSERT INTO photos (id, sighting_id, file_path, content_type, width, height, \
                 taken_at, sort_order, created_at) \
             VALUES ('p1', ?, 'p1.jpg', 'image/jpeg', 100, 100, '2026-03-01T10:05:00Z', 0, '2026-03-01T10:05:00Z')",
        )
        .bind(&sighting.id)
        .execute(&pool)
        .await
        .expect("insert photo");

        sqlx::query(
            "INSERT INTO triage_results \
                 (id, sighting_id, created_at, model, status, genus, candidate_species_json, \
                  missing_info_json, reasoning, photos_considered) \
             VALUES ('t1', ?, '2026-03-01T10:06:00Z', 'test-model', 'genus_candidate', 'Amanita', \
                      '[{\"species\":\"Amanita phalloides\",\"common_name\":\"Death cap\",\"confidence\":0.6}]', \
                      '[]', 'looks like Amanita', 1)",
        )
        .bind(&sighting.id)
        .execute(&pool)
        .await
        .expect("insert triage");

        sqlx::query(
            "INSERT INTO deepdive_results \
                 (id, sighting_id, created_at, model, best_match_species, confidence, \
                  wikipedia_title, wikipedia_url, wikipedia_extract, confusants_json, safety_notes) \
             VALUES ('d1', ?, '2026-03-01T10:10:00Z', 'test-model', 'Amanita phalloides', 0.6, \
                      'Amanita phalloides', 'https://en.wikipedia.org/wiki/Amanita_phalloides', \
                      'A deadly poisonous mushroom.', '[]', 'Never eat anything based on this app alone.')",
        )
        .bind(&sighting.id)
        .execute(&pool)
        .await
        .expect("insert deepdive");

        let detail = build_sighting_detail(&pool, "u1", &sighting.id)
            .await
            .expect("full detail");
        assert_eq!(detail.photos.len(), 1);
        let triage = detail.triage.expect("triage present");
        assert_eq!(triage.status, "genus_candidate");
        assert_eq!(triage.genus.as_deref(), Some("Amanita"));
        assert!(triage.candidate_species.is_array());
        assert_eq!(
            triage.candidate_species.as_array().map(|a| a.len()),
            Some(1)
        );
        let deepdive = detail.deepdive.expect("deepdive present");
        assert_eq!(deepdive.best_match_species, "Amanita phalloides");

        // Another user cannot see it.
        let err = build_sighting_detail(&pool, "someone-else", &sighting.id)
            .await
            .expect_err("must be NotFound for a non-owner");
        assert!(matches!(err, AppError::NotFound));
    }

    #[test]
    fn malformed_json_column_degrades_to_empty_array() {
        assert_eq!(
            parse_json_or_empty_array("not json"),
            serde_json::Value::Array(Vec::new())
        );
        assert_eq!(
            parse_json_or_empty_array("[1,2,3]"),
            serde_json::json!([1, 2, 3])
        );
    }
}
