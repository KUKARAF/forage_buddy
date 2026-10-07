//! Runtime-editable overrides for which LLM model each identification
//! gatherer uses (owns migration `0013_model_settings.sql`,
//! `model_settings` — a singleton row, id always 1).
//!
//! The `FORAGEBUDDY_CHAT_MODEL`/`FORAGEBUDDY_FACTS_MODEL`/
//! `FORAGEBUDDY_RISK_MODEL`/`FORAGEBUDDY_VISUAL_MATCH_MODEL` env vars (see
//! `Config`) are only the *defaults* used when this table has no override
//! for a given gatherer. [`effective_models`] is what every gatherer in
//! `identification::mod` actually calls, so changing the settings via
//! `PUT /api/settings` (or the web Settings page) takes effect on the very
//! next pipeline run — no restart required.

use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

use crate::auth::session::RequireAuth;
use crate::config::Config;
use crate::error::{AppError, AppResult};
use crate::state::AppState;

/// The model each identification gatherer should use right now — DB
/// override if present, else the matching `Config` default. Never fails:
/// a DB read error degrades to "use the defaults" (logged), the same
/// infallible-degradation posture as the gatherers that call this.
#[derive(Debug, Clone)]
pub struct EffectiveModels {
    pub candidate_model: String,
    pub facts_model: String,
    pub risk_model: String,
    pub visual_match_model: String,
}

#[derive(sqlx::FromRow)]
struct ModelSettingsRow {
    candidate_model: Option<String>,
    facts_model: Option<String>,
    risk_model: Option<String>,
    visual_match_model: Option<String>,
}

fn non_empty(s: Option<String>) -> Option<String> {
    s.filter(|v| !v.trim().is_empty())
}

pub async fn effective_models(db: &SqlitePool, config: &Config) -> EffectiveModels {
    let row = sqlx::query_as::<_, ModelSettingsRow>(
        "SELECT candidate_model, facts_model, risk_model, visual_match_model \
         FROM model_settings WHERE id = 1",
    )
    .fetch_optional(db)
    .await
    .unwrap_or_else(|err| {
        tracing::warn!(error = %err, "settings: reading model_settings failed; using defaults");
        None
    });

    let (candidate, facts, risk, visual_match) = match row {
        Some(r) => (
            non_empty(r.candidate_model),
            non_empty(r.facts_model),
            non_empty(r.risk_model),
            non_empty(r.visual_match_model),
        ),
        None => (None, None, None, None),
    };

    EffectiveModels {
        candidate_model: candidate.unwrap_or_else(|| config.chat_model.clone()),
        facts_model: facts.unwrap_or_else(|| config.facts_chat_model.clone()),
        risk_model: risk.unwrap_or_else(|| config.risk_chat_model.clone()),
        visual_match_model: visual_match.unwrap_or_else(|| config.visual_match_chat_model.clone()),
    }
}

// --- HTTP ---------------------------------------------------------------------

#[derive(Debug, Serialize)]
struct SettingsDto {
    candidate_model: String,
    facts_model: String,
    risk_model: String,
    visual_match_model: String,
    available_models: Vec<String>,
}

impl SettingsDto {
    fn from_effective(models: EffectiveModels, config: &Config) -> Self {
        SettingsDto {
            candidate_model: models.candidate_model,
            facts_model: models.facts_model,
            risk_model: models.risk_model,
            visual_match_model: models.visual_match_model,
            available_models: config.allowed_chat_models.clone(),
        }
    }
}

#[derive(Debug, Deserialize)]
struct UpdateSettingsReq {
    candidate_model: String,
    facts_model: String,
    risk_model: String,
    visual_match_model: String,
}

fn validate_model(config: &Config, field: &str, value: &str) -> AppResult<()> {
    if config.allowed_chat_models.iter().any(|m| m == value) {
        Ok(())
    } else {
        Err(AppError::BadRequest(format!(
            "{field}: {value:?} is not one of the allowed models"
        )))
    }
}

async fn get_settings(
    State(state): State<AppState>,
    RequireAuth(_user_id): RequireAuth,
) -> AppResult<Json<SettingsDto>> {
    let models = effective_models(&state.db, &state.config).await;
    Ok(Json(SettingsDto::from_effective(models, &state.config)))
}

async fn update_settings(
    State(state): State<AppState>,
    RequireAuth(_user_id): RequireAuth,
    Json(body): Json<UpdateSettingsReq>,
) -> AppResult<Json<SettingsDto>> {
    validate_model(&state.config, "candidate_model", &body.candidate_model)?;
    validate_model(&state.config, "facts_model", &body.facts_model)?;
    validate_model(&state.config, "risk_model", &body.risk_model)?;
    validate_model(
        &state.config,
        "visual_match_model",
        &body.visual_match_model,
    )?;

    let now = time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .map_err(|e| AppError::Internal(e.into()))?;

    sqlx::query(
        "INSERT INTO model_settings \
             (id, candidate_model, facts_model, risk_model, visual_match_model, updated_at) \
         VALUES (1, ?, ?, ?, ?, ?) \
         ON CONFLICT(id) DO UPDATE SET \
             candidate_model = excluded.candidate_model, \
             facts_model = excluded.facts_model, \
             risk_model = excluded.risk_model, \
             visual_match_model = excluded.visual_match_model, \
             updated_at = excluded.updated_at",
    )
    .bind(&body.candidate_model)
    .bind(&body.facts_model)
    .bind(&body.risk_model)
    .bind(&body.visual_match_model)
    .bind(&now)
    .execute(&state.db)
    .await
    .map_err(|e| AppError::Internal(e.into()))?;

    let models = effective_models(&state.db, &state.config).await;
    Ok(Json(SettingsDto::from_effective(models, &state.config)))
}

pub fn router() -> Router<AppState> {
    Router::new().route("/api/settings", get(get_settings).put(update_settings))
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn test_pool() -> (tempfile::TempDir, SqlitePool) {
        let dir = tempfile::tempdir().expect("tempdir");
        let db_path = dir.path().join("test.db");
        let pool = crate::db::init_pool(db_path.to_str().expect("utf8 path"))
            .await
            .expect("init pool");
        (dir, pool)
    }

    #[tokio::test]
    async fn effective_models_falls_back_to_config_defaults_with_no_row() {
        let (_dir, pool) = test_pool().await;
        let config = Config::from_env();
        let models = effective_models(&pool, &config).await;
        assert_eq!(models.candidate_model, config.chat_model);
        assert_eq!(models.facts_model, config.facts_chat_model);
        assert_eq!(models.risk_model, config.risk_chat_model);
        assert_eq!(models.visual_match_model, config.visual_match_chat_model);
    }

    #[tokio::test]
    async fn effective_models_uses_override_only_for_set_fields() {
        let (_dir, pool) = test_pool().await;
        let config = Config::from_env();

        sqlx::query(
            "INSERT INTO model_settings (id, candidate_model, facts_model, risk_model, \
                 visual_match_model, updated_at) \
             VALUES (1, 'openrouter/~anthropic/claude-sonnet-latest', NULL, '', NULL, '2026-01-01T00:00:00Z')",
        )
        .execute(&pool)
        .await
        .expect("insert override row");

        let models = effective_models(&pool, &config).await;
        assert_eq!(
            models.candidate_model,
            "openrouter/~anthropic/claude-sonnet-latest"
        );
        // NULL and empty-string overrides both fall back to the config default.
        assert_eq!(models.facts_model, config.facts_chat_model);
        assert_eq!(models.risk_model, config.risk_chat_model);
        assert_eq!(models.visual_match_model, config.visual_match_chat_model);
    }
}
