//! The slower "deep dive" pass: grounds the triage pass's best guess in its
//! Wikipedia article and surfaces dangerous look-alikes with a checklist of
//! what to check to rule them out.
//!
//! **Safety design.** Curated, hand-checked data (`species::curated_confusants_for`,
//! backed by migration `0005_species_reference.sql`) is the trustworthy
//! floor and is *always* included — never dropped, never overwritten by a
//! confused or failed LLM call. The LLM is used only as an *enrichment*
//! layer on top of it: Wikipedia grounding (best-effort — a dead/unreachable
//! Wikipedia must not break deep dive), one bounded tool-calling round to
//! suggest *additional* look-alikes the curated set doesn't know about (and
//! even then, a model-suggested extra's `danger_level` is never trusted —
//! it's looked back up against the curated data, or marked `"unknown"`,
//! never taken from the model's own say-so), and a final synthesis call
//! whose prose is appended to, never replaces, the hardcoded
//! [`SAFETY_DISCLAIMER`] constant.
//!
//! Owns migration `0008_deepdive.sql` (`deepdive_results`).
//!
//! Pipeline (see `docs/ARCHITECTURE.md`'s "Deep dive" deep-dive section):
//!   1. Load the latest `triage_results` row; require at least a genus.
//!   2. Wikipedia grounding (`wikipedia::fetch_or_cache` + chunk + embed).
//!   3. Confusant finding: curated pairs + one bounded LLM tool-call round.
//!   4. Synthesis: one `chat_json` call for prose `safety_notes`, with the
//!      disclaimer prepended programmatically.
//!   5. Persist + return.

use std::collections::HashSet;

use axum::extract::{Path, State};
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::auth::session::RequireAuth;
use crate::error::{AppError, AppResult};
use crate::state::AppState;
use crate::{species, vector, wikipedia};
use forage_buddy_core::domain::{TriageStatus, SAFETY_DISCLAIMER};

/// How many Wikipedia chunks to pull back for the `search_wikipedia` tool.
const SEARCH_TOP_K: usize = 5;

/// One confusant entry as stored in `deepdive_results.confusants_json` and
/// returned to the frontend. Unlike [`species::ConfusantDto`] (curated-only,
/// joined off a specific pair row), this also covers model-identified
/// extras — `danger_level` on an extra is either backed by a curated lookup
/// or `"unknown"`, never a bare model guess.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfusantOut {
    pub species: String,
    pub common_name: Option<String>,
    pub danger_level: String,
    pub distinguishing_features: Vec<String>,
    pub notes: String,
}

impl ConfusantOut {
    fn from_curated(c: species::ConfusantDto) -> Self {
        let species_name = match &c.species {
            Some(sp) => format!("{} {}", c.genus, sp),
            None => c.genus.clone(),
        };
        ConfusantOut {
            species: species_name,
            common_name: c.common_names.into_iter().next(),
            danger_level: c.danger_level,
            distinguishing_features: c.distinguishing_features,
            notes: c.notes,
        }
    }
}

/// A stored deep-dive attempt, as returned to the frontend.
#[derive(Debug, Clone, Serialize)]
pub struct DeepDiveDto {
    pub id: String,
    pub sighting_id: String,
    pub created_at: String,
    pub model: String,
    pub best_match_species: String,
    pub confidence: f64,
    pub wikipedia_title: Option<String>,
    pub wikipedia_url: Option<String>,
    pub wikipedia_extract: Option<String>,
    pub confusants: Vec<ConfusantOut>,
    pub safety_notes: String,
}

#[derive(Debug, sqlx::FromRow)]
struct LatestTriage {
    status: String,
    genus: Option<String>,
    candidate_species_json: String,
}

#[derive(Debug, Deserialize)]
struct TriageCandidate {
    species: String,
    #[serde(default)]
    common_name: Option<String>,
    #[serde(default)]
    confidence: f64,
}

