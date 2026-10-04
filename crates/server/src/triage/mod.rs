//! The fast "triage" pass: a cheap, vision-capable LLM call that looks at a
//! sighting's photos (+ location/date/notes) and returns a provisional
//! genus/species guess, or honestly says it doesn't know yet and asks for
//! specific additional photos/info.
//!
//! Runs automatically after every photo upload (`photos::on_photo_uploaded`
//! calls [`run_triage`] directly) and can be manually re-run by the user
//! (e.g. after editing notes) via `POST /api/sightings/{id}/triage`.
//!
//! **Safety note:** this module's system prompt is the most safety-critical
//! text in the app (see `docs/ARCHITECTURE.md`). Misidentifying a foraged
//! mushroom or plant can kill, so the prompt is written to bias hard toward
//! `status: "insufficient"` with concrete asks over a confident-sounding
//! wrong answer. Do not loosen that language.

use std::path::Path as FsPath;

use axum::extract::{Path, State};
use axum::routing::get;
use axum::{Json, Router};
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::auth::session::RequireAuth;
use crate::error::{AppError, AppResult};
use crate::state::AppState;
use forage_buddy_core::domain::TriageStatus;

/// Hard cap on how many of a sighting's photos are sent to the vision model
/// per triage attempt. Foraging sightings can accumulate many photos over
/// repeated "send more" round-trips; capping keeps the request bounded
/// (latency + provider cost) while still giving the model the photos most
/// likely to answer what it just asked for (the most recently added ones).
const MAX_PHOTOS_PER_ATTEMPT: usize = 6;

/// Model value stored when no LLM call was made for an attempt (e.g. zero
/// photos yet, or all stored photo files were unreadable).
const NO_MODEL_CALL: &str = "none";

/// System prompt for the triage vision call. See the module-level safety
/// note above before editing this.
const SYSTEM_PROMPT: &str = "You are Forage Buddy's triage assistant: a careful field-identification \
aid for wild fungi, plants, and other foraged organisms. You are shown photos \
of a specimen plus its observation date, rough location, and any notes the \
forager wrote, and you give a best-effort, provisional read: the most likely \
genus, and a specific species only when you are genuinely confident.\n\
\n\
SAFETY IS THE ENTIRE POINT OF THIS TOOL. Misidentifying a wild mushroom or \
plant can cause severe illness or death. You are not a substitute for a \
qualified local expert, a spore print, or a field guide, and every answer you \
give is shown to the user alongside a prominent disclaimer to that effect. \
Given that, you must NEVER state or imply more certainty than you actually \
have, and you must NEVER guess at a genus or species just to produce an \
answer. When the photos and context are not enough to narrow things down \
responsibly, the correct and EXPECTED response is \
status = \"insufficient\" with concrete, specific missing_info items \
describing exactly what would help next \u{2014} not a vague \"need more \
info\". Err strongly on the side of asking for more over committing to a \
confident-sounding wrong answer: a wrong guess can kill, an honest \"I don't \
know yet, please get me X\" cannot.\n\
\n\
Good examples of specific, foraging-relevant asks (draw on patterns like \
these, adapted to what you actually see and still need):\n\
- \"a clear photo of the gill attachment to the stem (free, attached, notched, or decurrent)\"\n\
- \"the spore print color (rest the cap gill-side-down on paper for a few hours)\"\n\
- \"a photo of the full plant/fungus including the base or root, not just the top\"\n\
- \"whether the flesh bruises, stains, or changes color when cut or scratched, and to what color\"\n\
- \"a clear photo of the underside: gills, pores, or teeth, in good light\"\n\
- \"whether there is a ring (annulus) on the stem, and/or a cup (volva) at the base\"\n\
- \"the habitat: growing on wood, on soil, or on a specific host tree species\"\n\
- \"the smell of the specimen (e.g. anise, almond, foul, bleach-like, none noticed)\"\n\
- \"a photo showing true scale next to a coin or ruler\"\n\
\n\
Rules:\n\
1. Treat the photos as the primary evidence. Use the observation date and \
rough location only to narrow candidates by season/region plausibility — \
never claim to know the exact location, habitat, or anything not actually \
visible or stated.\n\
2. If you are told what the user was previously asked to provide, check \
whether the newly supplied photo(s)/notes actually answer that before asking \
for something else.\n\
3. Only use status = \"species_candidate\" when you have real confidence in \
one or a few specific species. Prefer \"genus_candidate\" when the genus is \
reasonably clear but the species is not. Use \"insufficient\" whenever you \
cannot responsibly commit to even a genus.\n\
4. List multiple plausible candidate_species when relevant, INCLUDING \
dangerous look-alikes you want the user to rule out, each with an honest \
confidence between 0 and 1. Do not inflate confidence to sound more useful.\n\
5. `reasoning` must be one or two honest sentences naming the key visual \
features you relied on (or the key features you could not see, if \
insufficient).\n\
6. Respond with ONLY a single JSON object, no other text, matching exactly:\n\
{\"status\": \"insufficient\" | \"genus_candidate\" | \"species_candidate\", \
\"genus\": string or null, \"candidate_species\": [{\"species\": string, \
\"common_name\": string or null, \"confidence\": number between 0 and 1}], \
\"missing_info\": [string, ...], \"reasoning\": string}";

