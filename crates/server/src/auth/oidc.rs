//! OIDC (Authentik) login flow: authorization redirect + callback handling.
//!
//! Discovery happens once at startup (see [`OidcClient::discover`], invoked
//! from `main.rs`) and is stored in `AppState` behind an `Option` —
//! discovery failure is non-fatal so `/health` stays up.

use std::time::Duration;

use axum::extract::{Query, State};
use axum::response::Redirect;
use axum::routing::{get, post};
use axum::{Json, Router};
use axum_extra::extract::cookie::{Cookie, PrivateCookieJar};
use openidconnect::core::{CoreAuthenticationFlow, CoreClient, CoreProviderMetadata};
use openidconnect::{
    AuthorizationCode, ClientId, ClientSecret, CsrfToken, EndpointMaybeSet, EndpointNotSet,
    EndpointSet, IssuerUrl, Nonce, OAuth2TokenResponse, PkceCodeChallenge, PkceCodeVerifier,
    RedirectUrl, Scope, TokenResponse,
};
use serde::{Deserialize, Serialize};

use crate::auth::session;
use crate::config::Config;
use crate::error::{AppError, AppResult};
use crate::state::AppState;

/// The concrete `CoreClient` type once built from discovered provider metadata.
pub type ConfiguredCoreClient = CoreClient<
    EndpointSet,
    EndpointNotSet,
    EndpointNotSet,
    EndpointNotSet,
    EndpointMaybeSet,
    EndpointMaybeSet,
>;

/// A ready-to-use OIDC client plus the async HTTP client it talks through.
pub struct OidcClient {
    pub client: ConfiguredCoreClient,
    pub http_client: openidconnect::reqwest::Client,
}

impl OidcClient {
    /// Perform OIDC discovery and build a configured `CoreClient` (retried a
    /// few times with backoff, since the provider may not be up yet at
    /// startup).
    pub async fn discover(config: &Config) -> anyhow::Result<Self> {
        let http_client = openidconnect::reqwest::ClientBuilder::new()
            .redirect(openidconnect::reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(30))
            .build()?;

        let issuer_url = IssuerUrl::new(config.authentik_issuer_url.clone())?;

        const MAX_ATTEMPTS: u32 = 3;
        let mut last_err = None;
        for attempt in 1..=MAX_ATTEMPTS {
            match CoreProviderMetadata::discover_async(issuer_url.clone(), &http_client).await {
                Ok(metadata) => {
                    let client_secret = config.oidc_client_secret.clone().map(ClientSecret::new);
                    let client = CoreClient::from_provider_metadata(
                        metadata,
                        ClientId::new(config.oidc_client_id.clone()),
                        client_secret,
                    )
                    .set_redirect_uri(RedirectUrl::new(config.oidc_redirect_uri.clone())?);

                    return Ok(Self {
                        client,
                        http_client,
                    });
                }
                Err(err) => {
                    tracing::warn!(
                        attempt,
                        max_attempts = MAX_ATTEMPTS,
                        error = %err,
                        issuer = %config.authentik_issuer_url,
                        "OIDC discovery attempt failed"
                    );
                    last_err = Some(err);
                    if attempt < MAX_ATTEMPTS {
                        tokio::time::sleep(Duration::from_secs(2u64.pow(attempt - 1))).await;
                    }
                }
            }
        }

        Err(match last_err {
            Some(err) => anyhow::anyhow!(
                "OIDC discovery against {} failed after {MAX_ATTEMPTS} attempts: {}",
                config.authentik_issuer_url,
                err
            ),
            None => anyhow::anyhow!(
                "OIDC discovery against {} failed after {MAX_ATTEMPTS} attempts (retry-loop bug)",
                config.authentik_issuer_url
            ),
        })
    }
}

const FLOW_COOKIE_NAME: &str = "foragebuddy_oidc_flow";
const FLOW_COOKIE_MAX_AGE: time::Duration = time::Duration::minutes(5);

/// Custom-scheme deep link the Android app registers to receive its device token.
const APP_REDIRECT_ORIGIN: &str = "dev.foragebuddy.app://auth";

#[derive(Debug, Serialize, Deserialize)]
struct FlowState {
    pkce_verifier: String,
    csrf_state: String,
    nonce: String,
    #[serde(default)]
    client: Option<String>,
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/auth/login", get(login))
        .route("/auth/callback", get(callback))
        .route("/auth/logout", post(logout))
        .route("/auth/me", get(me))
}

#[derive(Debug, Deserialize)]
struct LoginParams {
    client: Option<String>,
}

