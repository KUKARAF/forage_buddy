//! Device-token store: long-lived bearer credentials for the mobile app.
//!
//! The Tauri Android app's webview origin is cross-site to this server, so
//! the `SameSite=Lax` session cookie is never sent. Instead the app completes
//! the OIDC login in the system browser (`?client=app`), receives a device
//! token via a custom-scheme deep link (`dev.foragebuddy.app://auth?token=<raw>`),
//! and sends `Authorization: Bearer <token>` on every request.
//!
//! Only `base64url(SHA-256(raw))` is stored; lookup is by hash so B-tree
//! timing reveals nothing. Sliding 90-day expiry (touch throttled to 1h);
//! logout revokes; at most [`MAX_TOKENS_PER_USER`] live tokens per user.

use axum::http::HeaderMap;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

use crate::error::{AppError, AppResult};

const TOKEN_TTL: time::Duration = time::Duration::days(90);
const TOUCH_THROTTLE: time::Duration = time::Duration::hours(1);
const MAX_TOKENS_PER_USER: u32 = 10;

fn hash_token(raw: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(raw.as_bytes()))
}

fn format_rfc3339(t: OffsetDateTime) -> AppResult<String> {
    t.format(&Rfc3339).map_err(|e| AppError::Internal(e.into()))
}

/// Mint a new device token for `user_id`, returning the raw token (the only
/// time it exists server-side). Evicts the user's oldest beyond the cap.
pub async fn create(db: &SqlitePool, user_id: &str, label: &str) -> AppResult<String> {
    let raw = forage_buddy_core::device_token::generate();
    let now = OffsetDateTime::now_utc();
    let now_s = format_rfc3339(now)?;
    let expires_s = format_rfc3339(now + TOKEN_TTL)?;

    sqlx::query(
        "INSERT INTO device_tokens (token_hash, user_id, label, created_at, last_used_at, expires_at) \
         VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(hash_token(&raw))
    .bind(user_id)
    .bind(label)
    .bind(&now_s)
    .bind(&now_s)
    .bind(&expires_s)
    .execute(db)
    .await
    .map_err(|e| AppError::Internal(e.into()))?;

    sqlx::query(
        "DELETE FROM device_tokens WHERE user_id = ? AND token_hash NOT IN ( \
             SELECT token_hash FROM device_tokens WHERE user_id = ? \
             ORDER BY created_at DESC, token_hash DESC LIMIT ? \
         )",
    )
    .bind(user_id)
    .bind(user_id)
    .bind(MAX_TOKENS_PER_USER)
    .execute(db)
    .await
    .map_err(|e| AppError::Internal(e.into()))?;

    Ok(raw)
}

/// Resolve a raw bearer token to its user id, or `None` if unknown/expired
/// (malformed timestamps fail closed). Slides expiry when stale.
pub async fn resolve(db: &SqlitePool, raw: &str) -> AppResult<Option<String>> {
    let hash = hash_token(raw);
    let row: Option<(String, String, String)> = sqlx::query_as(
        "SELECT user_id, last_used_at, expires_at FROM device_tokens WHERE token_hash = ?",
    )
    .bind(&hash)
    .fetch_optional(db)
    .await
    .map_err(|e| AppError::Internal(e.into()))?;

    let Some((user_id, last_used_at, expires_at)) = row else {
        return Ok(None);
    };

    let now = OffsetDateTime::now_utc();
    match OffsetDateTime::parse(&expires_at, &Rfc3339) {
        Ok(expires) if now < expires => {}
        _ => return Ok(None),
    }

    let needs_touch = match OffsetDateTime::parse(&last_used_at, &Rfc3339) {
        Ok(last_used) => now - last_used > TOUCH_THROTTLE,
        Err(_) => true,
    };
    if needs_touch {
        let now_s = format_rfc3339(now)?;
        let expires_s = format_rfc3339(now + TOKEN_TTL)?;
        sqlx::query(
            "UPDATE device_tokens SET last_used_at = ?, expires_at = ? WHERE token_hash = ?",
        )
        .bind(&now_s)
        .bind(&expires_s)
        .bind(&hash)
        .execute(db)
        .await
        .map_err(|e| AppError::Internal(e.into()))?;
    }

    Ok(Some(user_id))
}

/// Delete the token (by raw value). No-op if it doesn't exist.
pub async fn revoke(db: &SqlitePool, raw: &str) -> AppResult<()> {
    sqlx::query("DELETE FROM device_tokens WHERE token_hash = ?")
        .bind(hash_token(raw))
        .execute(db)
        .await
        .map_err(|e| AppError::Internal(e.into()))?;
    Ok(())
}

/// Extract the raw token from an `Authorization: Bearer <token>` header.
pub fn bearer_from_headers(headers: &HeaderMap) -> Option<String> {
    let value = headers
        .get(axum::http::header::AUTHORIZATION)?
        .to_str()
        .ok()?;
    let (scheme, token) = value.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("bearer") {
        return None;
    }
    let token = token.trim();
    if token.is_empty() {
        return None;
    }
    Some(token.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    async fn test_pool() -> (tempfile::TempDir, SqlitePool) {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let pool = crate::db::init_pool(db_path.to_str().unwrap())
            .await
            .unwrap();
        sqlx::query("INSERT INTO users (id, email, display_name, created_at) VALUES ('u1', NULL, NULL, '2026-01-01T00:00:00Z')")
            .execute(&pool)
            .await
            .unwrap();
        (dir, pool)
    }

    #[tokio::test]
    async fn create_resolve_roundtrip() {
        let (_dir, pool) = test_pool().await;
        let raw = create(&pool, "u1", "android-app").await.unwrap();
        assert_eq!(resolve(&pool, &raw).await.unwrap(), Some("u1".to_string()));
        assert_eq!(resolve(&pool, "not-a-real-token").await.unwrap(), None);
    }

    #[tokio::test]
    async fn expired_token_is_rejected() {
        let (_dir, pool) = test_pool().await;
        let raw = create(&pool, "u1", "android-app").await.unwrap();
        sqlx::query("UPDATE device_tokens SET expires_at = '2000-01-01T00:00:00Z'")
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(resolve(&pool, &raw).await.unwrap(), None);
    }

    #[tokio::test]
    async fn revoke_deletes_token() {
        let (_dir, pool) = test_pool().await;
        let raw = create(&pool, "u1", "android-app").await.unwrap();
        revoke(&pool, &raw).await.unwrap();
        assert_eq!(resolve(&pool, &raw).await.unwrap(), None);
        revoke(&pool, &raw).await.unwrap();
    }

    #[test]
    fn bearer_header_parsing() {
        let mut headers = HeaderMap::new();
        assert_eq!(bearer_from_headers(&headers), None);
        headers.insert(
            axum::http::header::AUTHORIZATION,
            HeaderValue::from_static("Bearer abc123"),
        );
        assert_eq!(bearer_from_headers(&headers), Some("abc123".to_string()));
        headers.insert(
            axum::http::header::AUTHORIZATION,
            HeaderValue::from_static("bearer abc123"),
        );
        assert_eq!(bearer_from_headers(&headers), Some("abc123".to_string()));
        headers.insert(
            axum::http::header::AUTHORIZATION,
            HeaderValue::from_static("Basic abc123"),
        );
        assert_eq!(bearer_from_headers(&headers), None);
    }
}