/// Additional look-alike suggested by the enrichment LLM call. Note there is
/// deliberately no field for a model-reported danger level — see the
/// module doc comment; any danger level shown to the user is always derived
/// from a curated lookup (or `"unknown"`), never taken from this struct.
#[derive(Debug, Deserialize)]
struct ExtraConfusant {
    species: String,
    #[serde(default)]
    common_name: Option<String>,
    #[serde(default)]
    distinguishing_features: Vec<String>,
    #[serde(default)]
    notes: String,
}

#[derive(Debug, Deserialize)]
struct SynthesisOutput {
    #[serde(default)]
    safety_notes: String,
}

fn format_rfc3339(t: OffsetDateTime) -> AppResult<String> {
    t.format(&Rfc3339).map_err(|e| AppError::Internal(e.into()))
}

fn normalize_name(s: &str) -> String {
    s.trim().to_lowercase()
}

/// Load the sighting's latest triage attempt and enforce the precondition
/// that deep dive needs at least a genus candidate. `NotFound` when there is
/// no triage attempt at all yet; `BadRequest` when the latest attempt is
/// still `"insufficient"`.
async fn load_latest_triage(db: &SqlitePool, sighting_id: &str) -> AppResult<LatestTriage> {
    let row: Option<LatestTriage> = sqlx::query_as(
        "SELECT status, genus, candidate_species_json FROM triage_results \
         WHERE sighting_id = ? ORDER BY created_at DESC LIMIT 1",
    )
    .bind(sighting_id)
    .fetch_optional(db)
    .await
    .map_err(|e| AppError::Internal(e.into()))?;

    let row = row.ok_or(AppError::NotFound)?;

    if row.status == TriageStatus::Insufficient.as_str() {
        return Err(AppError::BadRequest(
            "deep dive needs at least a genus-level triage result; this sighting's latest triage \
             attempt is still insufficient"
                .to_string(),
        ));
    }

    Ok(row)
}

/// Pick the top candidate species (by confidence) from a triage row, falling
/// back to the bare genus when no species-level candidate was recorded.
/// Returns `(best_match_species, common_name, confidence)`.
fn pick_best_candidate(row: &LatestTriage) -> AppResult<(String, Option<String>, f64)> {
    let candidates: Vec<TriageCandidate> = serde_json::from_str(&row.candidate_species_json)
        .map_err(|e| AppError::Internal(e.into()))?;

    if let Some(best) = candidates.iter().max_by(|a, b| {
        a.confidence
            .partial_cmp(&b.confidence)
            .unwrap_or(std::cmp::Ordering::Equal)
    }) {
        return Ok((
            best.species.clone(),
            best.common_name.clone(),
            best.confidence,
        ));
    }

    let genus = row.genus.clone().ok_or_else(|| {
        AppError::BadRequest(
            "triage result has neither a genus nor any candidate species".to_string(),
        )
    })?;
    Ok((genus, None, 0.0))
}

/// Best Wikipedia title to look up for `best_match_species`: the curated
/// `species_reference.wikipedia_title` when it matches a seed row, else
/// `best_match_species` itself (already either a species name or a bare
/// genus, per [`pick_best_candidate`]).
async fn resolve_wikipedia_title(db: &SqlitePool, best_match_species: &str) -> AppResult<String> {
    let curated = species::lookup(db, best_match_species).await?;
    Ok(curated
        .and_then(|c| c.wikipedia_title)
        .unwrap_or_else(|| best_match_species.to_string()))
}

/// Chunk + embed a fetched Wikipedia page into the `wikipedia`-kind vector
/// store. Best-effort: every failure is logged and skipped, never
/// propagated, since this is pure RAG enrichment on top of data the caller
/// already has in `page.extract`.
async fn embed_and_store_wikipedia_chunks(state: &AppState, page: &wikipedia::WikipediaPage) {
    for chunk in wikipedia::chunk_extract(&page.extract) {
        let embedding = match state.llm.embed(&chunk).await {
            Ok(v) => v,
            Err(err) => {
                tracing::warn!(
                    error = ?err,
                    title = %page.title,
                    "deepdive: failed to embed a wikipedia chunk; skipping"
                );
                continue;
            }
        };

        if let Err(err) = vector::upsert(
            &state.db,
            "wikipedia",
            &page.title,
            &state.config.embedding_model,
            &chunk,
            &embedding,
        )
        .await
        {
            tracing::warn!(
                error = ?err,
                title = %page.title,
                "deepdive: failed to store a wikipedia embedding; skipping"
            );
        }
    }
}

