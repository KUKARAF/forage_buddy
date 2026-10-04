//! Photo upload + storage for sightings.
//!
//! A sighting can have many photos. Every upload is decoded, downscaled (if
//! either dimension exceeds [`MAX_DIMENSION`]) and re-encoded to JPEG before
//! being written under `Config::photo_dir` as `{sighting_id}/{photo_id}.jpg`
//! — this keeps storage small and vision-model requests fast/cheap, and
//! means the stored `content_type` is always `image/jpeg` regardless of the
//! format the client uploaded.
//!
//! Owns migration `0003_photos.sql` (`photos`).
//!
//! Routes (all require auth, ownership scoped to the caller):
//!   - `POST /api/sightings/{id}/photos` — multipart upload (`photo` field
//!     required; optional `taken_at`, RFC3339)
//!   - `GET  /api/photos/{id}/file` — stream the stored (already-downscaled)
//!     image bytes back, with the correct `Content-Type`
//!
//! Per-photo `lat`/`lon`: the `photos` table (see `docs/ARCHITECTURE.md`) has
//! no column for them, only the sighting-level `lat`/`lon`. Rather than
//! invent an undocumented column, this module's v1 simply does not support a
//! per-shot location override — any `lat`/`lon` multipart fields are
//! accepted (ignored) rather than rejected, so older/newer clients that send
//! them don't break. See the final report for this simplification.

use axum::extract::{Multipart, Path, State};
use axum::http::header;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use image::codecs::jpeg::JpegEncoder;
use image::imageops::FilterType;
use serde::Serialize;
use sqlx::SqlitePool;
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::auth::session::RequireAuth;
use crate::config::Config;
use crate::error::{AppError, AppResult};
use crate::state::AppState;

/// Longest edge (px) a stored photo is downscaled to. Vision models don't
/// need more resolution than this, and it keeps requests fast/cheap.
const MAX_DIMENSION: u32 = 2000;

/// JPEG re-encode quality for stored photos.
const JPEG_QUALITY: u8 = 85;

/// One `photos` row as returned to clients. Never includes the on-disk
/// `file_path` — the frontend fetches bytes via `GET /api/photos/{id}/file`.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct PhotoView {
    pub id: String,
    pub sighting_id: String,
    pub content_type: String,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub taken_at: String,
    pub sort_order: i64,
    pub created_at: String,
}

const PHOTO_COLUMNS: &str =
    "id, sighting_id, content_type, width, height, taken_at, sort_order, created_at";

/// List a sighting's photos, ordered by `sort_order`. Does not itself check
/// ownership — callers (e.g. `sightings::build_sighting_detail`) are
/// expected to have already verified the sighting belongs to the caller.
pub(crate) async fn list_for_sighting(
    pool: &SqlitePool,
    sighting_id: &str,
) -> AppResult<Vec<PhotoView>> {
    sqlx::query_as::<_, PhotoView>(&format!(
        "SELECT {PHOTO_COLUMNS} FROM photos WHERE sighting_id = ? ORDER BY sort_order ASC"
    ))
    .bind(sighting_id)
    .fetch_all(pool)
    .await
    .map_err(|e| AppError::Internal(e.into()))
}

async fn fetch_photo(pool: &SqlitePool, id: &str) -> AppResult<Option<PhotoView>> {
    sqlx::query_as::<_, PhotoView>(&format!("SELECT {PHOTO_COLUMNS} FROM photos WHERE id = ?"))
        .bind(id)
        .fetch_optional(pool)
        .await
        .map_err(|e| AppError::Internal(e.into()))
}

/// Verify `sighting_id` exists and belongs to `user_id`; otherwise
/// `NotFound` (a missing sighting and another user's sighting must be
/// indistinguishable to the caller, to avoid leaking existence).
async fn verify_sighting_owner(
    pool: &SqlitePool,
    user_id: &str,
    sighting_id: &str,
) -> AppResult<()> {
    let owner: Option<(String,)> = sqlx::query_as("SELECT user_id FROM sightings WHERE id = ?")
        .bind(sighting_id)
        .fetch_optional(pool)
        .await
        .map_err(|e| AppError::Internal(e.into()))?;
    match owner {
        Some((o,)) if o == user_id => Ok(()),
        _ => Err(AppError::NotFound),
    }
}

