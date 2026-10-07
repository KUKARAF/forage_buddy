//! Application configuration, loaded from environment variables.
//!
//! All variables are prefixed `FORAGEBUDDY_`. Every field has a dev-friendly
//! default so the server starts locally with zero configuration; production
//! deployments override these via the environment. Ported from `ai_buddy`'s
//! `config.rs`, minus wallet/MCP/FCM/GeoIP fields which this app has no use
//! for.

#[derive(Debug, Clone)]
pub struct Config {
    pub authentik_issuer_url: String,
    pub oidc_client_id: String,
    /// `None` (unset/empty) means a Public/PKCE client (no shared secret).
    pub oidc_client_secret: Option<String>,
    pub oidc_redirect_uri: String,
    pub cookie_signing_key: String,
    pub base_url: String,
    pub sqlite_path: String,
    /// Directory original photos are stored under (one subdirectory per
    /// sighting id). Created on demand.
    pub photo_dir: String,
    pub bind_addr: String,
    pub cookie_secure: Option<bool>,
    /// If true, authentication is bypassed and every request is user "admin".
    /// Enabled by `FORAGEBUDDY_ENV=dev` or `FORAGEBUDDY_DEV_MODE=true`. Never
    /// enable in a deployment reachable by anyone else.
    pub dev_mode: bool,
    pub cors_allowed_origins: Vec<String>,
    /// Path to the built SvelteKit static assets (`web/build`). When set, the
    /// server serves the SPA (fallback `200.html`) so frontend + backend are
    /// one origin. Unset in local dev (Vite serves the frontend).
    pub static_dir: Option<String>,

    // --- LLM / RAG ---
    /// Which provider serves chat/vision completions: `"litellm"` (default)
    /// or `"openrouter"`.
    pub llm_provider: String,
    pub openrouter_api_key: Option<String>,
    pub litellm_api_key: Option<String>,
    pub litellm_base_url: String,
    /// Default chat model — MUST be vision-capable (used for triage).
    pub chat_model: String,
    /// Model for the identification pipeline's facts/risks gatherers
    /// (species facts lookup + confusant-enrichment tool calls) — these run
    /// off the vision gatherer's "need an answer in seconds" critical path,
    /// so a stronger (slower/costlier) model is a reasonable choice here.
    /// Defaults to `chat_model` when unset, so this is opt-in, not required.
    pub identification_chat_model: String,
    pub allowed_chat_models: Vec<String>,
    pub embedding_model: String,
    pub embedding_dim: usize,

    /// Per-photo upload cap (bytes), enforced before decode/resize.
    pub max_photo_bytes: usize,
}

/// User id used for every request when [`Config::dev_mode`] is enabled.
pub const DEV_MODE_USER_ID: &str = "admin";

/// Committed, INSECURE default cookie signing key so local dev works with
/// zero setup. Not valid standard base64 (`-` chars), so `signing_key_bytes`
/// falls back to its raw UTF-8 bytes (>= 32). [`Config::validate`] refuses to
/// boot on this value unless dev mode is on.
pub const DEV_DEFAULT_COOKIE_SIGNING_KEY: &str =
    "forage-buddy-dev-insecure-signing-key-change-me-0123456789";

/// Dev-friendly default base URL. A real deployment must set `FORAGEBUDDY_BASE_URL`.
pub const DEV_DEFAULT_BASE_URL: &str = "http://localhost:8080";

/// Minimum length (bytes, after decoding) of the cookie signing key.
pub const MIN_COOKIE_SIGNING_KEY_BYTES: usize = 32;

impl Config {
    pub fn from_env() -> Self {
        let base_url = std::env::var("FORAGEBUDDY_BASE_URL")
            .unwrap_or_else(|_| DEV_DEFAULT_BASE_URL.to_string());
        let chat_model = std::env::var("FORAGEBUDDY_CHAT_MODEL")
            .unwrap_or_else(|_| "openrouter/~anthropic/claude-haiku-latest".to_string());
        let identification_chat_model = std::env::var("FORAGEBUDDY_IDENTIFICATION_MODEL")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| chat_model.clone());