/// Execute the `search_wikipedia` tool call: embed its `query` argument and
/// semantically search the `wikipedia`-kind vector store. Any failure
/// (unparseable arguments, embedding error) degrades to an empty result
/// list rather than propagating — this whole enrichment step is best-effort.
async fn exec_search_wikipedia(
    state: &AppState,
    arguments_json: &str,
) -> Vec<(String, String, f32)> {
    #[derive(Deserialize)]
    struct Args {
        query: String,
    }

    let query = match serde_json::from_str::<Args>(arguments_json) {
        Ok(a) => a.query,
        Err(_) => return Vec::new(),
    };

    let embedding = match state.llm.embed(&query).await {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };

    vector::search(&state.db, "wikipedia", &embedding, SEARCH_TOP_K)
        .await
        .unwrap_or_default()
}

/// Pull the first `[ … ]` JSON array out of `content`, tolerating prose or
/// code fences around it (the model isn't in strict JSON mode here, since
/// `chat_tools` is also used for the tool-calling round).
fn extract_json_array(content: &str) -> Option<&str> {
    let start = content.find('[')?;
    let end = content.rfind(']')?;
    if end < start {
        return None;
    }
    content.get(start..=end)
}

/// Step 3 (enrichment half): ask the model for *additional* look-alikes not
/// already in `curated`, offering it one bounded tool-calling round (an
/// initial call, and — only if it asks — exactly one follow-up call after
/// executing the tool). Any error, timeout, or unparseable response at any
/// point yields an empty list; curated confusants are never at risk here
/// since they're merged in by the caller regardless of this function's
/// outcome.
async fn find_extra_confusants(
    state: &AppState,
    candidate: &str,
    curated: &[species::ConfusantDto],
) -> Vec<ExtraConfusant> {
    let tools = serde_json::json!([{
        "type": "function",
        "function": {
            "name": "search_wikipedia",
            "description": "Semantic search over cached Wikipedia text already fetched for this \
                sighting, to help confirm or extend a list of dangerous look-alike species.",
            "parameters": {
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "a short search phrase" }
                },
                "required": ["query"]
            }
        }
    }]);

    let curated_names: Vec<String> = curated
        .iter()
        .map(|c| match &c.species {
            Some(sp) => format!("{} {}", c.genus, sp),
            None => c.genus.clone(),
        })
        .collect();

    let system = "You help extend a curated list of dangerous look-alike species for a foraging \
        safety app. Only name additional look-alikes you are reasonably confident about from general \
        knowledge; never invent toxicology facts. You may call search_wikipedia once if it would help. \
        Respond with ONLY a JSON array (it may be empty), each item shaped exactly like: \
        {\"species\": string, \"common_name\": string or null, \"distinguishing_features\": \
        [string, ...], \"notes\": string}. Do not include a danger_level field — it is determined \
        separately from trusted reference data, not from your output.";

    let user = format!(
        "Candidate identification: {candidate}. Already-known curated look-alikes (do NOT repeat \
         these): {}.\nList only ADDITIONAL look-alikes not in that list, if you know of any with real \
         confidence. An empty array is a perfectly good answer.",
        if curated_names.is_empty() {
            "(none yet)".to_string()
        } else {
            curated_names.join(", ")
        }
    );

    let mut messages = vec![
        serde_json::json!({ "role": "system", "content": system }),
        serde_json::json!({ "role": "user", "content": user }),
    ];

    let model = state.llm.deepdive_chat_model();

    let first = match state.llm.chat_tools(model, messages.clone(), &tools).await {
        Ok(turn) => turn,
        Err(err) => {
            tracing::warn!(
                error = ?err,
                candidate,
                "deepdive: confusant-enrichment tool call failed; proceeding with curated confusants only"
            );
            return Vec::new();
        }
    };

    let final_content = if first.tool_calls.is_empty() {
        first.content
    } else {
        let Some(call) = first.tool_calls.first() else {
            return Vec::new();
        };

        let results = exec_search_wikipedia(state, &call.arguments).await;
        let results_json = serde_json::to_string(
            &results
                .iter()
                .map(|(_, text, score)| serde_json::json!({ "text": text, "score": score }))
                .collect::<Vec<_>>(),
        )
        .unwrap_or_else(|_| "[]".to_string());

        messages.push(serde_json::json!({
            "role": "assistant",
            "content": first.content,
            "tool_calls": [{
                "id": call.id,
                "type": "function",
                "function": { "name": call.name, "arguments": call.arguments }
            }]
        }));
        messages.push(serde_json::json!({
            "role": "tool",
            "tool_call_id": call.id,
            "content": results_json,
        }));

        // Exactly one bounded follow-up — never loop further, even if this
        // second turn also asks for a tool call.
        match state.llm.chat_tools(model, messages, &tools).await {
            Ok(second) => second.content,
            Err(err) => {
                tracing::warn!(
                    error = ?err,
                    candidate,
                    "deepdive: follow-up confusant-enrichment call failed; proceeding with curated \
                     confusants only"
                );
                None
            }
        }
    };

    let Some(content) = final_content else {
        return Vec::new();
    };

    match extract_json_array(&content) {
        Some(json) => serde_json::from_str::<Vec<ExtraConfusant>>(json).unwrap_or_default(),
        None => Vec::new(),
    }
}