fn now_rfc3339() -> AppResult<String> {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .map_err(|e| AppError::Internal(e.into()))
}

/// Compute the downscaled `(width, height)` for an image whose original
/// dimensions are `(w, h)`, so that neither side exceeds `max`. Returns the
/// original dimensions unchanged if already within bounds. Pure (no image
/// decode), so it's cheap to unit-test independent of the `image` crate.
fn target_dimensions(w: u32, h: u32, max: u32) -> (u32, u32) {
    if w <= max && h <= max {
        return (w, h);
    }
    let longest = w.max(h);
    if longest == 0 {
        return (w, h);
    }
    let scale = f64::from(max) / f64::from(longest);
    let new_w = ((f64::from(w) * scale).round() as u32).max(1);
    let new_h = ((f64::from(h) * scale).round() as u32).max(1);
    (new_w, new_h)
}

/// Decode `bytes` as an image, downscale if either dimension exceeds
/// [`MAX_DIMENSION`], and re-encode as JPEG at [`JPEG_QUALITY`]. Returns the
/// final JPEG bytes and its (possibly downscaled) `(width, height)`.
fn process_image(bytes: &[u8]) -> AppResult<(Vec<u8>, u32, u32)> {
    let img = image::load_from_memory(bytes)
        .map_err(|e| AppError::BadRequest(format!("not a decodable image: {e}")))?;

    let (w, h) = (img.width(), img.height());
    let (target_w, target_h) = target_dimensions(w, h, MAX_DIMENSION);
    let img = if (target_w, target_h) != (w, h) {
        img.resize(target_w, target_h, FilterType::Lanczos3)
    } else {
        img
    };

    let mut out = Vec::new();
    {
        let encoder = JpegEncoder::new_with_quality(&mut out, JPEG_QUALITY);
        img.write_with_encoder(encoder)
            .map_err(|e| AppError::Internal(e.into()))?;
    }
    Ok((out, img.width(), img.height()))
}

/// Full path on disk for a stored photo.
fn photo_path(config: &Config, sighting_id: &str, photo_id: &str) -> std::path::PathBuf {
    std::path::Path::new(&config.photo_dir)
        .join(sighting_id)
        .join(format!("{photo_id}.jpg"))
}

/// Path stored in the `photos.file_path` column, relative to
/// `Config::photo_dir`.
fn relative_photo_path(sighting_id: &str, photo_id: &str) -> String {
    format!("{sighting_id}/{photo_id}.jpg")
}

