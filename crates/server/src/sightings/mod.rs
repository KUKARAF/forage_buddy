//! Sightings: a user's foraging observations, plus the aggregate detail
//! view the frontend's detail screen renders from a single request.
//!
//! Owns migration `0002_sightings.sql` (`sightings`). Also reads (but does
//! not own) `photos` (the `photos` module, same agent) and
//! `identification_results` (the `identification` module, via its own
//! `identification::get_latest` for the detail aggregate, plus a direct
//! correlated subquery here for the list screen's cheap summary — same
//! pattern the old triage/deepdive-reading code used).
//!
//! Routes (all require auth, scoped to the caller's own `user_id` — a
//! sighting owned by someone else 404s rather than 403s, so existence is
//! never leaked):
//!   - `POST  /api/sightings`      create a sighting (status `open`)
//!   - `GET   /api/sightings`      list the caller's sightings, newest first
//!   - `GET   /api/sightings/{id}` sighting + photos + latest identification
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
use crate::identification::IdentificationResultDto;
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
    /// Status of the sighting's latest identification attempt
    /// (`pending`/`partial`/`complete`/`insufficient`/`failed`), if one has
    /// ever been run.
    pub latest_identification_status: Option<String>,
    /// The top-confidence candidate species name from the latest
    /// identification attempt, if any were named.
    pub latest_identification_species: Option<String>,
    /// The single highest-severity danger signal among the latest
    /// identification's candidates (each candidate's own `poisonous` flag,
    /// plus every confusant's `danger_level`) — lets the list screen show a
    /// safety signal (e.g. a red badge for a `deadly_toxic` look-alike)
    /// without a second request per row. One of `unknown`/`mild`/`toxic`/
    /// `deadly_toxic`, matching the identification API's vocabulary.
    pub latest_identification_danger_level: Option<String>,
    pub photo_count: i64,
    pub thumbnail_photo_id: Option<String>,
}

/// Raw row shape for the list query — carries the latest identification's
/// JSON TEXT column needed to derive `latest_identification_species`/
/// `latest_identification_danger_level` before they're reduced into
/// `SightingSummary`.
#[derive(sqlx::FromRow)]
struct SightingSummaryRow {
    id: String,
    created_at: String,
    observed_at: String,
    place_label: Option<String>,
    lat: Option<f64>,
    lon: Option<f64>,
    status: String,
    latest_identification_status: Option<String>,
    latest_identification_candidates_json: Option<String>,
    photo_count: i64,
    thumbnail_photo_id: Option<String>,
}

/// Severity ordering for the identification API's condensed `danger_level`
/// vocabulary, highest-last so a simple `max_by_key` picks the most
/// alarming entry. Unrecognized/missing values sort as `unknown` (lowest)
/// rather than erroring — this is a display nicety, never a source of truth
/// for safety data.
fn danger_severity(level: &str) -> u8 {
    match level {
        "mild" => 1,
        "toxic" => 2,
        "deadly_toxic" => 3,
        _ => 0, // "unknown" or anything unrecognized
    }
}

/// The top-confidence candidate's `species` name out of a `candidates_json`
/// TEXT column, or `None` if absent/empty/unparseable. Candidates are
/// already sorted by confidence (descending) by
/// `identification::compile_candidates`, so the first entry is the top one.
fn top_candidate_species(raw: Option<&str>) -> Option<String> {
    #[derive(Deserialize)]
    struct Candidate {
        species: String,
    }
    let raw = raw?;
    let candidates: Vec<Candidate> = serde_json::from_str(raw).ok()?;
    candidates.into_iter().next().map(|c| c.species)
}

/// The highest-severity danger signal across every candidate's `poisonous`
/// flag and every confusant's `danger_level`, out of a `candidates_json`
/// TEXT column, or `None` if absent/empty/unparseable.
fn highest_danger_level(raw: Option<&str>) -> Option<String> {
    #[derive(Deserialize)]
    struct Confusant {
        danger_level: String,
    }
    #[derive(Deserialize)]
    struct Candidate {
        #[serde(default)]
        poisonous: Option<bool>,
        #[serde(default)]
        confusants: Vec<Confusant>,
    }
    let raw = raw?;
    let candidates: Vec<Candidate> = serde_json::from_str(raw).ok()?;

    let mut levels: Vec<&str> = Vec::new();
    for candidate in &candidates {
        if candidate.poisonous == Some(true) {
            levels.push("toxic");
        }
        for confusant in &candidate.confusants {
            levels.push(confusant.danger_level.as_str());
        }
    }
    levels
        .into_iter()
        .max_by_key(|level| danger_severity(level))
        .map(str::to_string)
}