async fn login(
    State(state): State<AppState>,
    Query(params): Query<LoginParams>,
    jar: PrivateCookieJar,
) -> AppResult<impl axum::response::IntoResponse> {
    let Some(oidc) = state.oidc.as_ref() else {
        return Err(AppError::Internal(anyhow::anyhow!(
            "OIDC is not configured (discovery failed or has not completed yet)"
        )));
    };

    let (pkce_challenge, pkce_verifier) = PkceCodeChallenge::new_random_sha256();

    let (authorize_url, csrf_state, nonce) = oidc
        .client
        .authorize_url(
            CoreAuthenticationFlow::AuthorizationCode,
            CsrfToken::new_random,
            Nonce::new_random,
        )
        .add_scope(Scope::new("openid".to_string()))
        .add_scope(Scope::new("profile".to_string()))
        .add_scope(Scope::new("email".to_string()))
        .set_pkce_challenge(pkce_challenge)
        .url();

    let flow_state = FlowState {
        pkce_verifier: pkce_verifier.secret().clone(),
        csrf_state: csrf_state.secret().clone(),
        nonce: nonce.secret().clone(),
        client: params.client.filter(|c| c == "app"),
    };
    let value = serde_json::to_string(&flow_state).map_err(|e| AppError::Internal(e.into()))?;

    let mut cookie = Cookie::new(FLOW_COOKIE_NAME, value);
    cookie.set_path("/auth");
    cookie.set_http_only(true);
    cookie.set_same_site(axum_extra::extract::cookie::SameSite::Lax);
    // Match the session cookie's `Secure` flag: the flow cookie carries the
    // PKCE verifier (an http-only, encrypted secret), so it must never travel
    // over cleartext when the deployment is HTTPS.
    cookie.set_secure(state.config.cookie_secure());
    cookie.set_max_age(FLOW_COOKIE_MAX_AGE);
    let jar = jar.add(cookie);

    Ok((jar, Redirect::to(authorize_url.as_str())))
}

#[derive(Debug, Deserialize)]
struct CallbackParams {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

async fn callback(
    State(state): State<AppState>,
    Query(params): Query<CallbackParams>,
    jar: PrivateCookieJar,
    session: tower_sessions::Session,
) -> AppResult<impl axum::response::IntoResponse> {
    if let Some(err) = params.error {
        return Err(AppError::BadRequest(format!(
            "OIDC provider returned an error: {err} ({})",
            params.error_description.unwrap_or_default()
        )));
    }

    let Some(oidc) = state.oidc.as_ref() else {
        return Err(AppError::Internal(anyhow::anyhow!(
            "OIDC is not configured (discovery failed or has not completed yet)"
        )));
    };

    let code = params
        .code
        .ok_or_else(|| AppError::BadRequest("missing `code` query parameter".to_string()))?;
    let returned_state = params
        .state
        .ok_or_else(|| AppError::BadRequest("missing `state` query parameter".to_string()))?;

    let flow_cookie = jar
        .get(FLOW_COOKIE_NAME)
        .ok_or_else(|| AppError::BadRequest("missing or expired login flow cookie".to_string()))?;
    let flow_state: FlowState = serde_json::from_str(flow_cookie.value())
        .map_err(|_| AppError::BadRequest("invalid login flow cookie".to_string()))?;

    let jar = jar.remove(Cookie::from(FLOW_COOKIE_NAME));

    if returned_state != flow_state.csrf_state {
        return Err(AppError::BadRequest("state mismatch".to_string()));
    }

    let token_response = oidc
        .client
        .exchange_code(AuthorizationCode::new(code))
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        .set_pkce_verifier(PkceCodeVerifier::new(flow_state.pkce_verifier))
        .request_async(&oidc.http_client)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    let id_token = token_response.id_token().ok_or_else(|| {
        AppError::Internal(anyhow::anyhow!("provider did not return an ID token"))
    })?;
    let nonce = Nonce::new(flow_state.nonce);
    let id_token_verifier = oidc.client.id_token_verifier();
    let claims = id_token
        .claims(&id_token_verifier, &nonce)
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    let sub = claims.subject().as_str().to_string();
    let email = claims.email().map(|e| e.as_str().to_string());
    let display_name = claims
        .name()
        .and_then(|n| n.get(None))
        .map(|n| n.as_str().to_string())
        .or_else(|| claims.preferred_username().map(|u| u.as_str().to_string()));

    let now = time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .map_err(|e| AppError::Internal(e.into()))?;

    sqlx::query(
        "INSERT INTO users (id, email, display_name, created_at) VALUES (?, ?, ?, ?) \
         ON CONFLICT(id) DO UPDATE SET email = excluded.email, display_name = excluded.display_name",
    )
    .bind(&sub)
    .bind(&email)
    .bind(&display_name)
    .bind(&now)
    .execute(&state.db)
    .await
    .map_err(|e| AppError::Internal(e.into()))?;

    session::login(&session, &sub).await?;

    let _ = token_response.access_token();

    if flow_state.client.as_deref() == Some("app") {
        let raw = crate::auth::device_token::create(&state.db, &sub, "android-app").await?;
        return Ok((
            jar,
            Redirect::to(&format!("{APP_REDIRECT_ORIGIN}?token={raw}")),
        ));
    }

    Ok((jar, Redirect::to(&state.config.base_url)))
}

async fn logout(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    session: tower_sessions::Session,
) -> AppResult<impl axum::response::IntoResponse> {
    if let Some(token) = crate::auth::device_token::bearer_from_headers(&headers) {
        crate::auth::device_token::revoke(&state.db, &token).await?;
    }
    session::logout(&session).await?;
    Ok(axum::http::StatusCode::NO_CONTENT)
}

#[derive(Debug, Serialize)]
struct MeResponse {
    id: String,
    email: Option<String>,
    display_name: Option<String>,
}

async fn me(
    State(state): State<AppState>,
    session::RequireAuth(user_id): session::RequireAuth,
) -> AppResult<Json<MeResponse>> {
    let row: Option<(String, Option<String>, Option<String>)> =
        sqlx::query_as("SELECT id, email, display_name FROM users WHERE id = ?")
            .bind(&user_id)
            .fetch_optional(&state.db)
            .await
            .map_err(|e| AppError::Internal(e.into()))?;

    let (id, email, display_name) = row.ok_or(AppError::Unauthorized)?;

    Ok(Json(MeResponse {
        id,
        email,
        display_name,
    }))
}