/// Persist a freshly-uploaded photo: writes the (already processed) JPEG
/// bytes to disk and inserts the `photos` row. `sort_order` is the count of
/// photos already attached to the sighting. Returns the created row.
async fn store_photo(
    pool: &SqlitePool,
    config: &Config,
    sighting_id: &str,
    jpeg_bytes: &[u8],
    width: u32,
    height: u32,
    taken_at: &str,
) -> AppResult<PhotoView> {
    let photo_id = Uuid::new_v4().to_string();
    let now = now_rfc3339()?;

    let on_disk = photo_path(config, sighting_id, &photo_id);
    if let Some(parent) = on_disk.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| AppError::Internal(e.into()))?;
    }
    tokio::fs::write(&on_disk, jpeg_bytes)
        .await
        .map_err(|e| AppError::Internal(e.into()))?;

    let (count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM photos WHERE sighting_id = ?")
        .bind(sighting_id)
        .fetch_one(pool)
        .await
        .map_err(|e| AppError::Internal(e.into()))?;

    let relative = relative_photo_path(sighting_id, &photo_id);
    sqlx::query(
        "INSERT INTO photos \
             (id, sighting_id, file_path, content_type, width, height, taken_at, sort_order, created_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&photo_id)
    .bind(sighting_id)
    .bind(&relative)
    .bind("image/jpeg")
    .bind(i64::from(width))
    .bind(i64::from(height))
    .bind(taken_at)
    .bind(count)
    .bind(&now)
    .execute(pool)
    .await
    .map_err(|e| AppError::Internal(e.into()))?;

    fetch_photo(pool, &photo_id)
        .await?
        .ok_or(AppError::NotFound)
}

/// Load a photo's bytes from disk, scoped to `user_id` via its parent
/// sighting. Returns `NotFound` when the photo is missing or owned by
/// another user (ownership and existence are indistinguishable to the
/// caller).
async fn load_photo_file(
    pool: &SqlitePool,
    config: &Config,
    user_id: &str,
    photo_id: &str,
) -> AppResult<(String, Vec<u8>)> {
    let row: Option<(String, String, String)> = sqlx::query_as(
        "SELECT p.file_path, p.content_type, s.user_id \
         FROM photos p JOIN sightings s ON p.sighting_id = s.id \
         WHERE p.id = ?",
    )
    .bind(photo_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| AppError::Internal(e.into()))?;

    let (file_path, content_type) = match row {
        Some((file_path, content_type, owner)) if owner == user_id => (file_path, content_type),
        _ => return Err(AppError::NotFound),
    };

    let full_path = std::path::Path::new(&config.photo_dir).join(file_path);
    let bytes = tokio::fs::read(&full_path)
        .await
        .map_err(|e| AppError::Internal(e.into()))?;
    Ok((content_type, bytes))
}

/// Called by the upload handler after the file is persisted and the row
/// inserted. Fire-and-forget from the HTTP handler's point of view: spawned,
/// logs failures, never fails the upload response on a triage error.
pub async fn on_photo_uploaded(state: AppState, sighting_id: String) {
    if let Err(e) = crate::triage::run_triage(&state, &sighting_id).await {
        tracing::error!(error = ?e, sighting_id, "triage run failed after photo upload");
    }
}

/// `POST /api/sightings/{id}/photos` — multipart upload. Field `photo` is
/// the image file (required); optional field `taken_at` (RFC3339, defaults
/// to now) overrides the capture time. `lat`/`lon` fields are accepted but
/// ignored (see module docs: the schema has no per-photo location column).
async fn upload_photo(
    State(state): State<AppState>,
    RequireAuth(user_id): RequireAuth,
    Path(sighting_id): Path<String>,
    mut multipart: Multipart,
) -> AppResult<Json<PhotoView>> {
    verify_sighting_owner(&state.db, &user_id, &sighting_id).await?;

    let mut photo_bytes: Option<Vec<u8>> = None;
    let mut taken_at: Option<String> = None;

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::BadRequest(format!("invalid multipart body: {e}")))?
    {
        let name = field.name().unwrap_or("").to_string();
        match name.as_str() {
            "photo" => {
                let bytes = field.bytes().await.map_err(|e| {
                    AppError::BadRequest(format!("failed to read photo field: {e}"))
                })?;
                if bytes.len() > state.config.max_photo_bytes {
                    return Err(AppError::PayloadTooLarge);
                }
                photo_bytes = Some(bytes.to_vec());
            }
            "taken_at" => {
                let text = field
                    .text()
                    .await
                    .map_err(|e| AppError::BadRequest(format!("invalid taken_at field: {e}")))?;
                let text = text.trim();
                if !text.is_empty() {
                    taken_at = Some(text.to_string());
                }
            }
            // `lat`/`lon` (and anything else): accepted but ignored — see
            // module docs on the per-photo-location simplification.
            _ => {
                let _ = field.bytes().await;
            }
        }
    }

    let photo_bytes = photo_bytes
        .ok_or_else(|| AppError::BadRequest("multipart field \"photo\" is required".to_string()))?;
    if photo_bytes.is_empty() {
        return Err(AppError::BadRequest("empty photo upload".to_string()));
    }

    let taken_at = match taken_at {
        Some(raw) => {
            OffsetDateTime::parse(&raw, &Rfc3339)
                .map_err(|_| AppError::BadRequest("taken_at must be RFC3339".to_string()))?;
            raw
        }
        None => now_rfc3339()?,
    };

    let (jpeg_bytes, width, height) = process_image(&photo_bytes)?;

    let created = store_photo(
        &state.db,
        &state.config,
        &sighting_id,
        &jpeg_bytes,
        width,
        height,
        &taken_at,
    )
    .await?;

    // Fire-and-forget: the HTTP response returns as soon as the file + row
    // are saved, the (possibly slow) LLM triage call runs in the background.
    tokio::spawn(on_photo_uploaded(state.clone(), sighting_id));

    Ok(Json(created))
}