/// One candidate species/guess, as produced by the LLM and stored/returned
/// verbatim (parsed, not raw JSON) to the frontend.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TriageCandidateSpecies {
    pub species: String,
    #[serde(default)]
    pub common_name: Option<String>,
    pub confidence: f32,
}

/// Raw shape returned by the vision LLM call.
#[derive(Debug, Deserialize)]
struct TriageLlmOutput {
    status: TriageStatus,
    #[serde(default)]
    genus: Option<String>,
    #[serde(default)]
    candidate_species: Vec<TriageCandidateSpecies>,
    #[serde(default)]
    missing_info: Vec<String>,
    #[serde(default)]
    reasoning: String,
}

/// A stored triage attempt, as returned to the frontend. Mirrors the
/// `triage_results` row, but with `candidate_species`/`missing_info` as real
/// parsed JSON arrays rather than the `_json` TEXT columns they're stored in.
#[derive(Debug, Clone, Serialize)]
pub struct TriageResultDto {
    pub id: String,
    pub sighting_id: String,
    pub created_at: String,
    pub model: String,
    pub status: TriageStatus,
    pub genus: Option<String>,
    pub candidate_species: Vec<TriageCandidateSpecies>,
    pub missing_info: Vec<String>,
    pub reasoning: String,
    pub photos_considered: i64,
}

#[derive(sqlx::FromRow)]
struct SightingForTriage {
    lat: Option<f64>,
    lon: Option<f64>,
    place_label: Option<String>,
    observed_at: String,
    notes: Option<String>,
}

#[derive(sqlx::FromRow)]
struct PhotoForTriage {
    file_path: String,
    content_type: String,
}