impl From<SightingSummaryRow> for SightingSummary {
    fn from(row: SightingSummaryRow) -> Self {
        SightingSummary {
            latest_identification_species: top_candidate_species(
                row.latest_identification_candidates_json.as_deref(),
            ),
            latest_identification_danger_level: highest_danger_level(
                row.latest_identification_candidates_json.as_deref(),
            ),
            id: row.id,
            created_at: row.created_at,
            observed_at: row.observed_at,
            place_label: row.place_label,
            lat: row.lat,
            lon: row.lon,
            status: row.status,
            latest_identification_status: row.latest_identification_status,
            photo_count: row.photo_count,
            thumbnail_photo_id: row.thumbnail_photo_id,
        }
    }
}

/// The aggregate detail view: the sighting row plus everything its detail
/// screen needs in one request.
#[derive(Debug, Serialize)]
pub struct SightingDetail {
    pub sighting: Sighting,
    pub photos: Vec<PhotoView>,
    pub identification: Option<IdentificationResultDto>,
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
/// identification status/top species/danger level, photo count, a
/// thumbnail photo id) computed via correlated subqueries against
/// `identification_results` and `photos` in the same query.
async fn list_sightings_for(pool: &SqlitePool, user_id: &str) -> AppResult<Vec<SightingSummary>> {
    let rows = sqlx::query_as::<_, SightingSummaryRow>(
        "SELECT s.id, s.created_at, s.observed_at, s.place_label, s.lat, s.lon, s.status, \
                (SELECT i.status FROM identification_results i \
                   WHERE i.sighting_id = s.id ORDER BY i.created_at DESC LIMIT 1) AS latest_identification_status, \
                (SELECT i.candidates_json FROM identification_results i \
                   WHERE i.sighting_id = s.id ORDER BY i.created_at DESC LIMIT 1) AS latest_identification_candidates_json, \
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

/// Build the full aggregate detail view for a sighting, scoped to `user_id`.
async fn build_sighting_detail(
    pool: &SqlitePool,
    user_id: &str,
    id: &str,
) -> AppResult<SightingDetail> {
    let sighting = owned_sighting(pool, user_id, id).await?;
    let photos = crate::photos::list_for_sighting(pool, id).await?;
    let identification = crate::identification::get_latest(pool, id).await?;
    Ok(SightingDetail {
        sighting,
        photos,
        identification,
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
    fn top_candidate_species_picks_the_first_entry() {
        // Candidates are already confidence-sorted (descending) by
        // `identification::compile_candidates` before being persisted, so
        // the first array entry is the top one.
        let json = r#"[{"species":"Amanita phalloides","confidence":0.9},{"species":"Agaricus bisporus","confidence":0.3}]"#;
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
    fn highest_danger_level_picks_most_severe_across_poisonous_and_confusants() {
        let json = r#"[
            {"species":"A","poisonous":false,"confusants":[{"danger_level":"mild"}]},
            {"species":"B","poisonous":true,"confusants":[{"danger_level":"deadly_toxic"}]}
        ]"#;
        assert_eq!(
            highest_danger_level(Some(json)),
            Some("deadly_toxic".to_string())
        );
    }

    #[test]
    fn highest_danger_level_handles_missing_or_invalid() {
        assert_eq!(highest_danger_level(None), None);
        assert_eq!(highest_danger_level(Some("not json")), None);
        assert_eq!(highest_danger_level(Some("[]")), None);
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

    /// Inserts a minimal `identification_results` row directly (bypassing
    /// the real pipeline, which needs an LLM) so the list/detail aggregates
    /// can be exercised against the real table.
    async fn insert_identification_result(
        pool: &SqlitePool,
        sighting_id: &str,
        status: &str,
        candidates_json: &str,
    ) {
        sqlx::query(
            "INSERT INTO identification_results \
                 (id, sighting_id, created_at, updated_at, status, model, candidates_json, \
                  missing_info_json, photos_considered) \
             VALUES (?, ?, '2026-03-01T10:06:00Z', '2026-03-01T10:06:00Z', ?, 'test-model', ?, \
                      '[]', 1)",
        )
        .bind(Uuid::new_v4().to_string())
        .bind(sighting_id)
        .bind(status)
        .bind(candidates_json)
        .execute(pool)
        .await
        .expect("insert identification_results row");
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
    async fn detail_aggregates_photos_and_identification() {
        let (_dir, pool) = test_pool().await;

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

        // Before any photo/identification: detail still succeeds, with
        // empty photos and no identification.
        let empty_detail = build_sighting_detail(&pool, "u1", &sighting.id)
            .await
            .expect("detail before any photo");
        assert!(empty_detail.photos.is_empty());
        assert!(empty_detail.identification.is_none());

        sqlx::query(
            "INSERT INTO photos (id, sighting_id, file_path, content_type, width, height, \
                 taken_at, sort_order, created_at) \
             VALUES ('p1', ?, 'p1.jpg', 'image/jpeg', 100, 100, '2026-03-01T10:05:00Z', 0, '2026-03-01T10:05:00Z')",
        )
        .bind(&sighting.id)
        .execute(&pool)
        .await
        .expect("insert photo");

        insert_identification_result(
            &pool,
            &sighting.id,
            "complete",
            r#"[{"species":"Amanita phalloides","common_name":"Death cap","confidence":0.6,
                "edible":false,"medicinal":null,"psychoactive":null,"poisonous":true,
                "wikipedia_url":"https://en.wikipedia.org/wiki/Amanita_phalloides",
                "risk_note":"deadly","confusants":[]}]"#,
        )
        .await;

        let detail = build_sighting_detail(&pool, "u1", &sighting.id)
            .await
            .expect("full detail");
        assert_eq!(detail.photos.len(), 1);
        let identification = detail.identification.expect("identification present");
        assert_eq!(
            identification.status,
            crate::identification::IdentificationStatus::Complete
        );
        assert_eq!(identification.candidates.len(), 1);
        assert_eq!(identification.candidates[0].species, "Amanita phalloides");
        assert_eq!(identification.candidates[0].poisonous, Some(true));

        // Another user cannot see it.
        let err = build_sighting_detail(&pool, "someone-else", &sighting.id)
            .await
            .expect_err("must be NotFound for a non-owner");
        assert!(matches!(err, AppError::NotFound));
    }

    /// Regression test for a real bug: `SightingDetail.sighting` was
    /// `#[serde(flatten)]`ed, so the actual JSON response had no nested
    /// "sighting" object at all -- its fields (id, place_label, observed_at,
    /// ...) landed at the top level instead. The frontend's `SightingDetail`
    /// type (and every `detail.sighting.*` template reference) expects a
    /// genuinely nested object, matching docs/ARCHITECTURE.md's documented
    /// contract. The Rust-level assertions in the test above never caught
    /// this because they read `detail.sighting.id` as a struct field, which
    /// works regardless of how it serializes -- only inspecting the actual
    /// serialized JSON shape catches a `#[serde(flatten)]` mistake like this.
    #[tokio::test]
    async fn detail_serializes_sighting_as_a_nested_object_not_flattened() {
        let (_dir, pool) = test_pool().await;

        let sighting = insert_sighting(
            &pool,
            "u1",
            None,
            None,
            None,
            Some("Test Forest"),
            "2026-03-01T10:00:00Z",
            None,
        )
        .await
        .expect("insert sighting");

        let detail = build_sighting_detail(&pool, "u1", &sighting.id)
            .await
            .expect("detail");
        let value = serde_json::to_value(&detail).expect("serialize detail");

        assert_eq!(
            value.get("place_label"),
            None,
            "sighting fields must NOT be flattened onto the top-level response"
        );
        assert_eq!(
            value
                .get("sighting")
                .and_then(|s| s.get("place_label"))
                .and_then(|v| v.as_str()),
            Some("Test Forest"),
            "sighting must be a nested object with its own fields"
        );
        assert!(value.get("photos").is_some());
        assert!(value.get("identification").is_some());
    }
}