        Self {
            authentik_issuer_url: std::env::var("FORAGEBUDDY_AUTHENTIK_ISSUER_URL").unwrap_or_else(
                |_| "http://localhost:9000/application/o/forage-buddy/".to_string(),
            ),
            oidc_client_id: std::env::var("FORAGEBUDDY_OIDC_CLIENT_ID")
                .unwrap_or_else(|_| "forage-buddy-dev".to_string()),
            oidc_client_secret: std::env::var("FORAGEBUDDY_OIDC_CLIENT_SECRET")
                .ok()
                .filter(|s| !s.is_empty()),
            oidc_redirect_uri: std::env::var("FORAGEBUDDY_OIDC_REDIRECT_URI")
                .unwrap_or_else(|_| "http://localhost:8080/auth/callback".to_string()),
            cookie_signing_key: std::env::var("FORAGEBUDDY_COOKIE_SIGNING_KEY")
                .unwrap_or_else(|_| DEV_DEFAULT_COOKIE_SIGNING_KEY.to_string()),
            base_url,
            sqlite_path: std::env::var("FORAGEBUDDY_SQLITE_PATH")
                .unwrap_or_else(|_| "./data/forage_buddy.db".to_string()),
            photo_dir: std::env::var("FORAGEBUDDY_PHOTO_DIR")
                .unwrap_or_else(|_| "./data/photos".to_string()),
            bind_addr: std::env::var("FORAGEBUDDY_BIND_ADDR")
                .unwrap_or_else(|_| "127.0.0.1:8080".to_string()),
            cookie_secure: std::env::var("FORAGEBUDDY_COOKIE_SECURE")
                .ok()
                .and_then(|v| match v.trim().to_ascii_lowercase().as_str() {
                    "true" | "1" | "yes" => Some(true),
                    "false" | "0" | "no" => Some(false),
                    _ => None,
                }),
            dev_mode: std::env::var("FORAGEBUDDY_ENV").as_deref() == Ok("dev")
                || std::env::var("FORAGEBUDDY_DEV_MODE").as_deref() == Ok("true"),
            cors_allowed_origins: std::env::var("FORAGEBUDDY_CORS_ORIGINS")
                .map(|v| v.split(',').map(|s| s.trim().to_string()).collect())
                .unwrap_or_else(|_| {
                    [
                        "http://localhost:5173",
                        "http://127.0.0.1:5173",
                        "http://localhost:1420",
                        "http://127.0.0.1:1420",
                        // The Tauri Android app's fixed webview origin.
                        "http://tauri.localhost",
                    ]
                    .into_iter()
                    .map(String::from)
                    .collect()
                }),
            static_dir: std::env::var("FORAGEBUDDY_STATIC_DIR").ok(),

            llm_provider: std::env::var("FORAGEBUDDY_LLM_PROVIDER")
                .ok()
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "litellm".to_string()),
            openrouter_api_key: std::env::var("FORAGEBUDDY_OPENROUTER_API_KEY")
                .ok()
                .filter(|s| !s.is_empty()),
            litellm_api_key: std::env::var("FORAGEBUDDY_LITELLM_API_KEY")
                .ok()
                .filter(|s| !s.is_empty()),
            litellm_base_url: std::env::var("FORAGEBUDDY_LITELLM_BASE_URL")
                .ok()
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "https://litellm.osmosis.page/v1".to_string()),
            chat_model,
            identification_chat_model,
            allowed_chat_models: std::env::var("FORAGEBUDDY_ALLOWED_CHAT_MODELS")
                .ok()
                .map(|v| {
                    v.split(',')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect::<Vec<String>>()
                })
                .filter(|v| !v.is_empty())
                .unwrap_or_else(|| {
                    vec![
                        "openrouter/~anthropic/claude-haiku-latest".to_string(),
                        "gemma4-26b".to_string(),
                    ]
                }),
            embedding_model: std::env::var("FORAGEBUDDY_EMBEDDING_MODEL")
                .unwrap_or_else(|_| "bge-m3".to_string()),
            embedding_dim: std::env::var("FORAGEBUDDY_EMBEDDING_DIM")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(1024),

            max_photo_bytes: std::env::var("FORAGEBUDDY_MAX_PHOTO_BYTES")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(15 * 1024 * 1024),
        }
    }

    /// Effective `Secure` flag for the session cookie.
    pub fn cookie_secure(&self) -> bool {
        self.cookie_secure
            .unwrap_or_else(|| self.base_url.starts_with("https://"))
    }

    /// Whether [`Self::bind_addr`]'s host is a loopback interface.
    pub fn bind_is_loopback(&self) -> bool {
        use std::net::{IpAddr, SocketAddr};

        if let Ok(addr) = self.bind_addr.parse::<SocketAddr>() {
            return addr.ip().is_loopback();
        }
        let host = match self.bind_addr.strip_prefix('[') {
            Some(rest) => rest.split(']').next().unwrap_or(rest),
            None => self
                .bind_addr
                .rsplit_once(':')
                .map(|(h, _)| h)
                .unwrap_or(self.bind_addr.as_str()),
        };
        if let Ok(ip) = host.parse::<IpAddr>() {
            return ip.is_loopback();
        }
        host.eq_ignore_ascii_case("localhost")
    }

    /// Fail-fast validation of cross-field invariants before boot.
    pub fn validate(&self) -> anyhow::Result<()> {
        if self.cookie_signing_key.trim() == DEV_DEFAULT_COOKIE_SIGNING_KEY {
            if self.dev_mode {
                tracing::warn!(
                    "using the built-in INSECURE dev cookie signing key; set \
                     FORAGEBUDDY_COOKIE_SIGNING_KEY to a real secret before exposing this server"
                );
            } else {
                anyhow::bail!(
                    "refusing to boot: FORAGEBUDDY_COOKIE_SIGNING_KEY is unset or left at the \
                     built-in insecure dev default. Set it to a real secret (>= {} bytes).",
                    MIN_COOKIE_SIGNING_KEY_BYTES
                );
            }
        }

        if self.dev_mode && !self.bind_is_loopback() {
            anyhow::bail!(
                "refusing to boot: dev mode bypasses authentication and must not bind a \
                 non-loopback address (bind_addr = {}).",
                self.bind_addr
            );
        }

        if !self.dev_mode && self.base_url == DEV_DEFAULT_BASE_URL {
            anyhow::bail!(
                "refusing to boot: FORAGEBUDDY_BASE_URL is unset or left at the dev default ({}). \
                 Set it to this deployment's real public URL.",
                DEV_DEFAULT_BASE_URL
            );
        }

        Ok(())
    }

    /// Decode [`Self::cookie_signing_key`] to raw bytes, validating length.
    pub fn signing_key_bytes(&self) -> anyhow::Result<Vec<u8>> {
        use base64::Engine;

        let bytes = base64::engine::general_purpose::STANDARD
            .decode(self.cookie_signing_key.trim())
            .unwrap_or_else(|_| self.cookie_signing_key.as_bytes().to_vec());

        anyhow::ensure!(
            bytes.len() >= MIN_COOKIE_SIGNING_KEY_BYTES,
            "FORAGEBUDDY_COOKIE_SIGNING_KEY must decode (or be) at least {} bytes, got {}",
            MIN_COOKIE_SIGNING_KEY_BYTES,
            bytes.len()
        );

        Ok(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dev_default_signing_key_is_valid() {
        let config = Config::from_env();
        let bytes = config
            .signing_key_bytes()
            .expect("dev default should be valid");
        assert!(bytes.len() >= MIN_COOKIE_SIGNING_KEY_BYTES);
    }

    #[test]
    fn short_signing_key_is_rejected() {
        let mut config = Config::from_env();
        config.cookie_signing_key = "too-short".to_string();
        assert!(config.signing_key_bytes().is_err());
    }

    #[test]
    fn dev_default_key_refuses_to_boot_outside_dev_mode() {
        let mut config = Config::from_env();
        config.cookie_signing_key = DEV_DEFAULT_COOKIE_SIGNING_KEY.to_string();
        config.dev_mode = false;
        config.bind_addr = "0.0.0.0:8080".to_string();
        config.base_url = "https://forage.example.com".to_string();
        assert!(config.validate().is_err());

        config.cookie_signing_key = "x".repeat(64);
        assert!(config.validate().is_ok());
    }

    #[test]
    fn dev_mode_refuses_non_loopback_bind() {
        let mut config = Config::from_env();
        config.cookie_signing_key = "x".repeat(64);
        config.dev_mode = true;
        config.bind_addr = "0.0.0.0:8080".to_string();
        assert!(config.validate().is_err());
        for ok in ["127.0.0.1:8080", "localhost:8080", "[::1]:8080"] {
            config.bind_addr = ok.to_string();
            assert!(config.validate().is_ok(), "dev mode + {ok} must be allowed");
        }
    }
}
