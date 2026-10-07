mod auth;
mod config;
mod db;
mod error;
mod identification;
mod llm;
mod photos;
mod routes;
mod security;
mod sightings;
mod species;
mod state;
mod vector;
mod weather;
mod wikipedia;

use std::sync::Arc;

use auth::oidc::OidcClient;
use axum::http::{HeaderValue, Method};
use axum_extra::extract::cookie::Key;
use config::Config;
use state::AppState;
use tower_http::cors::CorsLayer;
use tower_sessions::cookie::time::Duration as CookieDuration;
use tower_sessions::{Expiry, SessionManagerLayer};
use tower_sessions_sqlx_store::SqliteStore;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Bare `fmt::init()` doesn't reliably default to showing anything useful
    // across tracing-subscriber versions when `RUST_LOG` is unset — which is
    // exactly how a production deployment went completely silent (not even
    // the startup line below ever printed) while nothing was actually wrong
    // with the process. Explicit default: honor RUST_LOG when set, otherwise
    // "info" so normal operation is always visible without extra config.
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let config = Config::from_env();
    tracing::info!(bind_addr = %config.bind_addr, "starting forage_buddy server");

    // Fail fast on insecure/misconfigured deployments.
    config.validate()?;

    let signing_key_bytes = config.signing_key_bytes()?;
    let cookie_key = Key::derive_from(&signing_key_bytes);

    let pool = db::init_pool(&config.sqlite_path).await?;
    std::fs::create_dir_all(&config.photo_dir)?;

    if config.dev_mode {
        tracing::warn!(
            "DEV MODE ENABLED: authentication is bypassed, every request is user \"{}\". \
             Do not expose this server while dev mode is on.",
            config::DEV_MODE_USER_ID
        );
        sqlx::query(
            "INSERT INTO users (id, email, display_name, created_at) \
             VALUES (?, ?, ?, datetime('now')) ON CONFLICT(id) DO NOTHING",
        )
        .bind(config::DEV_MODE_USER_ID)
        .bind("admin@localhost")
        .bind("Admin")
        .execute(&pool)
        .await?;
    }

    // OIDC discovery is non-fatal: on failure we boot anyway with `oidc: None`
    // so `/health` and non-auth routes stay up.
    let oidc = if config.dev_mode {
        tracing::info!("dev mode: skipping OIDC discovery entirely");
        None
    } else {
        match OidcClient::discover(&config).await {
            Ok(client) => {
                tracing::info!("OIDC discovery succeeded");
                Some(Arc::new(client))
            }
            Err(err) => {
                tracing::error!(
                    error = %err,
                    "OIDC discovery failed at startup; /auth/login and /auth/callback will return \
                     500 until this is fixed and the server is restarted"
                );
                None
            }
        }
    };

    let session_store = SqliteStore::new(pool.clone());
    session_store.migrate().await?;
    let session_layer = SessionManagerLayer::new(session_store)
        .with_name("foragebuddy_session")
        .with_secure(config.cookie_secure())
        .with_same_site(tower_sessions::cookie::SameSite::Lax)
        .with_expiry(Expiry::OnInactivity(CookieDuration::days(14)));

    let cors_origins: Vec<HeaderValue> = config
        .cors_allowed_origins
        .iter()
        .filter_map(|o| HeaderValue::from_str(o).ok())
        .collect();
    let cors = CorsLayer::new()
        .allow_origin(cors_origins)
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PUT,
            Method::PATCH,
            Method::DELETE,
            Method::OPTIONS,
        ])
        .allow_headers(tower_http::cors::AllowHeaders::mirror_request())
        .allow_credentials(true);

    let llm = llm::LlmClient::new(&config);

    let state = AppState {
        db: pool,
        config: Arc::new(config.clone()),
        oidc,
        cookie_key,
        llm,
        http_client: reqwest::Client::new(),
        identification_semaphore: Arc::new(tokio::sync::Semaphore::new(1)),
    };

    // Outermost layer: defense-in-depth security headers (CSP, nosniff, etc.)
    // on EVERY response — REST API and the static SPA fallback alike.
    let app = routes::build(state)
        .layer(session_layer)
        .layer(cors)
        .layer(axum::middleware::from_fn(security::set_security_headers));

    let listener = tokio::net::TcpListener::bind(&config.bind_addr).await?;
    tracing::info!("listening on {}", listener.local_addr()?);
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    Ok(())
}

/// Resolves when the process receives Ctrl-C or (on Unix) SIGTERM.
async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut sig) => {
                sig.recv().await;
            }
            Err(err) => {
                tracing::error!(error = %err, "failed to install SIGTERM handler");
                std::future::pending::<()>().await;
            }
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {}
        _ = terminate => {}
    }

    tracing::info!("shutdown signal received");
}
