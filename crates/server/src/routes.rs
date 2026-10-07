//! Top-level route table.

use std::time::Duration;

use axum::extract::DefaultBodyLimit;
use axum::http::StatusCode;
use axum::routing::get;
use axum::Router;
use tower::limit::GlobalConcurrencyLimitLayer;
use tower_http::services::{ServeDir, ServeFile};
use tower_http::timeout::TimeoutLayer;

use crate::auth;
use crate::identification;
use crate::photos;
use crate::sightings;
use crate::state::AppState;
use crate::weather;

/// Headroom added on top of `Config::max_photo_bytes` for multipart framing
/// overhead (field boundaries, headers) when a photo upload is the request.
const BODY_LIMIT_HEADROOM_BYTES: usize = 1024 * 1024;

/// Per-request wall-clock timeout. The identification pipeline calls out to
/// an LLM provider (and Wikipedia) multiple times and can legitimately take
/// tens of seconds, so this is generous.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);

/// Ceiling on concurrently-processed REST requests.
const MAX_CONCURRENT_REQUESTS: usize = 256;

/// Build the full application router. The session + CORS layers are applied
/// by the caller in `main.rs`.
pub fn build(state: AppState) -> Router {
    let max_body_bytes = state.config.max_photo_bytes + BODY_LIMIT_HEADROOM_BYTES;

    let api = Router::new()
        .route("/health", get(|| async { "ok" }))
        .merge(auth::oidc::router())
        .merge(sightings::router())
        .merge(photos::router())
        .merge(identification::router())
        .merge(weather::router())
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            REQUEST_TIMEOUT,
        ))
        .layer(DefaultBodyLimit::max(max_body_bytes))
        .layer(GlobalConcurrencyLimitLayer::new(MAX_CONCURRENT_REQUESTS));

    let app = api.with_state(state.clone());

    // Optionally serve the built SvelteKit static assets so frontend + backend
    // run as one origin. Any non-API path falls through to ServeDir, which
    // falls back to `200.html` (the SPA shell) for client-side routing.
    match state.config.static_dir.as_deref() {
        Some(dir) => {
            let spa_fallback = ServeFile::new(format!("{dir}/200.html"));
            let serve_dir = ServeDir::new(dir).not_found_service(spa_fallback);
            app.fallback_service(serve_dir)
        }
        None => app,
    }
}
