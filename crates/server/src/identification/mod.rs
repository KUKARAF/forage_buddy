//! The single, automatic "identification" pipeline: replaces the old
//! two-stage triage-then-manual-deep-dive flow with one pipeline that runs
//! automatically on every photo upload, made up of three internal gatherer
//! stages:
//!
//!   1. [`gather_candidates`] — a cheap vision call over the sighting's
//!      photos, producing candidate species + confidences (or an honest
//!      empty list + concrete `missing_info` when the photos aren't enough
//!      — see the safety note below, ported verbatim from the old `triage`
//!      module).
//!   2. [`gather_facts`] — per top candidate, a read-through
//!      `species_facts_cache` lookup (Wikipedia grounding + one LLM call on
//!      a cache miss) producing edible/medicinal/psychoactive/poisonous
//!      flags and a short risk note.
//!   3. [`gather_risks`] — per top candidate, curated `confusant_pairs` plus
//!      one bounded LLM tool-calling round to find additional dangerous
//!      look-alikes, condensed to one short phrase each.
//!
//! [`run_identification`] orchestrates all three and persists the result in
//! `identification_results` (migration `0009_identification.sql`); facts are
//! cached across sightings in `species_facts_cache` (migration
//! `0010_species_facts_cache.sql`). The old `triage_results`/
//! `deepdive_results` tables are preserved (never dropped — this app has
//! real production data) and best-effort backfilled into the new shape by
//! migration `0011_identification_backfill.sql`, but are no longer written
//! to going forward.
//!
//! **Safety note**, carried over from the old `triage`/`deepdive` modules:
//! misidentifying a wild mushroom or plant can kill. Gatherer 1 is biased
//! hard toward an honest empty candidate list + concrete `missing_info`
//! over a confident-sounding wrong answer. Curated reference data
//! (`species::lookup`/`species::curated_confusants_for`, backed by
//! `species_reference`/`confusant_pairs`) is a trustworthy FLOOR that an
//! LLM call can only raise, never lower — see [`reconcile_danger`] and the
//! `gather_risks` merge logic.

use std::collections::HashSet;
use std::path::Path as FsPath;

use axum::extract::{Path, State};
use axum::routing::get;
use axum::{Json, Router};
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::auth::session::RequireAuth;
use crate::error::{AppError, AppResult};
use crate::state::AppState;
use crate::{species, wikipedia};

/// Hard cap on how many of a sighting's photos are sent to the vision model
/// per gatherer-1 attempt. Mirrors the old triage module's cap.
const MAX_PHOTOS_PER_ATTEMPT: usize = 6;

/// Model value stored when no LLM call was made for an attempt (e.g. zero
/// photos yet, or all stored photo files were unreadable).
const NO_MODEL_CALL: &str = "none";

/// How many Wikipedia chunks to pull back for the `search_wikipedia` tool
/// in [`find_extra_confusants`].
const SEARCH_TOP_K: usize = 5;

/// How many top candidates gatherers 2+3 run for, and how many
/// [`compile_candidates`] keeps in the final result.
const MAX_CANDIDATES: usize = 3;

/// Hard cap on `risk_note` length (chars), per the frontend JSON contract.
const RISK_NOTE_MAX_CHARS: usize = 140;
/// Hard cap on a confusant `note` length (chars), per the frontend JSON contract.
const CONFUSANT_NOTE_MAX_CHARS: usize = 100;
/// Generous cap on free-text species/common-name/missing-info strings — not
/// part of the documented contract, but every text field gets *some* cap so
/// a misbehaving LLM call can never blow up storage or the response body.
const TEXT_FIELD_MAX_CHARS: usize = 200;

/// System prompt for gatherer 1's vision call. Ported from the old `triage`
/// module's prompt almost verbatim — same honesty bias, same safety
/// rationale — just without the old `status`/`genus` fields, since this
/// pipeline derives "insufficient" from an empty `candidates` list instead.
const CANDIDATES_SYSTEM_PROMPT: &str = "You are Forage Buddy's identification assistant: a careful \
field-identification aid for wild fungi, plants, and other foraged organisms. You are shown photos \
of a specimen plus its observation date, rough location, and any notes the forager wrote, and you \
give a best-effort, provisional read: the most likely species (or genus, if species-level confidence \
isn't there).\n\
\n\
SAFETY IS THE ENTIRE POINT OF THIS TOOL. Misidentifying a wild mushroom or plant can cause severe \
illness or death. You are not a substitute for a qualified local expert, a spore print, or a field \
guide, and every answer you give is shown to the user alongside a prominent disclaimer to that \
effect. Given that, you must NEVER state or imply more certainty than you actually have, and you \
must NEVER guess at a species just to produce an answer. When the photos and context are not enough \
to narrow things down responsibly, the correct and EXPECTED response is an EMPTY candidates array \
with concrete, specific missing_info items describing exactly what would help next \u{2014} not a \
vague \"need more info\". Err strongly on the side of asking for more over committing to a \
confident-sounding wrong answer: a wrong guess can kill, an honest \"I don't know yet, please get me \
X\" cannot.\n\
\n\
Good examples of specific, foraging-relevant asks (draw on patterns like these, adapted to what you \
actually see and still need):\n\
- \"a clear photo of the gill attachment to the stem (free, attached, notched, or decurrent)\"\n\
- \"the spore print color (rest the cap gill-side-down on paper for a few hours)\"\n\
- \"a photo of the full plant/fungus including the base or root, not just the top\"\n\
- \"whether the flesh bruises, stains, or changes color when cut or scratched, and to what color\"\n\
- \"a clear photo of the underside: gills, pores, or teeth, in good light\"\n\
- \"whether there is a ring (annulus) on the stem, and/or a cup (volva) at the base\"\n\
- \"the habitat: growing on wood, on soil, or on a specific host tree species\"\n\
- \"the smell of the specimen (e.g. anise, almond, foul, bleach-like, none noticed)\"\n\
- \"a photo showing true scale next to a coin or ruler\"\n\
\n\
Rules:\n\
1. Treat the photos as the primary evidence. Use the observation date and rough location only to \
narrow candidates by season/region plausibility \u{2014} never claim to know the exact location, \
habitat, or anything not actually visible or stated.\n\
2. If you are told what the user was previously asked to provide, check whether the newly supplied \
photo(s)/notes actually answer that before asking for something else.\n\
3. List multiple plausible candidates when relevant, INCLUDING dangerous look-alikes you want the \
user to rule out, each with an honest confidence between 0 and 1. Do not inflate confidence to sound \
more useful. A bare genus (e.g. \"Amanita\") is an acceptable candidate `species` value when you \
can't responsibly narrow further.\n\
4. Respond with ONLY a single JSON object, no other text, matching exactly:\n\
{\"candidates\": [{\"species\": string, \"common_name\": string or null, \"confidence\": number \
between 0 and 1}], \"missing_info\": [string, ...]}";

/// System prompt for gatherer 2's (facts) LLM call.
const FACTS_SYSTEM_PROMPT: &str = "You are Forage Buddy's species-facts assistant. Given a candidate \
species/genus name and (if available) a Wikipedia summary, report whether it is generally edible, \
medicinal, psychoactive, and/or poisonous, plus one short, practical risk note. Be honest and \
conservative: use null for any property you are not reasonably confident about rather than guessing, \
and never claim certainty. `risk_note` must be at most 140 characters \u{2014} one short, practical \
phrase, not a paragraph. Respond with ONLY a JSON object: {\"edible\": bool or null, \"medicinal\": \
bool or null, \"psychoactive\": bool or null, \"poisonous\": bool or null, \"risk_note\": string}.";

// --- Public DTOs (exact frontend JSON contract) -----------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdentificationStatus {
    Pending,
    Partial,
    Complete,
    Insufficient,
    Failed,
}

