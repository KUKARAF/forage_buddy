//! Shared application state handed to Axum handlers via `axum::extract::State`.

use std::sync::Arc;

use axum::extract::FromRef;
use axum_extra::extract::cookie::Key;
use sqlx::SqlitePool;

use crate::auth::oidc::OidcClient;
use crate::config::Config;
use crate::llm::LlmClient;

/// All interior handles are cheap to clone (Arc-backed pools / clients), so
/// `AppState` itself just derives `Clone` for `axum::extract::State`.
#[derive(Clone)]
pub struct AppState {
    pub db: SqlitePool,
    pub config: Arc<Config>,
    /// `None` if OIDC discovery failed at startup — OIDC routes return a
    /// clear 500 rather than panicking.
    pub oidc: Option<Arc<OidcClient>>,
    /// Signs/encrypts the short-lived OIDC login-flow cookie.
    pub cookie_key: Key,
    /// LLM chat/vision/embeddings client.
    pub llm: LlmClient,
}

impl FromRef<AppState> for Key {
    fn from_ref(state: &AppState) -> Self {
        state.cookie_key.clone()
    }
}