#[derive(sqlx::FromRow)]
struct TriageRow {
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

fn row_to_dto(row: TriageRow) -> AppResult<TriageResultDto> {
    let status = TriageStatus::parse(&row.status).ok_or_else(|| {
        AppError::Internal(anyhow::anyhow!(
            "triage_results row {} has invalid status {:?}",
            row.id,
            row.status
        ))
    })?;
    let candidate_species: Vec<TriageCandidateSpecies> =
        serde_json::from_str(&row.candidate_species_json)
            .map_err(|e| AppError::Internal(e.into()))?;
    let missing_info: Vec<String> =
        serde_json::from_str(&row.missing_info_json).map_err(|e| AppError::Internal(e.into()))?;

    Ok(TriageResultDto {
        id: row.id,
        sighting_id: row.sighting_id,
        created_at: row.created_at,
        model: row.model,
        status,
        genus: row.genus,
        candidate_species,
        missing_info,
        reasoning: row.reasoning,
        photos_considered: row.photos_considered,
    })
}

fn format_rfc3339(t: OffsetDateTime) -> AppResult<String> {
    t.format(&Rfc3339).map_err(|e| AppError::Internal(e.into()))
}

/// Insert a new attempt (whether or not an LLM call actually happened) and
/// return it as a [`TriageResultDto`].
#[allow(clippy::too_many_arguments)]
async fn persist(
    db: &SqlitePool,
    sighting_id: &str,
    model: &str,
    status: TriageStatus,
    genus: Option<String>,
    candidate_species: Vec<TriageCandidateSpecies>,
    missing_info: Vec<String>,
    reasoning: String,
    photos_considered: i64,
) -> AppResult<TriageResultDto> {
    let id = Uuid::new_v4().to_string();
    let created_at = format_rfc3339(OffsetDateTime::now_utc())?;
    let candidate_species_json =
        serde_json::to_string(&candidate_species).map_err(|e| AppError::Internal(e.into()))?;
    let missing_info_json =
        serde_json::to_string(&missing_info).map_err(|e| AppError::Internal(e.into()))?;

    sqlx::query(
        "INSERT INTO triage_results \
         (id, sighting_id, created_at, model, status, genus, candidate_species_json, \
          missing_info_json, reasoning, photos_considered) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(sighting_id)
    .bind(&created_at)
    .bind(model)
    .bind(status.as_str())
    .bind(&genus)
    .bind(&candidate_species_json)
    .bind(&missing_info_json)
    .bind(&reasoning)
    .bind(photos_considered)
    .execute(db)
    .await
    .map_err(|e| AppError::Internal(e.into()))?;

    Ok(TriageResultDto {
        id,
        sighting_id: sighting_id.to_string(),
        created_at,
        model: model.to_string(),
        status,
        genus,
        candidate_species,
        missing_info,
        reasoning,
        photos_considered,
    })
}

/// Run (or re-run) the triage pass for a sighting: load its photos, call the
/// vision LLM (unless there's nothing to show it), and persist+return the
/// new attempt. Errors with [`AppError::NotFound`] if the sighting doesn't
/// exist.
pub async fn run_triage(state: &AppState, sighting_id: &str) -> AppResult<TriageResultDto> {
    let sighting: Option<SightingForTriage> = sqlx::query_as(
        "SELECT lat, lon, place_label, observed_at, notes FROM sightings WHERE id = ?",
    )
    .bind(sighting_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| AppError::Internal(e.into()))?;
    let sighting = sighting.ok_or(AppError::NotFound)?;

    let photos: Vec<PhotoForTriage> = sqlx::query_as(
        "SELECT file_path, content_type FROM photos WHERE sighting_id = ? ORDER BY sort_order ASC",
    )
    .bind(sighting_id)
    .fetch_all(&state.db)
    .await
    .map_err(|e| AppError::Internal(e.into()))?;

    if photos.is_empty() {
        return persist(
            &state.db,
            sighting_id,
            NO_MODEL_CALL,
            TriageStatus::Insufficient,
            None,
            Vec::new(),
            vec!["at least one photo".to_string()],
            "No photos have been uploaded yet for this sighting.".to_string(),
            0,
        )
        .await;
    }

    // Most recent prior attempt's `missing_info`, if any, so the prompt can
    // tell the model what the new photo(s) are meant to answer.
    let prior_missing_info: Option<String> = sqlx::query_as::<_, (String,)>(
        "SELECT missing_info_json FROM triage_results WHERE sighting_id = ? \
         ORDER BY created_at DESC LIMIT 1",
    )
    .bind(sighting_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| AppError::Internal(e.into()))?
    .map(|(json,)| json);
    let prior_missing_info: Vec<String> = match prior_missing_info {
        Some(json) => serde_json::from_str(&json).unwrap_or_default(),
        None => Vec::new(),
    };

    // Keep only the most recently added photos, bounded by
    // `MAX_PHOTOS_PER_ATTEMPT`, since those are the ones most likely to
    // answer a prior "send me X" ask.
    let skip = photos.len().saturating_sub(MAX_PHOTOS_PER_ATTEMPT);
    let recent_photos: Vec<&PhotoForTriage> = photos.iter().skip(skip).collect();

    let mut images: Vec<String> = Vec::with_capacity(recent_photos.len());
    for photo in &recent_photos {
        let path = FsPath::new(&state.config.photo_dir).join(&photo.file_path);
        match tokio::fs::read(&path).await {
            Ok(bytes) => images.push(format!(
                "data:{};base64,{}",
                photo.content_type,
                BASE64.encode(bytes)
            )),
            Err(err) => {
                tracing::warn!(
                    error = %err,
                    path = %path.display(),
                    sighting_id,
                    "triage: failed to read a stored photo from disk; skipping it"
                );
            }
        }
    }

    if images.is_empty() {
        return persist(
            &state.db,
            sighting_id,
            NO_MODEL_CALL,
            TriageStatus::Insufficient,
            None,
            Vec::new(),
            vec!["at least one successfully stored photo (the saved photo file(s) could not be read)".to_string()],
            "This sighting has photo records, but the stored image file(s) could not be read from disk.".to_string(),
            0,
        )
        .await;
    }

    let mut user_text_parts: Vec<String> = Vec::new();
    user_text_parts.push(format!("Observed at: {}", sighting.observed_at));
    if let (Some(lat), Some(lon)) = (sighting.lat, sighting.lon) {
        user_text_parts.push(format!(
            "Location: latitude {lat:.5}, longitude {lon:.5} (raw coordinates \u{2014} use only to narrow \
             candidates by region/season, do not attempt to name or reverse-geocode the place)."
        ));
    }
    if let Some(place_label) = sighting
        .place_label
        .as_deref()
        .filter(|s| !s.trim().is_empty())
    {
        user_text_parts.push(format!("User-supplied place label: {place_label}"));
    }
    if let Some(notes) = sighting.notes.as_deref().filter(|s| !s.trim().is_empty()) {
        user_text_parts.push(format!("Forager's notes: {notes}"));
    }
    if !prior_missing_info.is_empty() {
        user_text_parts.push(format!(
            "On the previous triage attempt, the user was asked to provide: {}. Check whether the \
             photo(s) below answer this before asking for something else.",
            prior_missing_info.join("; ")
        ));
    }
    user_text_parts.push(format!(
        "{} photo(s) attached (most recent {} of {} total for this sighting).",
        images.len(),
        images.len(),
        photos.len()
    ));
    let user_text = user_text_parts.join("\n\n");

    let model = state.llm.default_chat_model().to_string();
    let llm_output: TriageLlmOutput = state
        .llm
        .chat_json_vision(&model, SYSTEM_PROMPT, &user_text, &images)
        .await?;

    persist(
        &state.db,
        sighting_id,
        &model,
        llm_output.status,
        llm_output.genus,
        llm_output.candidate_species,
        llm_output.missing_info,
        llm_output.reasoning,
        images.len() as i64,
    )
    .await
}

/// Full triage history for a sighting, newest first.
async fn fetch_history(db: &SqlitePool, sighting_id: &str) -> AppResult<Vec<TriageResultDto>> {
    let rows: Vec<TriageRow> = sqlx::query_as(
        "SELECT id, sighting_id, created_at, model, status, genus, candidate_species_json, \
         missing_info_json, reasoning, photos_considered \
         FROM triage_results WHERE sighting_id = ? ORDER BY created_at DESC",
    )
    .bind(sighting_id)
    .fetch_all(db)
    .await
    .map_err(|e| AppError::Internal(e.into()))?;

    rows.into_iter().map(row_to_dto).collect()
}

/// `NotFound` unless `sighting_id` exists and is owned by `user_id` — used by
/// both routes so a caller can't probe/trigger triage on someone else's
/// sighting.
async fn ensure_owned(db: &SqlitePool, sighting_id: &str, user_id: &str) -> AppResult<()> {
    let owner: Option<(String,)> = sqlx::query_as("SELECT user_id FROM sightings WHERE id = ?")
        .bind(sighting_id)
        .fetch_optional(db)
        .await
        .map_err(|e| AppError::Internal(e.into()))?;

    match owner {
        Some((owner_id,)) if owner_id == user_id => Ok(()),
        _ => Err(AppError::NotFound),
    }
}

async fn list_triage(
    State(state): State<AppState>,
    RequireAuth(user_id): RequireAuth,
    Path(sighting_id): Path<String>,
) -> AppResult<Json<Vec<TriageResultDto>>> {
    ensure_owned(&state.db, &sighting_id, &user_id).await?;
    let history = fetch_history(&state.db, &sighting_id).await?;
    Ok(Json(history))
}

async fn rerun_triage(
    State(state): State<AppState>,
    RequireAuth(user_id): RequireAuth,
    Path(sighting_id): Path<String>,
) -> AppResult<Json<TriageResultDto>> {
    ensure_owned(&state.db, &sighting_id, &user_id).await?;
    let result = run_triage(&state, &sighting_id).await?;
    Ok(Json(result))
}

pub fn router() -> Router<AppState> {
    Router::new().route(
        "/api/sightings/{id}/triage",
        get(list_triage).post(rerun_triage),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh temp-file sqlite pool with just enough schema (users,
    /// sightings, photos, triage_results) to exercise this module, built
    /// independently of the `sightings`/`photos` modules' own migrations
    /// (owned by a sibling agent) so this test doesn't depend on landing
    /// order. `IF NOT EXISTS` keeps it safe if those migrations are already
    /// present too.
    async fn test_state() -> (tempfile::TempDir, AppState) {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let db = crate::db::init_pool(db_path.to_str().unwrap())
            .await
            .unwrap();

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS sightings ( \
                id TEXT PRIMARY KEY, user_id TEXT NOT NULL, status TEXT NOT NULL DEFAULT 'open', \
                lat REAL, lon REAL, location_accuracy_m REAL, place_label TEXT, \
                observed_at TEXT NOT NULL, notes TEXT, created_at TEXT NOT NULL, updated_at TEXT NOT NULL \
            )",
        )
        .execute(&db)
        .await
        .unwrap();
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS photos ( \
                id TEXT PRIMARY KEY, sighting_id TEXT NOT NULL, file_path TEXT NOT NULL, \
                content_type TEXT NOT NULL, width INTEGER, height INTEGER, taken_at TEXT NOT NULL, \
                sort_order INTEGER NOT NULL, created_at TEXT NOT NULL \
            )",
        )
        .execute(&db)
        .await
        .unwrap();

        sqlx::query(
            "INSERT INTO users (id, email, display_name, created_at) VALUES ('u1', NULL, NULL, '2026-01-01T00:00:00Z')",
        )
        .execute(&db)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO sightings (id, user_id, status, observed_at, created_at, updated_at) \
             VALUES ('s1', 'u1', 'open', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
        )
        .execute(&db)
        .await
        .unwrap();

        let config = crate::config::Config::from_env();
        let llm = crate::llm::LlmClient::new(&config);
        let state = AppState {
            db,
            config: std::sync::Arc::new(config),
            oidc: None,
            cookie_key: axum_extra::extract::cookie::Key::generate(),
            llm,
        };
        (dir, state)
    }

    #[tokio::test]
    async fn zero_photos_returns_insufficient_without_calling_llm() {
        let (_dir, state) = test_state().await;

        let result = run_triage(&state, "s1").await.unwrap();

        assert_eq!(result.status, TriageStatus::Insufficient);
        assert_eq!(result.model, NO_MODEL_CALL);
        assert_eq!(result.photos_considered, 0);
        assert_eq!(result.missing_info, vec!["at least one photo".to_string()]);
        assert!(result.candidate_species.is_empty());
        assert!(result.genus.is_none());
    }

    #[tokio::test]
    async fn zero_photos_attempt_is_persisted_in_history() {
        let (_dir, state) = test_state().await;

        run_triage(&state, "s1").await.unwrap();
        let history = fetch_history(&state.db, "s1").await.unwrap();

        assert_eq!(history.len(), 1);
        let only = history.into_iter().next().unwrap();
        assert_eq!(only.status, TriageStatus::Insufficient);
    }

    #[tokio::test]
    async fn unreadable_photo_falls_back_to_insufficient_without_calling_llm() {
        let (_dir, state) = test_state().await;

        // A photo row whose file was never actually written to disk.
        sqlx::query(
            "INSERT INTO photos (id, sighting_id, file_path, content_type, taken_at, sort_order, created_at) \
             VALUES ('p1', 's1', 'missing.jpg', 'image/jpeg', '2026-01-01T00:00:00Z', 0, '2026-01-01T00:00:00Z')",
        )
        .execute(&state.db)
        .await
        .unwrap();

        let result = run_triage(&state, "s1").await.unwrap();

        assert_eq!(result.status, TriageStatus::Insufficient);
        assert_eq!(result.model, NO_MODEL_CALL);
        assert_eq!(result.photos_considered, 0);
    }

    #[tokio::test]
    async fn run_triage_errors_not_found_for_missing_sighting() {
        let (_dir, state) = test_state().await;

        let err = run_triage(&state, "does-not-exist").await.unwrap_err();

        assert!(matches!(err, AppError::NotFound));
    }

    #[tokio::test]
    async fn row_to_dto_rejects_invalid_status() {
        let row = TriageRow {
            id: "t1".to_string(),
            sighting_id: "s1".to_string(),
            created_at: "2026-01-01T00:00:00Z".to_string(),
            model: "none".to_string(),
            status: "not_a_real_status".to_string(),
            genus: None,
            candidate_species_json: "[]".to_string(),
            missing_info_json: "[]".to_string(),
            reasoning: "x".to_string(),
            photos_considered: 0,
        };

        assert!(row_to_dto(row).is_err());
    }
}