impl IdentificationStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Partial => "partial",
            Self::Complete => "complete",
            Self::Insufficient => "insufficient",
            Self::Failed => "failed",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "pending" => Some(Self::Pending),
            "partial" => Some(Self::Partial),
            "complete" => Some(Self::Complete),
            "insufficient" => Some(Self::Insufficient),
            "failed" => Some(Self::Failed),
            _ => None,
        }
    }
}

/// One condensed dangerous-lookalike entry — "no checklist arrays, no
/// paragraph notes", just a species name, a danger level, and one short
/// distinguishing phrase.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfusantSummaryDto {
    pub species: String,
    /// One of `unknown` | `mild` | `toxic` | `deadly_toxic`.
    pub danger_level: String,
    pub note: String,
    pub wikipedia_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdentificationCandidateDto {
    pub species: String,
    pub common_name: Option<String>,
    pub confidence: f32,
    pub edible: Option<bool>,
    pub medicinal: Option<bool>,
    pub psychoactive: Option<bool>,
    pub poisonous: Option<bool>,
    pub wikipedia_url: Option<String>,
    pub risk_note: String,
    pub confusants: Vec<ConfusantSummaryDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdentificationResultDto {
    pub status: IdentificationStatus,
    pub created_at: String,
    pub candidates: Vec<IdentificationCandidateDto>,
    pub missing_info: Vec<String>,
}

// --- Internal gatherer data shapes ------------------------------------------

/// One bare candidate as produced by gatherer 1, before facts/risks
/// enrichment.
#[derive(Debug, Clone)]
pub struct Candidate {
    pub species: String,
    pub common_name: Option<String>,
    pub confidence: f32,
}

/// Result of gatherer 1 ([`gather_candidates`]): raw candidates + whatever
/// else the caller needs to persist/continue the pipeline.
#[derive(Debug, Clone)]
pub struct CandidateGatherResult {
    pub model: String,
    pub photos_considered: i64,
    pub candidates: Vec<Candidate>,
    pub missing_info: Vec<String>,
}

/// Per-species facts gathered by [`gather_facts`]. `Default` represents
/// "nothing gathered yet" (used for the `partial` snapshot persisted while
/// gatherers 2+3 are still running).
#[derive(Debug, Clone, Default)]
pub struct SpeciesFacts {
    pub wikipedia_url: Option<String>,
    pub edible: Option<bool>,
    pub medicinal: Option<bool>,
    pub psychoactive: Option<bool>,
    pub poisonous: Option<bool>,
    pub risk_note: String,
}

/// A candidate with its gatherer-2/3 enrichment attached (or left at
/// defaults, for the `partial` snapshot).
#[derive(Debug, Clone)]
pub struct EnrichedCandidate {
    pub candidate: Candidate,
    pub facts: SpeciesFacts,
    pub confusants: Vec<ConfusantSummaryDto>,
}

impl EnrichedCandidate {
    fn bare(candidate: Candidate) -> Self {
        EnrichedCandidate {
            candidate,
            facts: SpeciesFacts::default(),
            confusants: Vec::new(),
        }
    }
}

// --- Small pure helpers (unit tested) ---------------------------------------

/// Char-safe truncation (never panics on a multi-byte UTF-8 boundary, unlike
/// byte slicing).
fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        s.chars().take(max).collect()
    }
}

/// Normalize a species/genus string for case-insensitive de-duplication.
fn normalize_name(s: &str) -> String {
    s.trim().to_lowercase()
}

/// `species_facts_cache.species_key` normalization: lowercased, trimmed —
/// same case-folding `species::lookup`'s matching already relies on.
fn species_key(s: &str) -> String {
    s.trim().to_lowercase()
}

/// Severity ranking for the OLD, 5-level `DangerLevel` vocabulary used by
/// curated `species_reference` rows and the internal facts cache.
fn danger_severity(level: &str) -> u8 {
    match level {
        "safe" => 1,
        "caution" => 2,
        "toxic" => 3,
        "deadly_toxic" => 4,
        _ => 0, // "unknown" or anything unrecognized
    }
}

fn severity_to_level(sev: u8) -> &'static str {
    match sev {
        4 => "deadly_toxic",
        3 => "toxic",
        2 => "caution",
        1 => "safe",
        _ => "unknown",
    }
}

/// Maps the OLD 5-level `DangerLevel` vocabulary (used by curated
/// `species_reference`/`confusant_pairs` rows, and by [`reconcile_danger`]'s
/// internal cache representation) onto the NEW, condensed 4-level
/// `danger_level` vocabulary in the frontend JSON contract
/// (`unknown|mild|toxic|deadly_toxic` — there is no separate "safe"/
/// "caution" distinction in the new confusant shape, so both collapse to
/// `mild`).
fn map_danger_level(old: &str) -> &'static str {
    match old {
        "safe" | "caution" => "mild",
        "toxic" => "toxic",
        "deadly_toxic" => "deadly_toxic",
        _ => "unknown",
    }
}

/// Reconciles a curated reference danger level (if any) with the LLM's own
/// `poisonous` boolean guess for [`gather_facts`]: curated data is a FLOOR
/// that the LLM can only raise, never lower. Returns `(final 5-level danger
/// string for the facts cache, final poisonous boolean for the API)`; the
/// boolean is `None` only when neither curated data nor the LLM said
/// anything at all.
fn reconcile_danger(
    curated_level: Option<&str>,
    llm_poisonous: Option<bool>,
) -> (String, Option<bool>) {
    let curated_sev = curated_level.map(danger_severity).unwrap_or(0);
    let llm_sev = match llm_poisonous {
        Some(true) => 3,
        Some(false) => 1,
        None => 0,
    };
    let final_sev = curated_sev.max(llm_sev);
    let final_level = severity_to_level(final_sev).to_string();

    let curated_informative = curated_sev > 0;
    let final_poisonous = if !curated_informative && llm_poisonous.is_none() {
        None
    } else {
        Some(final_sev >= 2)
    };
    (final_level, final_poisonous)
}

fn format_rfc3339(t: OffsetDateTime) -> AppResult<String> {
    t.format(&Rfc3339).map_err(|e| AppError::Internal(e.into()))
}

// --- compile_identification: pure merge/cap/sort, NOT an LLM call ----------

