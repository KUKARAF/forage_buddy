//! Session management (backed by `tower-sessions` + `tower-sessions-sqlx-store`).
//!
//! Only the authenticated user's `id` (the OIDC `sub`) is stored in the
//! session; everything else is looked up from `users` on demand.

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use tower_sessions::Session;

use crate::error::AppError;
use crate::state::AppState;

const USER_ID_KEY: &str = "user_id";

/// Record `user_id` as the authenticated identity (cycling the session id to
/// defend against fixation).
pub async fn login(session: &Session, user_id: &str) -> Result<(), AppError> {
    session
        .cycle_id()
        .await
        .map_err(|e| AppError::Internal(e.into()))?;
    session
        .insert(USER_ID_KEY, user_id)
        .await
        .map_err(|e| AppError::Internal(e.into()))?;
    Ok(())
}

/// Destroy the session entirely.
pub async fn logout(session: &Session) -> Result<(), AppError> {
    session
        .flush()
        .await
        .map_err(|e| AppError::Internal(e.into()))?;
    Ok(())
}

/// The authenticated user's id for this session, if any.
pub async fn current_user_id(session: &Session) -> Option<String> {
    session.get::<String>(USER_ID_KEY).await.ok().flatten()
}

/// Axum extractor requiring an authenticated request.
///
/// Resolution order: dev mode → `Authorization: Bearer` device token (a
/// present-but-invalid bearer is a hard 401, no session fallback) → session
/// cookie.
pub struct RequireAuth(pub String);

impl FromRequestParts<AppState> for RequireAuth {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        if state.config.dev_mode {
            return Ok(RequireAuth(crate::config::DEV_MODE_USER_ID.to_string()));
        }

        if let Some(token) = super::device_token::bearer_from_headers(&parts.headers) {
            return match super::device_token::resolve(&state.db, &token).await? {
                Some(user_id) => Ok(RequireAuth(user_id)),
                None => Err(AppError::Unauthorized),
            };
        }

        let session = Session::from_request_parts(parts, state)
            .await
            .map_err(|_| AppError::Unauthorized)?;

        match current_user_id(&session).await {
            Some(user_id) => Ok(RequireAuth(user_id)),
            None => Err(AppError::Unauthorized),
        }
    }
}