/// Merge curated confusants (always kept) with model-identified extras
/// (deduped by normalized species name; curated wins on overlap). An
/// extra's `danger_level` is looked back up against curated reference data
/// — never taken from the model — and falls back to `"unknown"` rather than
/// a guess when no curated row matches it.
async fn merge_confusants(
    state: &AppState,
    curated: Vec<species::ConfusantDto>,
    extras: Vec<ExtraConfusant>,
) -> Vec<ConfusantOut> {
    let mut merged: Vec<ConfusantOut> = curated
        .into_iter()
        .map(ConfusantOut::from_curated)
        .collect();
    let mut seen: HashSet<String> = merged.iter().map(|c| normalize_name(&c.species)).collect();

    for extra in extras {
        let norm = normalize_name(&extra.species);
        if seen.contains(&norm) {
            continue;
        }

        let danger_level = match species::lookup(&state.db, &extra.species).await {
            Ok(Some(curated_row)) => curated_row.danger_level,
            _ => "unknown".to_string(),
        };

        merged.push(ConfusantOut {
            species: extra.species,
            common_name: extra.common_name,
            danger_level,
            distinguishing_features: extra.distinguishing_features,
            notes: extra.notes,
        });
        seen.insert(norm);
    }

    merged
}

/// Step 4: one final text-only `chat_json` call producing a few sentences of
/// practical guidance. Returns an empty string (never an error) on any LLM
/// failure or empty/missing output — the caller always has the hardcoded
/// [`SAFETY_DISCLAIMER`] to fall back to, so `safety_notes` is never empty.
async fn synthesize_safety_notes(
    state: &AppState,
    candidate: &str,
    common_name: Option<&str>,
    wiki: Option<&wikipedia::WikipediaPage>,
    confusants: &[ConfusantOut],
) -> String {
    let system = "You write short, careful, practical safety notes for a forager reviewing an AI \
        best-guess identification of a wild plant or fungus. Be factual and cautious, never claim \
        certainty, and do not write your own disclaimer — one is added separately. Respond as JSON: \
        {\"safety_notes\": \"...\"}.";

    let wiki_summary = wiki
        .map(|p| p.extract.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("(no Wikipedia article could be retrieved for this candidate)");

    let confusant_summary = if confusants.is_empty() {
        "(no known dangerous look-alikes on file)".to_string()
    } else {
        confusants
            .iter()
            .map(|c| format!("{} [{}]", c.species, c.danger_level))
            .collect::<Vec<_>>()
            .join("; ")
    };

    let common_name_suffix = common_name.map(|c| format!(" ({c})")).unwrap_or_default();
    let user = format!(
        "Best-guess identification: {candidate}{common_name_suffix}.\nWikipedia summary: \
         {wiki_summary}\nKnown dangerous look-alikes: {confusant_summary}\n\nWrite 2-4 sentences of \
         practical guidance for double-checking this identification before eating/using anything, \
         calling out the most dangerous look-alike(s) by name where useful."
    );

    let model = state.llm.deepdive_chat_model();
    match state
        .llm
        .chat_json::<SynthesisOutput>(model, system, &user)
        .await
    {
        Ok(s) => s.safety_notes.trim().to_string(),
        Err(err) => {
            tracing::warn!(
                error = ?err,
                candidate,
                "deepdive: synthesis call failed; falling back to the disclaimer alone"
            );
            String::new()
        }
    }
}

#[derive(Debug, sqlx::FromRow)]
struct DeepDiveRow {
    id: String,
    sighting_id: String,
    created_at: String,
    model: String,
    best_match_species: String,
    confidence: f64,
    wikipedia_title: Option<String>,
    wikipedia_url: Option<String>,
    wikipedia_extract: Option<String>,
    confusants_json: String,
    safety_notes: String,
}

fn row_to_dto(row: DeepDiveRow) -> AppResult<DeepDiveDto> {
    let confusants: Vec<ConfusantOut> =
        serde_json::from_str(&row.confusants_json).map_err(|e| AppError::Internal(e.into()))?;

    Ok(DeepDiveDto {
        id: row.id,
        sighting_id: row.sighting_id,
        created_at: row.created_at,
        model: row.model,
        best_match_species: row.best_match_species,
        confidence: row.confidence,
        wikipedia_title: row.wikipedia_title,
        wikipedia_url: row.wikipedia_url,
        wikipedia_extract: row.wikipedia_extract,
        confusants,
        safety_notes: row.safety_notes,
    })
}

#[allow(clippy::too_many_arguments)]
async fn persist(
    db: &SqlitePool,
    sighting_id: &str,
    model: &str,
    best_match_species: &str,
    confidence: f64,
    wiki: Option<&wikipedia::WikipediaPage>,
    confusants: &[ConfusantOut],
    safety_notes: &str,
) -> AppResult<DeepDiveDto> {
    let id = Uuid::new_v4().to_string();
    let created_at = format_rfc3339(OffsetDateTime::now_utc())?;
    let confusants_json =
        serde_json::to_string(confusants).map_err(|e| AppError::Internal(e.into()))?;

    sqlx::query(
        "INSERT INTO deepdive_results \
         (id, sighting_id, created_at, model, best_match_species, confidence, wikipedia_title, \
          wikipedia_url, wikipedia_extract, confusants_json, safety_notes) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(sighting_id)
    .bind(&created_at)
    .bind(model)
    .bind(best_match_species)
    .bind(confidence)
    .bind(wiki.map(|w| w.title.clone()))
    .bind(wiki.map(|w| w.url.clone()))
    .bind(wiki.map(|w| w.extract.clone()))
    .bind(&confusants_json)
    .bind(safety_notes)
    .execute(db)
    .await
    .map_err(|e| AppError::Internal(e.into()))?;

    Ok(DeepDiveDto {
        id,
        sighting_id: sighting_id.to_string(),
        created_at,
        model: model.to_string(),
        best_match_species: best_match_species.to_string(),
        confidence,
        wikipedia_title: wiki.map(|w| w.title.clone()),
        wikipedia_url: wiki.map(|w| w.url.clone()),
        wikipedia_extract: wiki.map(|w| w.extract.clone()),
        confusants: confusants.to_vec(),
        safety_notes: safety_notes.to_string(),
    })
}

/// Whether any deep-dive attempt has ever been persisted for this sighting.
/// Used to decide whether to auto-trigger one after triage lands on a
/// non-insufficient result — auto-triggering should happen once, not on
/// every subsequent triage re-run as the user adds more photos (a manual
/// "Re-run deep dive" stays available for that).
pub async fn has_any_result(db: &SqlitePool, sighting_id: &str) -> AppResult<bool> {
    let row: Option<(i64,)> =
        sqlx::query_as("SELECT 1 FROM deepdive_results WHERE sighting_id = ? LIMIT 1")
            .bind(sighting_id)
            .fetch_optional(db)
            .await
            .map_err(|e| AppError::Internal(e.into()))?;
    Ok(row.is_some())
}

/// Run the full deep-dive pipeline for a sighting and persist a new attempt.
/// `NotFound` when the sighting has no triage attempt yet; `BadRequest` when
/// the latest triage attempt is still `"insufficient"`.
pub async fn run_deepdive(state: &AppState, sighting_id: &str) -> AppResult<DeepDiveDto> {
    // Step 0: load the candidate to ground/enrich.
    let triage = load_latest_triage(&state.db, sighting_id).await?;
    let (best_match_species, common_name, confidence) = pick_best_candidate(&triage)?;

    // Step 1: Wikipedia grounding (best-effort; `None` on any live-fetch
    // failure, never an error — see `wikipedia::fetch_or_cache`).
    let wiki_title = resolve_wikipedia_title(&state.db, &best_match_species).await?;
    let http = wikipedia::build_client();
    let wiki_page = wikipedia::fetch_or_cache(&state.db, &http, &wiki_title).await?;
    if let Some(page) = &wiki_page {
        embed_and_store_wikipedia_chunks(state, page).await;
    }

    // Step 2: confusant finding. Curated pairs are always included;
    // model-identified extras are enrichment only.
    let curated_confusants =
        species::curated_confusants_for(&state.db, &best_match_species).await?;
    let extras = find_extra_confusants(state, &best_match_species, &curated_confusants).await;
    let confusants = merge_confusants(state, curated_confusants, extras).await;

    // Step 3: synthesis. The disclaimer is hardcoded and always present,
    // regardless of whether the model call succeeds.
    let model_notes = synthesize_safety_notes(
        state,
        &best_match_species,
        common_name.as_deref(),
        wiki_page.as_ref(),
        &confusants,
    )
    .await;
    let safety_notes = if model_notes.is_empty() {
        SAFETY_DISCLAIMER.to_string()
    } else {
        format!("{SAFETY_DISCLAIMER}\n\n{model_notes}")
    };

    // Step 4: persist + return.
    persist(
        &state.db,
        sighting_id,
        state.llm.deepdive_chat_model(),
        &best_match_species,
        confidence,
        wiki_page.as_ref(),
        &confusants,
        &safety_notes,
    )
    .await
}

async fn fetch_latest(db: &SqlitePool, sighting_id: &str) -> AppResult<Option<DeepDiveDto>> {
    let row: Option<DeepDiveRow> = sqlx::query_as(
        "SELECT id, sighting_id, created_at, model, best_match_species, confidence, wikipedia_title, \
         wikipedia_url, wikipedia_extract, confusants_json, safety_notes \
         FROM deepdive_results WHERE sighting_id = ? ORDER BY created_at DESC LIMIT 1",
    )
    .bind(sighting_id)
    .fetch_optional(db)
    .await
    .map_err(|e| AppError::Internal(e.into()))?;

    row.map(row_to_dto).transpose()
}

/// `NotFound` unless `sighting_id` exists and is owned by `user_id` — same
/// ownership-check pattern as the `triage` module's routes.
async fn ensure_owned(db: &SqlitePool, sighting_id: &str, user_id: &str) -> AppResult<()> {
    let owner: Option<(String,)> = sqlx::query_as("SELECT user_id FROM sightings WHERE id = ?")
        .bind(sighting_id)
        .fetch_optional(db)
        .await
        .map_err(|e| AppError::Internal(e.into()))?;

    match owner {
        Some((owner_id,)) if owner_id == user_id => Ok(()),
        _ => Err(AppError::NotFound),
    }
}

async fn trigger_deepdive(
    State(state): State<AppState>,
    RequireAuth(user_id): RequireAuth,
    Path(sighting_id): Path<String>,
) -> AppResult<Json<DeepDiveDto>> {
    ensure_owned(&state.db, &sighting_id, &user_id).await?;
    let result = run_deepdive(&state, &sighting_id).await?;
    Ok(Json(result))
}

async fn latest_deepdive(
    State(state): State<AppState>,
    RequireAuth(user_id): RequireAuth,
    Path(sighting_id): Path<String>,
) -> AppResult<Json<DeepDiveDto>> {
    ensure_owned(&state.db, &sighting_id, &user_id).await?;
    let result = fetch_latest(&state.db, &sighting_id)
        .await?
        .ok_or(AppError::NotFound)?;
    Ok(Json(result))
}

pub fn router() -> Router<AppState> {
    Router::new().route(
        "/api/sightings/{id}/deepdive",
        get(latest_deepdive).post(trigger_deepdive),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn test_state() -> (tempfile::TempDir, AppState) {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let db = crate::db::init_pool(db_path.to_str().unwrap())
            .await
            .unwrap();

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS sightings ( \
                id TEXT PRIMARY KEY, user_id TEXT NOT NULL, status TEXT NOT NULL DEFAULT 'open', \
                lat REAL, lon REAL, location_accuracy_m REAL, place_label TEXT, \
                observed_at TEXT NOT NULL, notes TEXT, created_at TEXT NOT NULL, updated_at TEXT NOT NULL \
            )",
        )
        .execute(&db)
        .await
        .unwrap();
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS triage_results ( \
                id TEXT PRIMARY KEY, sighting_id TEXT NOT NULL, created_at TEXT NOT NULL, \
                model TEXT NOT NULL, status TEXT NOT NULL, genus TEXT, \
                candidate_species_json TEXT NOT NULL, missing_info_json TEXT NOT NULL, \
                reasoning TEXT NOT NULL, photos_considered INTEGER NOT NULL \
            )",
        )
        .execute(&db)
        .await
        .unwrap();

        sqlx::query(
            "INSERT INTO users (id, email, display_name, created_at) VALUES ('u1', NULL, NULL, '2026-01-01T00:00:00Z')",
        )
        .execute(&db)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO sightings (id, user_id, status, observed_at, created_at, updated_at) \
             VALUES ('s1', 'u1', 'open', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
        )
        .execute(&db)
        .await
        .unwrap();

        let config = crate::config::Config::from_env();
        let llm = crate::llm::LlmClient::new(&config);
        let state = AppState {
            db,
            config: std::sync::Arc::new(config),
            oidc: None,
            cookie_key: axum_extra::extract::cookie::Key::generate(),
            llm,
        };
        (dir, state)
    }

    async fn insert_triage(
        db: &SqlitePool,
        sighting_id: &str,
        status: &str,
        genus: Option<&str>,
        candidate_species_json: &str,
    ) {
        sqlx::query(
            "INSERT INTO triage_results \
             (id, sighting_id, created_at, model, status, genus, candidate_species_json, \
              missing_info_json, reasoning, photos_considered) \
             VALUES (?, ?, datetime('now'), 'test-model', ?, ?, ?, '[]', 'test', 1)",
        )
        .bind(Uuid::new_v4().to_string())
        .bind(sighting_id)
        .bind(status)
        .bind(genus)
        .bind(candidate_species_json)
        .execute(db)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn run_deepdive_errors_not_found_without_any_triage() {
        let (_dir, state) = test_state().await;
        let err = run_deepdive(&state, "s1").await.unwrap_err();
        assert!(matches!(err, AppError::NotFound));
    }

    #[tokio::test]
    async fn run_deepdive_errors_bad_request_on_insufficient_triage() {
        let (_dir, state) = test_state().await;
        insert_triage(&state.db, "s1", "insufficient", None, "[]").await;

        let err = run_deepdive(&state, "s1").await.unwrap_err();
        assert!(matches!(err, AppError::BadRequest(_)));
    }

    #[test]
    fn pick_best_candidate_prefers_highest_confidence_species() {
        let row = LatestTriage {
            status: "species_candidate".to_string(),
            genus: Some("Amanita".to_string()),
            candidate_species_json: r#"[
                {"species":"Amanita phalloides","common_name":"Death cap","confidence":0.4},
                {"species":"Amanita virosa","common_name":"Destroying angel","confidence":0.8}
            ]"#
            .to_string(),
        };

        let (species_name, common_name, confidence) = pick_best_candidate(&row).unwrap();
        assert_eq!(species_name, "Amanita virosa");
        assert_eq!(common_name, Some("Destroying angel".to_string()));
        assert_eq!(confidence, 0.8);
    }

    #[test]
    fn pick_best_candidate_falls_back_to_genus() {
        let row = LatestTriage {
            status: "genus_candidate".to_string(),
            genus: Some("Amanita".to_string()),
            candidate_species_json: "[]".to_string(),
        };

        let (species_name, common_name, confidence) = pick_best_candidate(&row).unwrap();
        assert_eq!(species_name, "Amanita");
        assert_eq!(common_name, None);
        assert_eq!(confidence, 0.0);
    }

    #[test]
    fn pick_best_candidate_errors_with_neither_genus_nor_species() {
        let row = LatestTriage {
            status: "genus_candidate".to_string(),
            genus: None,
            candidate_species_json: "[]".to_string(),
        };

        assert!(pick_best_candidate(&row).is_err());
    }

    #[tokio::test]
    async fn merge_confusants_keeps_curated_and_marks_unknown_extras() {
        let (_dir, state) = test_state().await;

        let curated = vec![species::ConfusantDto {
            species_ref_id: "agaricus-bisporus".to_string(),
            genus: "Agaricus".to_string(),
            species: Some("bisporus".to_string()),
            common_names: vec!["button mushroom".to_string()],
            danger_level: "deadly_toxic".to_string(),
            distinguishing_features: vec!["check for a volva".to_string()],
            notes: "curated note".to_string(),
        }];
        let extras = vec![
            // Matches a curated species_reference row by common name -> must
            // pick up the curated danger level, not stay "unknown".
            ExtraConfusant {
                species: "destroying angel".to_string(),
                common_name: None,
                distinguishing_features: vec![],
                notes: String::new(),
            },
            // Not in the curated data at all -> must be "unknown", never a
            // fabricated danger level.
            ExtraConfusant {
                species: "Totally Fictional Species".to_string(),
                common_name: None,
                distinguishing_features: vec![],
                notes: String::new(),
            },
        ];

        let merged = merge_confusants(&state, curated, extras).await;

        assert_eq!(merged.len(), 3);
        let fictional = merged
            .iter()
            .find(|c| c.species == "Totally Fictional Species")
            .unwrap();
        assert_eq!(fictional.danger_level, "unknown");
        let destroying_angel = merged
            .iter()
            .find(|c| c.species == "destroying angel")
            .unwrap();
        assert_eq!(destroying_angel.danger_level, "deadly_toxic");
    }

    #[test]
    fn extract_json_array_tolerates_surrounding_prose() {
        assert_eq!(
            extract_json_array("sure, here: [1,2,3] thanks"),
            Some("[1,2,3]")
        );
        assert_eq!(extract_json_array("no array here"), None);
    }
}