/// Sorts enriched candidates by confidence (desc), caps to
/// [`MAX_CANDIDATES`], and enforces every text-field length cap in the
/// frontend JSON contract.
pub fn compile_candidates(
    mut candidates: Vec<EnrichedCandidate>,
) -> Vec<IdentificationCandidateDto> {
    candidates.sort_by(|a, b| {
        b.candidate
            .confidence
            .partial_cmp(&a.candidate.confidence)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    candidates.truncate(MAX_CANDIDATES);

    candidates
        .into_iter()
        .map(|ec| IdentificationCandidateDto {
            species: truncate_chars(ec.candidate.species.trim(), TEXT_FIELD_MAX_CHARS),
            common_name: ec
                .candidate
                .common_name
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|s| truncate_chars(s, TEXT_FIELD_MAX_CHARS)),
            confidence: ec.candidate.confidence.clamp(0.0, 1.0),
            edible: ec.facts.edible,
            medicinal: ec.facts.medicinal,
            psychoactive: ec.facts.psychoactive,
            poisonous: ec.facts.poisonous,
            wikipedia_url: ec.facts.wikipedia_url,
            risk_note: truncate_chars(ec.facts.risk_note.trim(), RISK_NOTE_MAX_CHARS),
            confusants: ec
                .confusants
                .into_iter()
                .map(|cf| ConfusantSummaryDto {
                    species: truncate_chars(cf.species.trim(), TEXT_FIELD_MAX_CHARS),
                    danger_level: cf.danger_level,
                    note: truncate_chars(cf.note.trim(), CONFUSANT_NOTE_MAX_CHARS),
                    wikipedia_url: cf.wikipedia_url,
                })
                .collect(),
        })
        .collect()
}

/// Trims, caps, and drops empty `missing_info` entries.
pub fn compile_missing_info(missing_info: Vec<String>) -> Vec<String> {
    missing_info
        .into_iter()
        .map(|s| truncate_chars(s.trim(), TEXT_FIELD_MAX_CHARS))
        .filter(|s| !s.is_empty())
        .collect()
}

/// Pure merge step: combines gathered candidates (with or without
/// facts/risks enrichment attached) and missing-info into the final
/// [`IdentificationResultDto`]. NOT an LLM call.
pub fn compile_identification(
    status: IdentificationStatus,
    created_at: String,
    candidates: Vec<EnrichedCandidate>,
    missing_info: Vec<String>,
) -> IdentificationResultDto {
    IdentificationResultDto {
        status,
        created_at,
        candidates: compile_candidates(candidates),
        missing_info: compile_missing_info(missing_info),
    }
}

// --- Gatherer 1: candidates (vision call) -----------------------------------

#[derive(sqlx::FromRow)]
struct SightingForIdentification {
    lat: Option<f64>,
    lon: Option<f64>,
    place_label: Option<String>,
    observed_at: String,
    notes: Option<String>,
}

#[derive(sqlx::FromRow)]
struct PhotoForIdentification {
    file_path: String,
    content_type: String,
}

#[derive(Debug, Deserialize)]
struct RawCandidateJson {
    species: String,
    #[serde(default)]
    common_name: Option<String>,
    #[serde(default)]
    confidence: f32,
}

#[derive(Debug, Deserialize)]
struct CandidatesLlmOutput {
    #[serde(default)]
    candidates: Vec<RawCandidateJson>,
    #[serde(default)]
    missing_info: Vec<String>,
}

async fn load_prior_missing_info(db: &SqlitePool, sighting_id: &str) -> AppResult<Vec<String>> {
    let row: Option<(String,)> = sqlx::query_as(
        "SELECT missing_info_json FROM identification_results WHERE sighting_id = ? \
         ORDER BY created_at DESC LIMIT 1",
    )
    .bind(sighting_id)
    .fetch_optional(db)
    .await
    .map_err(|e| AppError::Internal(e.into()))?;

    Ok(match row {
        Some((json,)) => serde_json::from_str(&json).unwrap_or_default(),
        None => Vec::new(),
    })
}

/// Gatherer 1: a cheap vision call over the sighting's photos. Ported from
/// the old `triage::run_triage`'s logic (same honesty bias — see the
/// module-level safety note) but returns raw data instead of persisting
/// anything.
pub async fn gather_candidates(
    state: &AppState,
    sighting_id: &str,
) -> AppResult<CandidateGatherResult> {
    let sighting: Option<SightingForIdentification> = sqlx::query_as(
        "SELECT lat, lon, place_label, observed_at, notes FROM sightings WHERE id = ?",
    )
    .bind(sighting_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| AppError::Internal(e.into()))?;
    let sighting = sighting.ok_or(AppError::NotFound)?;

    let photos: Vec<PhotoForIdentification> = sqlx::query_as(
        "SELECT file_path, content_type FROM photos WHERE sighting_id = ? ORDER BY sort_order ASC",
    )
    .bind(sighting_id)
    .fetch_all(&state.db)
    .await
    .map_err(|e| AppError::Internal(e.into()))?;

    if photos.is_empty() {
        return Ok(CandidateGatherResult {
            model: NO_MODEL_CALL.to_string(),
            photos_considered: 0,
            candidates: Vec::new(),
            missing_info: vec!["at least one photo".to_string()],
        });
    }

    let prior_missing_info = load_prior_missing_info(&state.db, sighting_id).await?;

    let skip = photos.len().saturating_sub(MAX_PHOTOS_PER_ATTEMPT);
    let recent_photos: Vec<&PhotoForIdentification> = photos.iter().skip(skip).collect();

    let mut images: Vec<String> = Vec::with_capacity(recent_photos.len());
    for photo in &recent_photos {
        let path = FsPath::new(&state.config.photo_dir).join(&photo.file_path);
        match tokio::fs::read(&path).await {
            Ok(bytes) => images.push(format!(
                "data:{};base64,{}",
                photo.content_type,
                BASE64.encode(bytes)
            )),
            Err(err) => {
                tracing::warn!(
                    error = %err,
                    path = %path.display(),
                    sighting_id,
                    "identification: failed to read a stored photo from disk; skipping it"
                );
            }
        }
    }

    if images.is_empty() {
        return Ok(CandidateGatherResult {
            model: NO_MODEL_CALL.to_string(),
            photos_considered: 0,
            candidates: Vec::new(),
            missing_info: vec![
                "at least one successfully stored photo (the saved photo file(s) \
                could not be read)"
                    .to_string(),
            ],
        });
    }

    let mut user_text_parts: Vec<String> = Vec::new();
    user_text_parts.push(format!("Observed at: {}", sighting.observed_at));
    if let (Some(lat), Some(lon)) = (sighting.lat, sighting.lon) {
        user_text_parts.push(format!(
            "Location: latitude {lat:.5}, longitude {lon:.5} (raw coordinates \u{2014} use only to \
             narrow candidates by region/season, do not attempt to name or reverse-geocode the \
             place)."
        ));
    }
    if let Some(place_label) = sighting
        .place_label
        .as_deref()
        .filter(|s| !s.trim().is_empty())
    {
        user_text_parts.push(format!("User-supplied place label: {place_label}"));
    }
    if let Some(notes) = sighting.notes.as_deref().filter(|s| !s.trim().is_empty()) {
        user_text_parts.push(format!("Forager's notes: {notes}"));
    }
    if !prior_missing_info.is_empty() {
        user_text_parts.push(format!(
            "On the previous identification attempt, the user was asked to provide: {}. Check \
             whether the photo(s) below answer this before asking for something else.",
            prior_missing_info.join("; ")
        ));
    }
    user_text_parts.push(format!(
        "{} photo(s) attached (most recent {} of {} total for this sighting).",
        images.len(),
        images.len(),
        photos.len()
    ));
    let user_text = user_text_parts.join("\n\n");

    let model = state.llm.default_chat_model().to_string();
    let llm_output: CandidatesLlmOutput = state
        .llm
        .chat_json_vision(&model, CANDIDATES_SYSTEM_PROMPT, &user_text, &images)
        .await?;

    Ok(CandidateGatherResult {
        model,
        photos_considered: images.len() as i64,
        candidates: llm_output
            .candidates
            .into_iter()
            .map(|c| Candidate {
                species: c.species,
                common_name: c.common_name,
                confidence: c.confidence,
            })
            .collect(),
        missing_info: llm_output.missing_info,
    })
}

// --- Gatherer 2: facts (read-through species_facts_cache) -------------------

#[derive(sqlx::FromRow)]
struct FactsCacheRow {
    wikipedia_url: Option<String>,
    edible: Option<i64>,
    medicinal: Option<i64>,
    psychoactive: Option<i64>,
    poisonous: Option<i64>,
    risk_note: String,
}

fn int_to_bool(v: Option<i64>) -> Option<bool> {
    v.map(|n| n != 0)
}

fn bool_to_int(v: Option<bool>) -> Option<i64> {
    v.map(|b| if b { 1 } else { 0 })
}

async fn load_cached_facts(db: &SqlitePool, key: &str) -> AppResult<Option<SpeciesFacts>> {
    let row: Option<FactsCacheRow> = sqlx::query_as(
        "SELECT wikipedia_url, edible, medicinal, psychoactive, poisonous, risk_note \
         FROM species_facts_cache WHERE species_key = ?",
    )
    .bind(key)
    .fetch_optional(db)
    .await
    .map_err(|e| AppError::Internal(e.into()))?;

    Ok(row.map(|r| SpeciesFacts {
        wikipedia_url: r.wikipedia_url,
        edible: int_to_bool(r.edible),
        medicinal: int_to_bool(r.medicinal),
        psychoactive: int_to_bool(r.psychoactive),
        poisonous: int_to_bool(r.poisonous),
        risk_note: r.risk_note,
    }))
}

#[allow(clippy::too_many_arguments)]
async fn store_cached_facts(
    db: &SqlitePool,
    key: &str,
    wiki_title: &str,
    facts: &SpeciesFacts,
    danger_level: &str,
    model: &str,
) -> AppResult<()> {
    sqlx::query(
        "INSERT INTO species_facts_cache \
             (species_key, wikipedia_title, wikipedia_url, edible, medicinal, psychoactive, \
              poisonous, danger_level, risk_note, source_model, fetched_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, datetime('now')) \
         ON CONFLICT(species_key) DO UPDATE SET \
             wikipedia_title = excluded.wikipedia_title, wikipedia_url = excluded.wikipedia_url, \
             edible = excluded.edible, medicinal = excluded.medicinal, \
             psychoactive = excluded.psychoactive, poisonous = excluded.poisonous, \
             danger_level = excluded.danger_level, risk_note = excluded.risk_note, \
             source_model = excluded.source_model, fetched_at = excluded.fetched_at",
    )
    .bind(key)
    .bind(wiki_title)
    .bind(&facts.wikipedia_url)
    .bind(bool_to_int(facts.edible))
    .bind(bool_to_int(facts.medicinal))
    .bind(bool_to_int(facts.psychoactive))
    .bind(bool_to_int(facts.poisonous))
    .bind(danger_level)
    .bind(&facts.risk_note)
    .bind(model)
    .execute(db)
    .await
    .map_err(|e| AppError::Internal(e.into()))?;

    Ok(())
}

#[derive(Debug, Default, Deserialize)]
struct LlmFactsOutput {
    #[serde(default)]
    edible: Option<bool>,
    #[serde(default)]
    medicinal: Option<bool>,
    #[serde(default)]
    psychoactive: Option<bool>,
    #[serde(default)]
    poisonous: Option<bool>,
    #[serde(default)]
    risk_note: String,
}

async fn fetch_llm_facts(
    state: &AppState,
    candidate: &str,
    wiki_extract: &str,
    model: &str,
) -> LlmFactsOutput {
    let user = format!(
        "Candidate species or genus: {candidate}\nWikipedia summary: {wiki_extract}\n\nRespond \
         honestly and conservatively; use null for any property you are not reasonably confident \
         about rather than guessing."
    );
    match state
        .llm
        .chat_json::<LlmFactsOutput>(model, FACTS_SYSTEM_PROMPT, &user)
        .await
    {
        Ok(out) => out,
        Err(err) => {
            tracing::warn!(
                error = ?err,
                candidate,
                "identification: facts LLM call failed; degrading to unknown facts"
            );
            LlmFactsOutput::default()
        }
    }
}

/// Gatherer 2: per-candidate facts, read-through `species_facts_cache`.
/// Infallible by design (mirrors the old `deepdive` module's enrichment
/// steps) — any lookup/network/LLM failure degrades to best-effort/unknown
/// data rather than propagating, so one candidate's facts failure never
/// blocks the rest of the pipeline.
pub async fn gather_facts(state: &AppState, candidate_species: &str) -> SpeciesFacts {
    let key = species_key(candidate_species);

    match load_cached_facts(&state.db, &key).await {
        Ok(Some(cached)) => return cached,
        Ok(None) => {}
        Err(err) => tracing::warn!(
            error = ?err,
            candidate_species,
            "identification: facts cache read failed; proceeding to gather fresh"
        ),
    }

    let curated = match species::lookup(&state.db, candidate_species).await {
        Ok(c) => c,
        Err(err) => {
            tracing::warn!(
                error = ?err,
                candidate_species,
                "identification: curated species lookup failed"
            );
            None
        }
    };

    let wiki_title = curated
        .as_ref()
        .and_then(|c| c.wikipedia_title.clone())
        .unwrap_or_else(|| candidate_species.to_string());
    let http = wikipedia::build_client();
    let wiki_page = match wikipedia::fetch_or_cache(&state.db, &http, &wiki_title).await {
        Ok(p) => p,
        Err(err) => {
            tracing::warn!(
                error = ?err,
                candidate_species,
                "identification: wikipedia fetch failed; proceeding ungrounded"
            );
            None
        }
    };

    let wikipedia_url = wiki_page.as_ref().map(|p| p.url.clone());
    let wiki_extract = wiki_page
        .as_ref()
        .map(|p| p.extract.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("(no Wikipedia article could be retrieved for this candidate)");

    let model = state.llm.identification_chat_model();
    let llm = fetch_llm_facts(state, candidate_species, wiki_extract, model).await;

    let curated_level = curated.as_ref().map(|c| c.danger_level.as_str());
    let (final_level, final_poisonous) = reconcile_danger(curated_level, llm.poisonous);
    let risk_note = truncate_chars(llm.risk_note.trim(), RISK_NOTE_MAX_CHARS);

    let facts = SpeciesFacts {
        wikipedia_url,
        edible: llm.edible,
        medicinal: llm.medicinal,
        psychoactive: llm.psychoactive,
        poisonous: final_poisonous,
        risk_note,
    };

    if let Err(err) =
        store_cached_facts(&state.db, &key, &wiki_title, &facts, &final_level, model).await
    {
        tracing::warn!(
            error = ?err,
            candidate_species,
            "identification: failed to write-through the facts cache"
        );
    }

    facts
}

// --- Gatherer 3: risks (curated confusants + LLM enrichment) ---------------

#[derive(Debug, Deserialize)]
struct ExtraConfusant {
    species: String,
    #[serde(default)]
    notes: String,
}

/// Best-known Wikipedia title for a species/genus, degrading to the bare
/// name on any lookup failure (never propagates — this is purely for
/// best-guess link construction / search grounding, never a source of
/// truth).
async fn best_wikipedia_title(db: &SqlitePool, species_or_genus: &str) -> String {
    match species::lookup(db, species_or_genus).await {
        Ok(Some(c)) => c
            .wikipedia_title
            .unwrap_or_else(|| species_or_genus.to_string()),
        _ => species_or_genus.to_string(),
    }
}

/// Best-guess Wikipedia URL for a species name: curated title if known,
/// else the species name itself, via [`wikipedia::page_url`]. Not
/// network-verified (unlike [`gather_facts`]'s grounding) — constructing a
/// URL for every confusant would mean a live fetch per confusant, which
/// isn't worth the latency for a "nice to have" link.
async fn wikipedia_url_guess(db: &SqlitePool, species_name: &str) -> Option<String> {
    let title = best_wikipedia_title(db, species_name).await;
    let title = title.trim();
    if title.is_empty() {
        None
    } else {
        Some(wikipedia::page_url(title))
    }
}

/// Best-effort embed of a fetched Wikipedia page's chunks into the
/// `wikipedia`-kind vector store, so the `search_wikipedia` tool call below
/// has something to search. Every failure is logged and skipped, matching
/// the old `deepdive` module's resilience pattern.
async fn embed_and_store_wikipedia_chunks(state: &AppState, page: &wikipedia::WikipediaPage) {
    for chunk in wikipedia::chunk_extract(&page.extract) {
        let embedding = match state.llm.embed(&chunk).await {
            Ok(v) => v,
            Err(err) => {
                tracing::warn!(
                    error = ?err,
                    title = %page.title,
                    "identification: failed to embed a wikipedia chunk; skipping"
                );
                continue;
            }
        };

        if let Err(err) = crate::vector::upsert(
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
                "identification: failed to store a wikipedia embedding; skipping"
            );
        }
    }
}

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

    crate::vector::search(&state.db, "wikipedia", &embedding, SEARCH_TOP_K)
        .await
        .unwrap_or_default()
}

