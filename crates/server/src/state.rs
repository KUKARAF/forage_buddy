//! Shared application state handed to Axum handlers via `axum::extract::State`.

use std::sync::Arc;

use axum::extract::FromRef;
use axum_extra::extract::cookie::Key;
use sqlx::SqlitePool;
use tokio::sync::Semaphore;

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
    /// Plain `reqwest::Client` for small external calls that aren't the LLM
    /// provider (`weather::fetch_last_14_days`,
    /// `identification::fetch_image_as_data_url`) — no auth/provider-switch
    /// plumbing needed, so it doesn't live on `llm`. Built by
    /// [`build_http_client`] — MUST send a `User-Agent`: Wikimedia's CDN
    /// now 403s image requests with none (a real bug this comment exists
    /// to prevent regressing — see `fetch_image_as_data_url`'s call site).
    pub http_client: reqwest::Client,
    /// Serializes `identification::run_identification` runs process-wide.
    /// Uploading several photos for the same sighting in quick succession
    /// (a multi-select) spawns one `photos::on_photo_uploaded` task per
    /// photo; without this, each would race to start its own full
    /// gather-then-compile pipeline (several LLM calls each) against the
    /// same sighting concurrently, multiplying cost and racing writes to
    /// the same `identification_results` row. A single global permit is
    /// enough at this app's scale — identification already runs in the
    /// background, so queuing behind one in-flight run costs nothing the
    /// user can perceive.
    pub identification_semaphore: Arc<Semaphore>,
}

impl FromRef<AppState> for Key {
    fn from_ref(state: &AppState) -> Self {
        state.cookie_key.clone()
    }
}

/// Builds [`AppState::http_client`]: no redirects (SSRF hygiene, same
/// rationale as the OIDC/LLM/Wikipedia clients) and a real `User-Agent` —
/// required since some CDNs (Wikimedia's included, as of a recent policy
/// change) now reject requests that send none at all, rather than just
/// rate-limiting a generic one.
pub fn build_http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .user_agent("forage-buddy/0.1 (safety research aid)")
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}