/// `GET /api/photos/{id}/file` — stream the stored (already-downscaled)
/// image back, with the correct `Content-Type`.
async fn get_photo_file(
    State(state): State<AppState>,
    RequireAuth(user_id): RequireAuth,
    Path(photo_id): Path<String>,
) -> AppResult<Response> {
    let (content_type, bytes) =
        load_photo_file(&state.db, &state.config, &user_id, &photo_id).await?;
    let headers = [(header::CONTENT_TYPE, content_type)];
    Ok((headers, bytes).into_response())
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/sightings/{id}/photos", post(upload_photo))
        .route("/api/photos/{id}/file", get(get_photo_file))
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- pure helper tests ---------------------------------------------------

    #[test]
    fn dimensions_unchanged_when_within_bounds() {
        assert_eq!(target_dimensions(800, 600, 2000), (800, 600));
        assert_eq!(target_dimensions(2000, 2000, 2000), (2000, 2000));
    }

    #[test]
    fn dimensions_downscaled_preserving_aspect_ratio() {
        let (w, h) = target_dimensions(4000, 3000, 2000);
        assert_eq!(w, 2000);
        assert_eq!(h, 1500);

        // Portrait orientation: height is the long side.
        let (w, h) = target_dimensions(3000, 6000, 2000);
        assert_eq!(h, 2000);
        assert_eq!(w, 1000);
    }

    #[test]
    fn dimensions_handle_zero_gracefully() {
        // Degenerate input must never panic or divide by zero.
        assert_eq!(target_dimensions(0, 0, 2000), (0, 0));
    }

    #[test]
    fn relative_path_matches_sighting_and_photo_id() {
        assert_eq!(
            relative_photo_path("sight-1", "photo-1"),
            "sight-1/photo-1.jpg"
        );
    }

    /// A minimal valid 2x2 PNG (red pixels), used to exercise the real
    /// decode/encode path without a fixture file.
    fn tiny_png() -> Vec<u8> {
        let img = image::RgbImage::from_pixel(2, 2, image::Rgb([255, 0, 0]));
        let dynamic = image::DynamicImage::ImageRgb8(img);
        let mut out = Vec::new();
        dynamic
            .write_with_encoder(image::codecs::png::PngEncoder::new(&mut out))
            .expect("encode tiny test png");
        out
    }

    #[test]
    fn process_image_decodes_and_reencodes_as_jpeg() {
        let png = tiny_png();
        let (jpeg_bytes, w, h) = process_image(&png).expect("process tiny image");
        assert_eq!((w, h), (2, 2));
        // JPEG magic bytes.
        assert!(jpeg_bytes.starts_with(&[0xFF, 0xD8]));
    }

    #[test]
    fn process_image_rejects_garbage_bytes() {
        let err = process_image(b"not an image").expect_err("garbage must be rejected");
        assert!(matches!(err, AppError::BadRequest(_)));
    }

    // ---- DB-backed tests ------------------------------------------------------

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

    async fn insert_sighting(pool: &SqlitePool, id: &str, user_id: &str) {
        sqlx::query(
            "INSERT INTO sightings \
                 (id, user_id, status, observed_at, created_at, updated_at) \
             VALUES (?, ?, 'open', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
        )
        .bind(id)
        .bind(user_id)
        .execute(pool)
        .await
        .expect("insert sighting");
    }

    fn test_config(dir: &tempfile::TempDir) -> Config {
        let mut config = Config::from_env();
        config.photo_dir = dir
            .path()
            .join("photos")
            .to_str()
            .expect("utf8 path")
            .to_string();
        config
    }

    #[tokio::test]
    async fn storing_a_photo_writes_file_and_inserts_row() {
        let (dir, pool) = test_pool().await;
        insert_sighting(&pool, "s1", "u1").await;
        let config = test_config(&dir);

        let (jpeg_bytes, w, h) = process_image(&tiny_png()).expect("process");
        let created = store_photo(
            &pool,
            &config,
            "s1",
            &jpeg_bytes,
            w,
            h,
            "2026-02-01T00:00:00Z",
        )
        .await
        .expect("store photo");

        assert_eq!(created.sighting_id, "s1");
        assert_eq!(created.sort_order, 0);
        assert_eq!(created.content_type, "image/jpeg");

        let on_disk = photo_path(&config, "s1", &created.id);
        assert!(on_disk.exists(), "jpeg file should be written to disk");

        // A second photo for the same sighting gets the next sort_order.
        let created2 = store_photo(
            &pool,
            &config,
            "s1",
            &jpeg_bytes,
            w,
            h,
            "2026-02-02T00:00:00Z",
        )
        .await
        .expect("store second photo");
        assert_eq!(created2.sort_order, 1);

        let listed = list_for_sighting(&pool, "s1").await.expect("list");
        assert_eq!(listed.len(), 2);
        assert_eq!(listed.first().map(|p| p.sort_order), Some(0));
        assert_eq!(listed.get(1).map(|p| p.sort_order), Some(1));
    }

    #[tokio::test]
    async fn loading_another_users_photo_is_not_found() {
        let (dir, pool) = test_pool().await;
        sqlx::query(
            "INSERT INTO users (id, email, display_name, created_at) \
             VALUES ('u2', NULL, NULL, '2026-01-01T00:00:00Z')",
        )
        .execute(&pool)
        .await
        .expect("insert u2");
        insert_sighting(&pool, "s1", "u2").await;
        let config = test_config(&dir);

        let (jpeg_bytes, w, h) = process_image(&tiny_png()).expect("process");
        let created = store_photo(
            &pool,
            &config,
            "s1",
            &jpeg_bytes,
            w,
            h,
            "2026-02-01T00:00:00Z",
        )
        .await
        .expect("store photo");

        let err = load_photo_file(&pool, &config, "u1", &created.id)
            .await
            .expect_err("another user's photo must be NotFound");
        assert!(matches!(err, AppError::NotFound));

        let (content_type, bytes) = load_photo_file(&pool, &config, "u2", &created.id)
            .await
            .expect("owner can load their own photo");
        assert_eq!(content_type, "image/jpeg");
        assert!(!bytes.is_empty());
    }

    #[tokio::test]
    async fn verify_sighting_owner_rejects_missing_and_other_users() {
        let (_dir, pool) = test_pool().await;
        sqlx::query(
            "INSERT INTO users (id, email, display_name, created_at) \
             VALUES ('owner', NULL, NULL, '2026-01-01T00:00:00Z')",
        )
        .execute(&pool)
        .await
        .expect("insert owner");
        insert_sighting(&pool, "s1", "owner").await;

        let err = verify_sighting_owner(&pool, "u1", "s1")
            .await
            .expect_err("another user's sighting must be rejected");
        assert!(matches!(err, AppError::NotFound));

        let missing = verify_sighting_owner(&pool, "u1", "does-not-exist")
            .await
            .expect_err("missing sighting must be rejected");
        assert!(matches!(missing, AppError::NotFound));

        verify_sighting_owner(&pool, "owner", "s1")
            .await
            .expect("owner is accepted");
    }
}