/// Pull the first `[ … ]` JSON array out of `content`, tolerating prose or
/// code fences around it.
fn extract_json_array(content: &str) -> Option<&str> {
    let start = content.find('[')?;
    let end = content.rfind(']')?;
    if end < start {
        return None;
    }
    content.get(start..=end)
}

/// Ported from the old `deepdive` module's `find_extra_confusants`: one
/// bounded LLM tool-calling round offering a `search_wikipedia` tool, asking
/// for ADDITIONAL look-alikes not already in `curated`. Any error, timeout,
/// or unparseable response yields an empty list — curated confusants are
/// never at risk here since the caller merges them in regardless.
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
        {\"species\": string, \"notes\": string}. Keep notes under 100 characters \u{2014} one short \
        distinguishing phrase, not a checklist. Do not include a danger_level field \u{2014} it is \
        determined separately from trusted reference data, not from your output.";

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

    let model = state.llm.identification_chat_model();

    let first = match state.llm.chat_tools(model, messages.clone(), &tools).await {
        Ok(turn) => turn,
        Err(err) => {
            tracing::warn!(
                error = ?err,
                candidate,
                "identification: confusant-enrichment tool call failed; proceeding with curated \
                 confusants only"
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

        match state.llm.chat_tools(model, messages, &tools).await {
            Ok(second) => second.content,
            Err(err) => {
                tracing::warn!(
                    error = ?err,
                    candidate,
                    "identification: follow-up confusant-enrichment call failed; proceeding with \
                     curated confusants only"
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

async fn confusant_summary_from_curated(
    db: &SqlitePool,
    c: &species::ConfusantDto,
) -> ConfusantSummaryDto {
    let species_name = match &c.species {
        Some(sp) => format!("{} {}", c.genus, sp),
        None => c.genus.clone(),
    };
    let wikipedia_url = wikipedia_url_guess(db, &species_name).await;
    ConfusantSummaryDto {
        species: species_name,
        danger_level: map_danger_level(&c.danger_level).to_string(),
        note: truncate_chars(c.notes.trim(), CONFUSANT_NOTE_MAX_CHARS),
        wikipedia_url,
    }
}

async fn confusant_summary_from_extra(
    db: &SqlitePool,
    extra: ExtraConfusant,
) -> ConfusantSummaryDto {
    let danger_level = match species::lookup(db, &extra.species).await {
        Ok(Some(row)) => map_danger_level(&row.danger_level).to_string(),
        _ => "unknown".to_string(),
    };
    let wikipedia_url = wikipedia_url_guess(db, &extra.species).await;
    ConfusantSummaryDto {
        species: extra.species,
        danger_level,
        note: truncate_chars(extra.notes.trim(), CONFUSANT_NOTE_MAX_CHARS),
        wikipedia_url,
    }
}

/// Merge curated confusants (always kept) with model-identified extras,
/// deduped by normalized species name (curated wins on overlap). Ported
/// from the old `deepdive` module's `merge_confusants`.
async fn merge_confusants(
    state: &AppState,
    curated: Vec<species::ConfusantDto>,
    extras: Vec<ExtraConfusant>,
) -> Vec<ConfusantSummaryDto> {
    let mut merged = Vec::with_capacity(curated.len() + extras.len());
    let mut seen: HashSet<String> = HashSet::new();

    for c in &curated {
        let summary = confusant_summary_from_curated(&state.db, c).await;
        seen.insert(normalize_name(&summary.species));
        merged.push(summary);
    }

    for extra in extras {
        let norm = normalize_name(&extra.species);
        if seen.contains(&norm) {
            continue;
        }
        let summary = confusant_summary_from_extra(&state.db, extra).await;
        seen.insert(norm);
        merged.push(summary);
    }

    merged
}

/// Gatherer 3: per-candidate dangerous look-alikes. Infallible by design,
/// same rationale as [`gather_facts`] — curated data is always attempted
/// first and any downstream enrichment failure just means fewer extras, not
/// an error.
pub async fn gather_risks(state: &AppState, candidate_species: &str) -> Vec<ConfusantSummaryDto> {
    let curated = match species::curated_confusants_for(&state.db, candidate_species).await {
        Ok(c) => c,
        Err(err) => {
            tracing::warn!(
                error = ?err,
                candidate_species,
                "identification: curated confusant lookup failed; proceeding with none"
            );
            Vec::new()
        }
    };

    // Best-effort Wikipedia grounding so the search_wikipedia tool below has
    // something to search; a dead/unreachable Wikipedia must never break
    // this gatherer.
    let wiki_title = best_wikipedia_title(&state.db, candidate_species).await;
    let http = wikipedia::build_client();
    if let Ok(Some(page)) = wikipedia::fetch_or_cache(&state.db, &http, &wiki_title).await {
        embed_and_store_wikipedia_chunks(state, &page).await;
    }

    let extras = find_extra_confusants(state, candidate_species, &curated).await;
    merge_confusants(state, curated, extras).await
}

// --- Orchestrator ------------------------------------------------------------

async fn enrich_candidate(state: &AppState, candidate: Candidate) -> EnrichedCandidate {
    let (facts, confusants) = tokio::join!(
        gather_facts(state, &candidate.species),
        gather_risks(state, &candidate.species),
    );
    EnrichedCandidate {
        candidate,
        facts,
        confusants,
    }
}

async fn insert_pending_row(
    db: &SqlitePool,
    id: &str,
    sighting_id: &str,
    now: &str,
) -> AppResult<()> {
    sqlx::query(
        "INSERT INTO identification_results \
             (id, sighting_id, created_at, updated_at, status, model, candidates_json, \
              missing_info_json, photos_considered) \
         VALUES (?, ?, ?, ?, 'pending', 'pending', '[]', '[]', 0)",
    )
    .bind(id)
    .bind(sighting_id)
    .bind(now)
    .bind(now)
    .execute(db)
    .await
    .map_err(|e| AppError::Internal(e.into()))?;
    Ok(())
}

async fn update_row(
    db: &SqlitePool,
    id: &str,
    status: IdentificationStatus,
    model: &str,
    dto: &IdentificationResultDto,
    photos_considered: i64,
) -> AppResult<()> {
    let now = format_rfc3339(OffsetDateTime::now_utc())?;
    let candidates_json =
        serde_json::to_string(&dto.candidates).map_err(|e| AppError::Internal(e.into()))?;
    let missing_info_json =
        serde_json::to_string(&dto.missing_info).map_err(|e| AppError::Internal(e.into()))?;

    sqlx::query(
        "UPDATE identification_results \
         SET updated_at = ?, status = ?, model = ?, candidates_json = ?, missing_info_json = ?, \
             photos_considered = ? \
         WHERE id = ?",
    )
    .bind(&now)
    .bind(status.as_str())
    .bind(model)
    .bind(&candidates_json)
    .bind(&missing_info_json)
    .bind(photos_considered)
    .bind(id)
    .execute(db)
    .await
    .map_err(|e| AppError::Internal(e.into()))?;
    Ok(())
}

/// Marks a row `failed`, best-effort. Used on any gatherer/persist error so
/// a row is never left stuck at `pending`/`partial` forever (this app has a
/// real history of "stuck forever" bugs).
async fn mark_failed(db: &SqlitePool, id: &str) -> AppResult<()> {
    let now = format_rfc3339(OffsetDateTime::now_utc())?;
    sqlx::query("UPDATE identification_results SET status = 'failed', updated_at = ? WHERE id = ?")
        .bind(&now)
        .bind(id)
        .execute(db)
        .await
        .map_err(|e| AppError::Internal(e.into()))?;
    Ok(())
}

/// Writes `dto` to the row; on any failure to write it, falls back to
/// marking the row `failed` (best-effort) before propagating the original
/// error, so the row is never left stuck.
async fn finalize(
    db: &SqlitePool,
    id: &str,
    model: &str,
    photos_considered: i64,
    dto: &IdentificationResultDto,
) -> AppResult<()> {
    match update_row(db, id, dto.status, model, dto, photos_considered).await {
        Ok(()) => Ok(()),
        Err(err) => {
            if let Err(fallback_err) = mark_failed(db, id).await {
                tracing::error!(
                    error = ?fallback_err,
                    id,
                    "identification: failed to mark row failed after a finalize error"
                );
            }
            Err(err)
        }
    }
}

/// Orchestrates the full pipeline for a sighting: inserts a `pending` row
/// immediately, runs gatherer 1, updates the row (`partial` or
/// `insufficient`), then for the top candidates runs gatherers 2+3
/// concurrently per candidate, and finally compiles + updates the row to
/// `complete` (or `failed` on an unrecoverable error at any step).
pub async fn run_identification(state: &AppState, sighting_id: &str) -> AppResult<()> {
    // Verify the sighting exists before creating any row for it — avoids a
    // dangling identification_results row for a bogus id.
    let exists: Option<(String,)> = sqlx::query_as("SELECT id FROM sightings WHERE id = ?")
        .bind(sighting_id)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| AppError::Internal(e.into()))?;
    if exists.is_none() {
        return Err(AppError::NotFound);
    }

    let id = Uuid::new_v4().to_string();
    let created_at = format_rfc3339(OffsetDateTime::now_utc())?;
    insert_pending_row(&state.db, &id, sighting_id, &created_at).await?;

    let gather1 = gather_candidates(state, sighting_id).await;
    let candidate_result = match gather1 {
        Ok(r) => r,
        Err(err) => {
            tracing::error!(error = ?err, sighting_id, "identification: gatherer 1 failed");
            if let Err(fallback_err) = mark_failed(&state.db, &id).await {
                tracing::error!(
                    error = ?fallback_err,
                    sighting_id,
                    "identification: failed to mark row failed after gatherer-1 error"
                );
            }
            return Err(err);
        }
    };

    if candidate_result.candidates.is_empty() {
        let dto = compile_identification(
            IdentificationStatus::Insufficient,
            created_at,
            Vec::new(),
            candidate_result.missing_info,
        );
        return finalize(
            &state.db,
            &id,
            &candidate_result.model,
            candidate_result.photos_considered,
            &dto,
        )
        .await;
    }

    let mut ordered = candidate_result.candidates;
    ordered.sort_by(|a, b| {
        b.confidence
            .partial_cmp(&a.confidence)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    ordered.truncate(MAX_CANDIDATES);

    // Mark `partial` immediately so a slow enrichment phase never looks
    // stuck from the outside.
    let partial_dto = compile_identification(
        IdentificationStatus::Partial,
        created_at.clone(),
        ordered
            .iter()
            .cloned()
            .map(EnrichedCandidate::bare)
            .collect(),
        candidate_result.missing_info.clone(),
    );
    finalize(
        &state.db,
        &id,
        &candidate_result.model,
        candidate_result.photos_considered,
        &partial_dto,
    )
    .await?;

    let enriched: Vec<EnrichedCandidate> =
        futures::future::join_all(ordered.into_iter().map(|c| enrich_candidate(state, c))).await;

    let final_dto = compile_identification(
        IdentificationStatus::Complete,
        created_at,
        enriched,
        candidate_result.missing_info,
    );
    finalize(
        &state.db,
        &id,
        &candidate_result.model,
        candidate_result.photos_considered,
        &final_dto,
    )
    .await
}

// --- Queries / HTTP -----------------------------------------------------------

#[derive(sqlx::FromRow)]
struct IdentificationRow {
    created_at: String,
    status: String,
    candidates_json: String,
    missing_info_json: String,
}

fn row_to_dto(row: IdentificationRow) -> AppResult<IdentificationResultDto> {
    let status = IdentificationStatus::parse(&row.status).ok_or_else(|| {
        AppError::Internal(anyhow::anyhow!(
            "identification_results row has invalid status {:?}",
            row.status
        ))
    })?;
    let candidates: Vec<IdentificationCandidateDto> =
        serde_json::from_str(&row.candidates_json).map_err(|e| AppError::Internal(e.into()))?;
    let missing_info: Vec<String> =
        serde_json::from_str(&row.missing_info_json).map_err(|e| AppError::Internal(e.into()))?;

    Ok(IdentificationResultDto {
        status,
        created_at: row.created_at,
        candidates,
        missing_info,
    })
}

/// Latest identification attempt for a sighting, if any.
pub async fn get_latest(
    db: &SqlitePool,
    sighting_id: &str,
) -> AppResult<Option<IdentificationResultDto>> {
    let row: Option<IdentificationRow> = sqlx::query_as(
        "SELECT created_at, status, candidates_json, missing_info_json \
         FROM identification_results WHERE sighting_id = ? ORDER BY created_at DESC LIMIT 1",
    )
    .bind(sighting_id)
    .fetch_optional(db)
    .await
    .map_err(|e| AppError::Internal(e.into()))?;

    row.map(row_to_dto).transpose()
}

/// Full identification history for a sighting, newest first.
pub async fn get_history(
    db: &SqlitePool,
    sighting_id: &str,
) -> AppResult<Vec<IdentificationResultDto>> {
    let rows: Vec<IdentificationRow> = sqlx::query_as(
        "SELECT created_at, status, candidates_json, missing_info_json \
         FROM identification_results WHERE sighting_id = ? ORDER BY created_at DESC",
    )
    .bind(sighting_id)
    .fetch_all(db)
    .await
    .map_err(|e| AppError::Internal(e.into()))?;

    rows.into_iter().map(row_to_dto).collect()
}

/// `NotFound` unless `sighting_id` exists and is owned by `user_id`.
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

async fn list_identification(
    State(state): State<AppState>,
    RequireAuth(user_id): RequireAuth,
    Path(sighting_id): Path<String>,
) -> AppResult<Json<Vec<IdentificationResultDto>>> {
    ensure_owned(&state.db, &sighting_id, &user_id).await?;
    let history = get_history(&state.db, &sighting_id).await?;
    Ok(Json(history))
}

async fn rerun_identification(
    State(state): State<AppState>,
    RequireAuth(user_id): RequireAuth,
    Path(sighting_id): Path<String>,
) -> AppResult<Json<IdentificationResultDto>> {
    ensure_owned(&state.db, &sighting_id, &user_id).await?;
    run_identification(&state, &sighting_id).await?;
    let result = get_latest(&state.db, &sighting_id)
        .await?
        .ok_or(AppError::NotFound)?;
    Ok(Json(result))
}

pub fn router() -> Router<AppState> {
    Router::new().route(
        "/api/sightings/{id}/identification",
        get(list_identification).post(rerun_identification),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- compile_candidates: sorting + capping + length caps ---------------

    fn candidate(species: &str, confidence: f32) -> Candidate {
        Candidate {
            species: species.to_string(),
            common_name: None,
            confidence,
        }
    }

    #[test]
    fn compile_candidates_sorts_by_confidence_desc() {
        let input = vec![
            EnrichedCandidate::bare(candidate("Low", 0.1)),
            EnrichedCandidate::bare(candidate("High", 0.9)),
            EnrichedCandidate::bare(candidate("Mid", 0.5)),
        ];
        let out = compile_candidates(input);
        let names: Vec<&str> = out.iter().map(|c| c.species.as_str()).collect();
        assert_eq!(names, vec!["High", "Mid", "Low"]);
    }

    #[test]
    fn compile_candidates_caps_to_max_candidates() {
        let input: Vec<EnrichedCandidate> = (0..10)
            .map(|i| EnrichedCandidate::bare(candidate(&format!("Species{i}"), i as f32 / 10.0)))
            .collect();
        let out = compile_candidates(input);
        assert_eq!(out.len(), MAX_CANDIDATES);
        // Kept the highest-confidence ones.
        assert_eq!(out.first().map(|c| c.species.as_str()), Some("Species9"));
    }

    #[test]
    fn compile_candidates_truncates_risk_note_and_confusant_note() {
        let mut ec = EnrichedCandidate::bare(candidate("Amanita phalloides", 0.9));
        ec.facts.risk_note = "x".repeat(500);
        ec.confusants.push(ConfusantSummaryDto {
            species: "Agaricus bisporus".to_string(),
            danger_level: "mild".to_string(),
            note: "y".repeat(500),
            wikipedia_url: None,
        });

        let out = compile_candidates(vec![ec]);
        let only = out.into_iter().next().expect("one candidate");
        assert_eq!(only.risk_note.chars().count(), RISK_NOTE_MAX_CHARS);
        assert_eq!(
            only.confusants
                .first()
                .expect("one confusant")
                .note
                .chars()
                .count(),
            CONFUSANT_NOTE_MAX_CHARS
        );
    }

    #[test]
    fn compile_candidates_clamps_confidence_to_unit_range() {
        let ec = EnrichedCandidate::bare(candidate("Overconfident", 5.0));
        let out = compile_candidates(vec![ec]);
        assert_eq!(out.first().map(|c| c.confidence), Some(1.0));
    }

    #[test]
    fn compile_missing_info_drops_empty_and_truncates() {
        let input = vec![
            "  a real ask  ".to_string(),
            "   ".to_string(),
            "z".repeat(500),
        ];
        let out = compile_missing_info(input);
        assert_eq!(out.len(), 2);
        assert_eq!(out.first().map(String::as_str), Some("a real ask"));
        assert_eq!(
            out.get(1).map(|s| s.chars().count()),
            Some(TEXT_FIELD_MAX_CHARS)
        );
    }

    // --- reconcile_danger: curated floor wins over the LLM ------------------

    #[test]
    fn reconcile_danger_never_downgrades_a_curated_deadly_entry() {
        let (level, poisonous) = reconcile_danger(Some("deadly_toxic"), Some(false));
        assert_eq!(level, "deadly_toxic");
        assert_eq!(poisonous, Some(true));
    }

    #[test]
    fn reconcile_danger_lets_the_llm_upgrade_a_curated_safe_entry() {
        let (level, poisonous) = reconcile_danger(Some("safe"), Some(true));
        assert_eq!(level, "toxic");
        assert_eq!(poisonous, Some(true));
    }

    #[test]
    fn reconcile_danger_uses_llm_alone_with_no_curated_data() {
        let (level, poisonous) = reconcile_danger(None, Some(true));
        assert_eq!(level, "toxic");
        assert_eq!(poisonous, Some(true));

        let (level, poisonous) = reconcile_danger(None, Some(false));
        assert_eq!(level, "safe");
        assert_eq!(poisonous, Some(false));
    }

    #[test]
    fn reconcile_danger_stays_unknown_when_both_are_silent() {
        let (level, poisonous) = reconcile_danger(None, None);
        assert_eq!(level, "unknown");
        assert_eq!(poisonous, None);
    }

    #[test]
    fn reconcile_danger_respects_curated_safe_when_llm_is_silent() {
        let (level, poisonous) = reconcile_danger(Some("safe"), None);
        assert_eq!(level, "safe");
        assert_eq!(poisonous, Some(false));
    }

    #[test]
    fn map_danger_level_collapses_safe_and_caution_to_mild() {
        assert_eq!(map_danger_level("safe"), "mild");
        assert_eq!(map_danger_level("caution"), "mild");
        assert_eq!(map_danger_level("toxic"), "toxic");
        assert_eq!(map_danger_level("deadly_toxic"), "deadly_toxic");
        assert_eq!(map_danger_level("anything-else"), "unknown");
    }

    #[test]
    fn truncate_chars_is_utf8_safe() {
        let s = "héllo wörld";
        let truncated = truncate_chars(s, 5);
        assert_eq!(truncated.chars().count(), 5);
    }

    // --- Orchestrator / query integration tests -----------------------------

    async fn test_state() -> (tempfile::TempDir, AppState) {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let db = crate::db::init_pool(db_path.to_str().unwrap())
            .await
            .unwrap();

        sqlx::query(
            "INSERT INTO users (id, email, display_name, created_at) \
             VALUES ('u1', NULL, NULL, '2026-01-01T00:00:00Z')",
        )
        .execute(&db)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO sightings (id, user_id, status, observed_at, created_at, updated_at) \
             VALUES ('s1', 'u1', 'open', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z', \
                      '2026-01-01T00:00:00Z')",
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
            http_client: reqwest::Client::new(),
            identification_semaphore: std::sync::Arc::new(tokio::sync::Semaphore::new(1)),
        };
        (dir, state)
    }

    #[tokio::test]
    async fn run_identification_errors_not_found_for_missing_sighting() {
        let (_dir, state) = test_state().await;
        let err = run_identification(&state, "does-not-exist")
            .await
            .unwrap_err();
        assert!(matches!(err, AppError::NotFound));
    }

    #[tokio::test]
    async fn zero_photos_yields_insufficient_without_calling_llm() {
        let (_dir, state) = test_state().await;

        run_identification(&state, "s1").await.unwrap();

        let latest = get_latest(&state.db, "s1").await.unwrap().expect("a row");
        assert_eq!(latest.status, IdentificationStatus::Insufficient);
        assert!(latest.candidates.is_empty());
        assert_eq!(latest.missing_info, vec!["at least one photo".to_string()]);
    }

    #[tokio::test]
    async fn zero_photos_attempt_is_persisted_in_history() {
        let (_dir, state) = test_state().await;
        run_identification(&state, "s1").await.unwrap();
        let history = get_history(&state.db, "s1").await.unwrap();
        assert_eq!(history.len(), 1);
    }

    #[tokio::test]
    async fn get_latest_returns_none_with_no_attempts() {
        let (_dir, state) = test_state().await;
        assert!(get_latest(&state.db, "s1").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn facts_cache_roundtrips_booleans_and_text() {
        let (_dir, state) = test_state().await;
        let facts = SpeciesFacts {
            wikipedia_url: Some("https://en.wikipedia.org/wiki/Example".to_string()),
            edible: Some(true),
            medicinal: Some(false),
            psychoactive: None,
            poisonous: Some(false),
            risk_note: "a short note".to_string(),
        };
        store_cached_facts(
            &state.db,
            "example genus",
            "Example",
            &facts,
            "safe",
            "test-model",
        )
        .await
        .unwrap();

        let cached = load_cached_facts(&state.db, "example genus")
            .await
            .unwrap()
            .expect("cached facts");
        assert_eq!(cached.edible, Some(true));
        assert_eq!(cached.medicinal, Some(false));
        assert_eq!(cached.psychoactive, None);
        assert_eq!(cached.poisonous, Some(false));
        assert_eq!(cached.risk_note, "a short note");
        assert_eq!(
            cached.wikipedia_url,
            Some("https://en.wikipedia.org/wiki/Example".to_string())
        );
    }

    // --- Migration 0011 backfill JSON shape ---------------------------------

    mod backfill {
        use super::*;
        use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};

        const MIGRATIONS_BEFORE_BACKFILL: &[&str] = &[
            include_str!("../../migrations/0001_init.sql"),
            include_str!("../../migrations/0002_sightings.sql"),
            include_str!("../../migrations/0003_photos.sql"),
            include_str!("../../migrations/0004_triage.sql"),
            include_str!("../../migrations/0005_species_reference.sql"),
            include_str!("../../migrations/0006_wikipedia.sql"),
            include_str!("../../migrations/0007_vectors.sql"),
            include_str!("../../migrations/0008_deepdive.sql"),
            include_str!("../../migrations/0009_identification.sql"),
            include_str!("../../migrations/0010_species_facts_cache.sql"),
        ];
        const BACKFILL_MIGRATION: &str =
            include_str!("../../migrations/0011_identification_backfill.sql");

        async fn pool_before_backfill() -> (tempfile::TempDir, SqlitePool) {
            let dir = tempfile::tempdir().unwrap();
            let db_path = dir.path().join("test.db");
            let options = SqliteConnectOptions::new()
                .filename(&db_path)
                .create_if_missing(true);
            let pool = SqlitePoolOptions::new()
                .connect_with(options)
                .await
                .unwrap();
            for sql in MIGRATIONS_BEFORE_BACKFILL {
                sqlx::raw_sql(sql).execute(&pool).await.unwrap();
            }

            sqlx::query(
                "INSERT INTO users (id, email, display_name, created_at) \
                 VALUES ('u1', NULL, NULL, '2026-01-01T00:00:00Z')",
            )
            .execute(&pool)
            .await
            .unwrap();

            (dir, pool)
        }

        async fn run_backfill(pool: &SqlitePool) {
            sqlx::raw_sql(BACKFILL_MIGRATION)
                .execute(pool)
                .await
                .unwrap();
        }

        async fn insert_sighting(pool: &SqlitePool, id: &str) {
            sqlx::query(
                "INSERT INTO sightings (id, user_id, status, observed_at, created_at, updated_at) \
                 VALUES (?, 'u1', 'open', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z', \
                          '2026-01-01T00:00:00Z')",
            )
            .bind(id)
            .execute(pool)
            .await
            .unwrap();
        }

        #[tokio::test]
        async fn backfills_insufficient_triage_as_insufficient() {
            let (_dir, pool) = pool_before_backfill().await;
            insert_sighting(&pool, "s-insufficient").await;
            sqlx::query(
                "INSERT INTO triage_results \
                     (id, sighting_id, created_at, model, status, genus, candidate_species_json, \
                      missing_info_json, reasoning, photos_considered) \
                 VALUES ('t1', 's-insufficient', '2026-02-01T00:00:00Z', 'none', 'insufficient', \
                          NULL, '[]', '[\"a clear photo\"]', 'not enough to go on', 0)",
            )
            .execute(&pool)
            .await
            .unwrap();

            run_backfill(&pool).await;

            let row: (String, String, String) = sqlx::query_as(
                "SELECT status, candidates_json, missing_info_json FROM identification_results \
                 WHERE sighting_id = 's-insufficient'",
            )
            .fetch_one(&pool)
            .await
            .unwrap();
            assert_eq!(row.0, "insufficient");
            assert_eq!(row.1, "[]");
            let missing: Vec<String> = serde_json::from_str(&row.2).unwrap();
            assert_eq!(missing, vec!["a clear photo".to_string()]);
        }

        #[tokio::test]
        async fn backfills_triage_only_genus_candidate_as_partial() {
            let (_dir, pool) = pool_before_backfill().await;
            insert_sighting(&pool, "s-partial").await;
            sqlx::query(
                "INSERT INTO triage_results \
                     (id, sighting_id, created_at, model, status, genus, candidate_species_json, \
                      missing_info_json, reasoning, photos_considered) \
                 VALUES ('t1', 's-partial', '2026-02-01T00:00:00Z', 'test-model', \
                          'species_candidate', 'Amanita', \
                          '[{\"species\":\"Amanita phalloides\",\"common_name\":\"Death cap\",\
                          \"confidence\":0.6}]', '[]', 'looks like Amanita', 1)",
            )
            .execute(&pool)
            .await
            .unwrap();

            run_backfill(&pool).await;

            let row: (String, String) = sqlx::query_as(
                "SELECT status, candidates_json FROM identification_results WHERE sighting_id = 's-partial'",
            )
            .fetch_one(&pool)
            .await
            .unwrap();
            assert_eq!(row.0, "partial");

            #[derive(Deserialize)]
            struct C {
                species: String,
                common_name: Option<String>,
                confidence: f64,
                edible: Option<bool>,
                confusants: Vec<serde_json::Value>,
            }
            let candidates: Vec<C> = serde_json::from_str(&row.1).unwrap();
            assert_eq!(candidates.len(), 1);
            let c = &candidates[0];
            assert_eq!(c.species, "Amanita phalloides");
            assert_eq!(c.common_name.as_deref(), Some("Death cap"));
            assert_eq!(c.confidence, 0.6);
            assert_eq!(c.edible, None);
            assert!(c.confusants.is_empty());
        }

        #[tokio::test]
        async fn backfills_sighting_with_deepdive_as_complete_with_mapped_confusants() {
            let (_dir, pool) = pool_before_backfill().await;
            insert_sighting(&pool, "s-complete").await;
            sqlx::query(
                "INSERT INTO triage_results \
                     (id, sighting_id, created_at, model, status, genus, candidate_species_json, \
                      missing_info_json, reasoning, photos_considered) \
                 VALUES ('t1', 's-complete', '2026-02-01T00:00:00Z', 'test-model', \
                          'species_candidate', 'Amanita', \
                          '[{\"species\":\"Amanita phalloides\",\"common_name\":\"Death cap\",\
                          \"confidence\":0.6}]', '[]', 'looks like Amanita', 1)",
            )
            .execute(&pool)
            .await
            .unwrap();
            sqlx::query(
                "INSERT INTO deepdive_results \
                     (id, sighting_id, created_at, model, best_match_species, confidence, \
                      wikipedia_title, wikipedia_url, wikipedia_extract, confusants_json, \
                      safety_notes) \
                 VALUES ('d1', 's-complete', '2026-02-02T00:00:00Z', 'test-model', \
                          'Amanita phalloides', 0.6, 'Amanita phalloides', \
                          'https://en.wikipedia.org/wiki/Amanita_phalloides', 'deadly mushroom', \
                          '[{\"species\":\"Agaricus bisporus\",\"common_name\":\"button mushroom\",\
                          \"danger_level\":\"safe\",\"distinguishing_features\":[\"check the volva\"],\
                          \"notes\":\"look for a volva at the base\"}]', \
                          'Never eat anything based on this app alone.')",
            )
            .execute(&pool)
            .await
            .unwrap();

            run_backfill(&pool).await;

            let row: (String, String, String) = sqlx::query_as(
                "SELECT status, created_at, candidates_json FROM identification_results \
                 WHERE sighting_id = 's-complete'",
            )
            .fetch_one(&pool)
            .await
            .unwrap();
            assert_eq!(row.0, "complete");
            // created_at comes from the deepdive row, not the triage row.
            assert_eq!(row.1, "2026-02-02T00:00:00Z");

            #[derive(Deserialize)]
            struct Confusant {
                species: String,
                danger_level: String,
                note: String,
            }
            #[derive(Deserialize)]
            struct C {
                species: String,
                common_name: Option<String>,
                confidence: f64,
                wikipedia_url: Option<String>,
                risk_note: String,
                confusants: Vec<Confusant>,
            }
            let candidates: Vec<C> = serde_json::from_str(&row.2).unwrap();
            assert_eq!(candidates.len(), 1);
            let c = &candidates[0];
            assert_eq!(c.species, "Amanita phalloides");
            assert_eq!(c.common_name.as_deref(), Some("Death cap"));
            assert_eq!(c.confidence, 0.6);
            assert_eq!(
                c.wikipedia_url.as_deref(),
                Some("https://en.wikipedia.org/wiki/Amanita_phalloides")
            );
            assert_eq!(c.risk_note, "Never eat anything based on this app alone.");
            assert_eq!(c.confusants.len(), 1);
            // old "safe" danger_level maps onto the new "mild" vocabulary.
            assert_eq!(c.confusants[0].species, "Agaricus bisporus");
            assert_eq!(c.confusants[0].danger_level, "mild");
            assert_eq!(c.confusants[0].note, "look for a volva at the base");
        }

        #[tokio::test]
        async fn backfill_only_touches_sightings_with_a_triage_row() {
            let (_dir, pool) = pool_before_backfill().await;
            insert_sighting(&pool, "s-no-triage").await;

            run_backfill(&pool).await;

            let count: (i64,) = sqlx::query_as(
                "SELECT COUNT(*) FROM identification_results WHERE sighting_id = 's-no-triage'",
            )
            .fetch_one(&pool)
            .await
            .unwrap();
            assert_eq!(count.0, 0);
        }
    }
}
