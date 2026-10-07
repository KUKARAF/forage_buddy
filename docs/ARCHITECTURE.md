# Forage Buddy — Architecture & Module Contract

> **Working name.** An AI foraging helper: you photograph a plant/fungus/etc.
> in the field, the app records photo + GPS location + timestamp, and ONE
> automatic "identification" pipeline runs on every photo upload: it finds
> candidate species (or honestly says "I don't know, send more photos" when
> it can't), grounds each top candidate in its Wikipedia article, flags
> edible/medicinal/psychoactive/poisonous facts, and surfaces dangerous
> look-alikes ("confusant species") with a short note on how to tell them
> apart. There is no separate manually-triggered second stage — the whole
> pipeline is one automatic call, internally broken into three gatherer
> stages (candidates, facts, risks) run by the `identification` module.
>
> **Safety is the entire point of this pipeline.** Misidentifying a mushroom
> or plant can kill. Every LLM prompt in this app, every API response, and
> every screen that shows a species guess MUST carry a prominent disclaimer:
> this is a research aid, not a field guide substitute, and nothing it
> identifies should be eaten/used on the strength of its output alone. Never
> soften or omit this.

## Why this stack (mirrors `ai_buddy`, the sibling project)

Same philosophy as `ai_buddy`/`rust_note`: one Rust binary, SQLite, no extra
services (no vector DB, no job queue, no Redis). Brute-force cosine search
over a `BLOB` embeddings table is plenty at personal-project scale.

- **Backend:** Rust — axum 0.8, tokio, sqlx + SQLite (WAL), tower-sessions
  (SQLite store).
- **Auth:** OIDC (Authentik) via `openidconnect` (PKCE) for the web app +
  long-lived bearer device tokens for the Android app. `FORAGEBUDDY_ENV=dev`
  bypasses auth entirely (single implicit user `admin`) for zero-setup local
  use — ported verbatim from `ai_buddy`'s `auth` module.
- **LLM:** OpenAI-compatible chat completions, provider-switchable (LiteLLM
  proxy by default, OpenRouter as the revert switch) — ported from
  `ai_buddy`'s `llm` module, **minus all wallet/billing/cost-estimation
  code** (this app has no payments), **plus vision**: the identification
  pipeline's candidate-gathering pass sends photos as `image_url` content
  parts (OpenAI vision format), so the configured model must support image
  input. The existing default, `openrouter/~anthropic/claude-haiku-latest`,
  already does.
- **RAG:** same brute-force-cosine-over-a-BLOB-column design as `ai_buddy`'s
  `vector` module, reused almost verbatim, for two independent corpora:
  Wikipedia article chunks (`kind='wikipedia'`) and curated confusant notes
  (`kind='confusant'`).
- **Frontend:** SvelteKit (adapter-static) + Tauri 2 Android.
- **No wallet, no Stripe, no circles, no MCP, no FCM/push, no GeoIP.** All of
  that is `ai_buddy`-specific (it's an accountability coach with payments).
  Forage Buddy is a single-purpose identification tool; keep it that small.

## Workspace layout

```
crates/core     forage-buddy-core: shared domain enums + device-token gen (no axum/sqlx/tauri)
crates/server   the axum binary: auth, db, llm, photos, sightings, identification, species, wikipedia, vector, weather, settings
crates/mobile   Tauri 2 Android app wrapping web/build
web/            SvelteKit SPA (capture → identification screens)
crates/server/migrations/
  0001_init.sql            users, device_tokens
  0002_sightings.sql       sightings table
  0003_photos.sql          photos table
  0004_triage.sql          triage_results table (OLD two-stage flow; kept for history, no longer written to)
  0005_species_reference.sql   curated species_reference + confusant_pairs (seeded)
  0006_wikipedia.sql       wikipedia_pages cache
  0007_vectors.sql         embeddings table (RAG, shared by wikipedia + confusant kinds)
  0008_deepdive.sql        deepdive_results table (OLD two-stage flow; kept for history, no longer written to)
  0009_identification.sql     identification_results table (the current pipeline)
  0010_species_facts_cache.sql species_facts_cache table (cross-sighting facts cache)
  0011_identification_backfill.sql  one-time data migration: old triage/deepdive history -> identification_results
  0012_wikipedia_image_url.sql  wikipedia_pages gains image_url (reference photo for gatherer 4)
  0013_model_settings.sql  model_settings table (per-gatherer runtime model overrides)
  0014_species_facts_cache_image_url.sql  species_facts_cache gains wikipedia_image_url
```

**On the old `triage`/`deepdive` tables:** this app has real production data,
so `0004_triage.sql`/`0008_deepdive.sql` and their tables are never dropped.
Migration `0011` does a best-effort backfill of their history into the new
`identification_results` shape (see that migration file's header comment for
the exact mapping, including how the old `DangerLevel` vocabulary collapses
onto the new confusant `danger_level` vocabulary). Going forward, nothing
writes to `triage_results`/`deepdive_results` anymore — the Rust `triage`/
`deepdive` modules themselves were deleted; only the migrations and their
tables remain, as a historical record.

## Environment variables (all prefixed `FORAGEBUDDY_`)

Mirrors `ai_buddy`'s `AIBUDDY_*` naming exactly, module for module:

| Var | Default | Notes |
| --- | --- | --- |
| `FORAGEBUDDY_ENV=dev` / `FORAGEBUDDY_DEV_MODE=true` | off | Bypass auth; loopback bind only |
| `FORAGEBUDDY_BASE_URL` | `http://localhost:8080` | Real public URL in prod |
| `FORAGEBUDDY_BIND_ADDR` | `127.0.0.1:8080` | `0.0.0.0:8080` in a container |
| `FORAGEBUDDY_SQLITE_PATH` | `./data/forage_buddy.db` | |
| `FORAGEBUDDY_PHOTO_DIR` | `./data/photos` | Original photos, one subdir per sighting id |
| `FORAGEBUDDY_STATIC_DIR` | unset | Path to `web/build` to serve the SPA from the backend |
| `FORAGEBUDDY_COOKIE_SIGNING_KEY` | insecure dev default | ≥32 bytes; required outside dev |
| `FORAGEBUDDY_AUTHENTIK_ISSUER_URL` / `FORAGEBUDDY_OIDC_CLIENT_ID` / `FORAGEBUDDY_OIDC_CLIENT_SECRET` / `FORAGEBUDDY_OIDC_REDIRECT_URI` | localhost dev values | OIDC (Public/PKCE client; secret optional) |
| `FORAGEBUDDY_LLM_PROVIDER` | `litellm` | `litellm` or `openrouter` |
| `FORAGEBUDDY_LITELLM_API_KEY` / `FORAGEBUDDY_LITELLM_BASE_URL` | unset / `https://litellm.osmosis.page/v1` | |
| `FORAGEBUDDY_OPENROUTER_API_KEY` | unset | Used when provider=openrouter, and always for embeddings fallback per ai_buddy's logic |
| `FORAGEBUDDY_CHAT_MODEL` | `openrouter/~google/gemini-flash-latest` | Vision-capable — default for gatherer 1 (candidates) |
| `FORAGEBUDDY_FACTS_MODEL` | `openrouter/~anthropic/claude-haiku-latest` | Default for gatherer 2 (facts) |
| `FORAGEBUDDY_RISK_MODEL` | `openrouter/~google/gemini-flash-latest` | Tool-calling-capable — default for gatherer 3 (risks) |
| `FORAGEBUDDY_VISUAL_MATCH_MODEL` | `openrouter/~google/gemini-flash-latest` | Vision-capable — default for gatherer 4 (visual match) |
| `FORAGEBUDDY_ALLOWED_CHAT_MODELS` | 5 curated models | comma-separated; also the Settings page's dropdown options |

**These four `*_MODEL` vars are only defaults.** The `settings` module
(migration `0013_model_settings.sql`, table `model_settings` — a singleton
row) holds per-gatherer overrides, editable at runtime via `GET`/
`PUT /api/settings` or the web Settings page, with no restart required.
`identification::effective_models` equivalent —
`settings::effective_models(db, config)` — is what every gatherer actually
calls: DB override if set and non-empty, else the matching `Config`
default.
| `FORAGEBUDDY_EMBEDDING_MODEL` / `FORAGEBUDDY_EMBEDDING_DIM` | `bge-m3` / `1024` | |
| `FORAGEBUDDY_CORS_ORIGINS` | dev localhost + `http://tauri.localhost` | |
| `FORAGEBUDDY_MAX_PHOTO_BYTES` | `15728640` (15 MiB) | per-photo upload cap before resize |

## Domain types (`crates/core/src/domain.rs`)

String enums (same `str_enum!` macro pattern as `ai_buddy`), stored as
lowercase TEXT in SQLite, serialized as the same string over JSON:

```rust
SightingStatus { Open => "open", Archived => "archived" }
TriageStatus   { Insufficient => "insufficient", GenusCandidate => "genus_candidate", SpeciesCandidate => "species_candidate" }  // OLD, kept only for the 0004 migration's history; no longer produced
DangerLevel    { Unknown => "unknown", Safe => "safe", Caution => "caution", Toxic => "toxic", DeadlyToxic => "deadly_toxic" }   // still the curated species_reference/confusant_pairs vocabulary
```

The identification pipeline's own `IdentificationStatus`
(`pending`/`partial`/`complete`/`insufficient`/`failed`) and confusant
`danger_level` (`unknown`/`mild`/`toxic`/`deadly_toxic`) are server-only
types defined in `crates/server/src/identification/mod.rs`, not in
`forage-buddy-core` — the mobile crate only wraps the web build and never
needs them as Rust types. See that module's doc comments for the mapping
from the old 5-level `DangerLevel` vocabulary onto the new 4-level one.

`device_token` module: byte-for-byte copy of `ai_buddy_core::device_token`.

## Database schema (details)

```sql
-- 0002_sightings.sql
CREATE TABLE sightings (
    id           TEXT PRIMARY KEY,
    user_id      TEXT NOT NULL REFERENCES users(id),
    status       TEXT NOT NULL DEFAULT 'open',   -- SightingStatus
    lat          REAL,                            -- nullable: user can skip location
    lon          REAL,
    location_accuracy_m REAL,
    place_label  TEXT,                             -- optional reverse-geocoded/free-text label
    observed_at  TEXT NOT NULL,                     -- RFC3339, when the sighting happened (client-supplied)
    notes        TEXT,
    created_at   TEXT NOT NULL,
    updated_at   TEXT NOT NULL
);
CREATE INDEX idx_sightings_user ON sightings(user_id, created_at DESC);

-- 0003_photos.sql
CREATE TABLE photos (
    id           TEXT PRIMARY KEY,
    sighting_id  TEXT NOT NULL REFERENCES sightings(id),
    file_path    TEXT NOT NULL,     -- relative to FORAGEBUDDY_PHOTO_DIR
    content_type TEXT NOT NULL,
    width        INTEGER,
    height       INTEGER,
    taken_at     TEXT NOT NULL,     -- RFC3339, client-supplied capture time (defaults to upload time)
    sort_order   INTEGER NOT NULL,
    created_at   TEXT NOT NULL
);
CREATE INDEX idx_photos_sighting ON photos(sighting_id, sort_order);

-- 0004_triage.sql (OLD two-stage flow — kept for history, no longer written to;
-- see 0011_identification_backfill.sql and the "identification pipeline" section below)
CREATE TABLE triage_results (
    id             TEXT PRIMARY KEY,
    sighting_id    TEXT NOT NULL REFERENCES sightings(id),
    created_at     TEXT NOT NULL,
    model          TEXT NOT NULL,
    status         TEXT NOT NULL,        -- TriageStatus
    genus          TEXT,
    candidate_species_json TEXT NOT NULL, -- JSON array of {species, common_name, confidence}
    missing_info_json      TEXT NOT NULL, -- JSON array of strings, e.g. "photo of gill attachment"
    reasoning      TEXT NOT NULL,         -- short, shown to user as "why"
    photos_considered INTEGER NOT NULL    -- how many photos were in context for this attempt
);
CREATE INDEX idx_triage_sighting ON triage_results(sighting_id, created_at DESC);

-- 0005_species_reference.sql (curated, seeded with INSERTs in the migration itself)
CREATE TABLE species_reference (
    id            TEXT PRIMARY KEY,       -- slug, e.g. "amanita-phalloides"
    genus         TEXT NOT NULL,
    species       TEXT,                   -- NULL = genus-level entry
    common_names_json TEXT NOT NULL,       -- JSON array
    danger_level  TEXT NOT NULL,           -- DangerLevel
    wikipedia_title TEXT,                  -- best-known title to look up; NULL = derive from name
    summary       TEXT NOT NULL            -- 1-3 sentence curated safety-relevant summary
);
CREATE TABLE confusant_pairs (
    id              TEXT PRIMARY KEY,
    species_ref_a   TEXT NOT NULL REFERENCES species_reference(id),
    species_ref_b   TEXT NOT NULL REFERENCES species_reference(id),
    danger_level    TEXT NOT NULL,          -- danger of confusing A for B (usually B's danger)
    distinguishing_features_json TEXT NOT NULL, -- JSON array of strings, the checklist
    notes           TEXT NOT NULL
);
CREATE INDEX idx_confusant_a ON confusant_pairs(species_ref_a);
CREATE INDEX idx_confusant_b ON confusant_pairs(species_ref_b);

-- 0006_wikipedia.sql
CREATE TABLE wikipedia_pages (
    title       TEXT PRIMARY KEY,   -- canonical page title, used as cache key
    pageid      INTEGER,
    url         TEXT NOT NULL,
    extract     TEXT NOT NULL,      -- plain-text extract (REST API "extracts")
    fetched_at  TEXT NOT NULL
);

-- 0007_vectors.sql  (identical design to ai_buddy's `embeddings` table)
CREATE TABLE embeddings (
    id         TEXT PRIMARY KEY,
    kind       TEXT NOT NULL,   -- 'wikipedia' | 'confusant'
    source_id  TEXT NOT NULL,   -- wikipedia_pages.title, or confusant_pairs.id
    model      TEXT NOT NULL,
    dim        INTEGER NOT NULL,
    vec        BLOB NOT NULL,   -- dim * 4 bytes, little-endian f32
    text       TEXT NOT NULL,   -- chunk text, returned by search for RAG
    created_at TEXT NOT NULL
);
CREATE INDEX idx_embeddings_kind_source ON embeddings(kind, source_id);

-- 0008_deepdive.sql (OLD two-stage flow — kept for history, no longer written to)
CREATE TABLE deepdive_results (
    id                TEXT PRIMARY KEY,
    sighting_id       TEXT NOT NULL REFERENCES sightings(id),
    created_at        TEXT NOT NULL,
    model             TEXT NOT NULL,
    best_match_species TEXT NOT NULL,
    confidence        REAL NOT NULL,
    wikipedia_title   TEXT,
    wikipedia_url     TEXT,
    wikipedia_extract TEXT,
    confusants_json   TEXT NOT NULL,   -- JSON array of {species, common_name, danger_level, distinguishing_features: [string], notes}
    safety_notes      TEXT NOT NULL    -- always non-empty, always includes the standard disclaimer
);
CREATE INDEX idx_deepdive_sighting ON deepdive_results(sighting_id, created_at DESC);

-- 0009_identification.sql (the CURRENT, single pipeline)
CREATE TABLE identification_results (
    id                 TEXT PRIMARY KEY,
    sighting_id        TEXT NOT NULL REFERENCES sightings(id),
    created_at         TEXT NOT NULL,
    updated_at         TEXT NOT NULL,
    status             TEXT NOT NULL CHECK(status IN ('pending','partial','complete','insufficient','failed')),
    model              TEXT NOT NULL,
    candidates_json    TEXT NOT NULL DEFAULT '[]',   -- JSON array, see IdentificationResultDto below
    missing_info_json  TEXT NOT NULL DEFAULT '[]',   -- JSON array of strings
    photos_considered  INTEGER NOT NULL
);
CREATE INDEX idx_identification_sighting ON identification_results(sighting_id, created_at DESC);

-- 0010_species_facts_cache.sql (cross-sighting, cross-user read-through cache
-- for the facts gatherer — one row per normalized species/genus name, reused
-- regardless of which sighting asked about it first)
CREATE TABLE species_facts_cache (
    species_key     TEXT PRIMARY KEY,   -- lowercased "genus species" or bare genus
    wikipedia_title  TEXT,
    wikipedia_url    TEXT,
    edible           INTEGER,           -- nullable 0/1 boolean
    medicinal        INTEGER,
    psychoactive     INTEGER,
    poisonous        INTEGER,
    danger_level     TEXT,              -- internal 5-level DangerLevel representation (curated-floor-reconciled)
    risk_note        TEXT NOT NULL,
    source_model     TEXT NOT NULL,
    fetched_at       TEXT NOT NULL
);

-- 0011_identification_backfill.sql: a one-time INSERT...SELECT (no new
-- table) that converts each sighting's latest triage_results row (+ latest
-- deepdive_results row, if any) into one identification_results row. See
-- that file's header comment for the exact field-by-field mapping.
```

## Backend module contracts (cross-module call signatures — DO NOT rename)

Each module owns its own files and migration(s). **Do not edit another
module's files or the shared `main.rs`/`state.rs`/`routes.rs`/`Cargo.toml` —
those are wired up by the integrator after all modules land.** Instead,
build against these exact signatures and report in your final summary
exactly what field(s) `AppState` needs and what router line(s) `routes.rs`
needs.

```rust
// crates/server/src/llm/mod.rs — owned by the "llm" agent
pub struct LlmClient { /* ... */ }
impl LlmClient {
    pub fn new(config: &Config) -> Self;
    pub fn resolve_model(&self, requested: Option<&str>) -> String;
    pub fn default_chat_model(&self) -> &str;

    /// Plain JSON-mode completion, no images.
    pub async fn chat_json<T: DeserializeOwned>(&self, model: &str, system: &str, user: &str) -> AppResult<T>;

    /// JSON-mode completion with 1+ images attached to the user turn.
    /// `images` are data URLs: "data:image/jpeg;base64,...".
    pub async fn chat_json_vision<T: DeserializeOwned>(
        &self, model: &str, system: &str, user_text: &str, images: &[String],
    ) -> AppResult<T>;

    /// Raw tool-calling turn (OpenAI `tools` format) — used by the
    /// identification pipeline's risks gatherer's Wikipedia-agentic-query loop.
    pub async fn chat_tools(&self, model: &str, messages: Vec<serde_json::Value>, tools: &serde_json::Value) -> AppResult<ToolTurn>;

    pub async fn embed(&self, text: &str) -> AppResult<Vec<f32>>;
}
```

```rust
// crates/server/src/vector/mod.rs (ported from ai_buddy)
pub async fn upsert(db: &SqlitePool, kind: &str, source_id: &str, model: &str, text: &str, vec: &[f32]) -> AppResult<()>;
pub async fn search(db: &SqlitePool, kind: &str, query_vec: &[f32], top_k: usize) -> AppResult<Vec<(String /* source_id */, String /* text */, f32 /* score */)>>;
```

```rust
// crates/server/src/photos/mod.rs
pub fn router() -> Router<AppState>;  // POST /api/sightings/{id}/photos (multipart), GET /api/photos/{id}/file

/// Called by the upload handler after the file is persisted and the row
/// inserted. Fire-and-forget from the HTTP handler's point of view: spawn
/// it, log failures, never fail the upload response on an identification
/// error.
pub async fn on_photo_uploaded(state: AppState, sighting_id: String);
```

```rust
// crates/server/src/identification/mod.rs — the single identification pipeline
pub async fn run_identification(state: &AppState, sighting_id: &str) -> AppResult<()>;
pub async fn get_latest(db: &SqlitePool, sighting_id: &str) -> AppResult<Option<IdentificationResultDto>>;
pub async fn get_history(db: &SqlitePool, sighting_id: &str) -> AppResult<Vec<IdentificationResultDto>>;
pub fn router() -> Router<AppState>;  // GET /api/sightings/{id}/identification (history), POST .../identification (manual re-run/retry)

// Internal gatherer stages (also independently callable/testable):
pub async fn gather_candidates(state: &AppState, sighting_id: &str) -> AppResult<CandidateGatherResult>;
pub async fn gather_facts(state: &AppState, candidate_species: &str) -> SpeciesFacts;      // infallible, best-effort
pub async fn gather_risks(state: &AppState, candidate_species: &str) -> Vec<ConfusantSummaryDto>; // infallible, best-effort
pub fn compile_identification(status: IdentificationStatus, created_at: String, candidates: Vec<EnrichedCandidate>, missing_info: Vec<String>) -> IdentificationResultDto; // pure, not an LLM call
```

```rust
// crates/server/src/sightings/mod.rs (sightings + photos are one module's scope)
pub fn router() -> Router<AppState>;
```

`photos::on_photo_uploaded` calls `identification::run_identification`
internally — that's the ONE cross-module call on the upload path (down from
two separate calls — `triage::run_triage` then `deepdive::run_deepdive` — in
the old design). `on_photo_uploaded` ignores the `Ok` value and logs `Err`.

Every call to `on_photo_uploaded` first acquires a permit from
`AppState.identification_semaphore` (process-wide, capacity 1). Uploading
several photos for one sighting in quick succession (a multi-select) spawns
one `on_photo_uploaded` task per photo; without this, each would start its
own full gather-then-compile pipeline concurrently against the same
sighting, racing writes to the same `identification_results` row and paying
for overlapping LLM calls. The semaphore only serializes — it does not
dedupe, so N photos uploaded back-to-back still run N full pipelines
sequentially rather than one; accepted as a minor cost inefficiency at this
app's scale.

## REST API surface (all `/api/*` require auth; same auth model as `ai_buddy`)

- `GET /health` · OIDC `/auth/login|/auth/callback|/auth/logout|/auth/me`
- **Sightings:**
  `POST /api/sightings` `{lat?, lon?, location_accuracy_m?, place_label?, observed_at, notes?}` → sighting
  `GET /api/sightings` → list, newest first
  `GET /api/sightings/{id}` → sighting + its photos + latest identification (one aggregate view — the frontend should need exactly one request to render the detail screen)
  `PATCH /api/sightings/{id}` `{notes?, status?}`
- **Photos:**
  `POST /api/sightings/{id}/photos` — multipart, field `photo` (+ optional `taken_at`, `lat`, `lon` fields overriding the sighting's own if the user moved between shots); triggers `on_photo_uploaded` after commit
  `GET /api/photos/{id}/file` — streams the original image (auth + ownership checked)
- **Identification:** `GET /api/sightings/{id}/identification` (full history, newest first) · `POST /api/sightings/{id}/identification` (manual re-run/retry, e.g. after notes edited or a prior attempt failed)
- **Weather:** `GET /api/weather?lat=&lon=` → last 14 days of daily historical weather (`temp_max_c`/`temp_min_c`/`temp_mean_c`/`precipitation_mm`/`rain_mm`/`wind_speed_max_kmh`/`wind_speed_mean_kmh`/`humidity_mean_pct` per day), via `weather::fetch_last_14_days` (Open-Meteo Archive API, no key required). Groundwork for a future season/weather-aware foraging-suggestion feature; not called from the frontend yet.
- **Settings:** `GET /api/settings` → `{candidate_model, facts_model, risk_model, visual_match_model, available_models}` (effective per-gatherer model + the curated dropdown list) · `PUT /api/settings` `{candidate_model, facts_model, risk_model, visual_match_model}` (all 4 required; each validated against `available_models`, else `400`) → same shape, now reflecting the update. Backed by `settings::effective_models`/the `model_settings` table — see the Environment variables section.

Every error response is `{"message": "..."}` (see `ai_buddy`'s `AppError`,
ported as-is minus `PaymentRequired`).

## The identification pipeline, in detail

Runs automatically on every photo upload (`photos::on_photo_uploaded` calls
`identification::run_identification` directly, serialized by
`AppState.identification_semaphore` — see that field's doc comment) and can
be manually re-run via `POST /api/sightings/{id}/identification`.
`run_identification` orchestrates four internal gatherer stages — NOT
separate processes, just named, narrowly-scoped calls:

1. **Candidates** (`gather_candidates`, fast/cheap vision call) — identifies
   the most likely species (or genus, when species-level confidence isn't
   there) from the photo(s), using the photos + the sighting's lat/lon +
   observed_at (season/geography narrows candidates a lot) + any prior
   attempt's `missing_info` (so follow-up photos answer what was asked). It
   MUST be willing to return an EMPTY candidates array with concrete
   `missing_info` (e.g. "photo of the gill attachment to the stem", "photo
   of the underside/pores", "a spore print", "the full plant including
   root/base") rather than guess. One `chat_json_vision` call on the
   `candidate_model` setting.
2. **Facts** (`gather_facts`, per top candidate, runs concurrently with
   step 3) — read-through `species_facts_cache` keyed on the normalized
   species/genus name; on a cache miss, resolves the best Wikipedia article
   (try `species_reference.wikipedia_title` first, else the species name)
   via `wikipedia::fetch_or_cache` (which now also captures a representative
   photo URL — `thumbnail.source` falling back to `originalimage.source` —
   used by step 4), then one `chat_json` call on the `facts_model` setting
   extracting `{edible, medicinal, psychoactive, poisonous, risk_note}`
   (`risk_note` truncated to 140 chars in code — never trusted to
   self-limit), then writes through the cache. **Curated reference data is
   a floor, never a ceiling**: if `species::lookup` finds a curated
   `species_reference` row, its `danger_level` can only be *raised*, never
   lowered, by the LLM's own `poisonous` guess (see
   `identification::reconcile_danger`).
3. **Risks** (`gather_risks`, per top candidate, runs concurrently with
   step 2) — queries `confusant_pairs` for the candidate (and its
   genus-mates) first (curated, trustworthy, offline); then runs one
   `chat_tools` loop on the `risk_model` setting where the model can call a
   `search_wikipedia` tool (semantic search over embeddings this step
   grounds via `wikipedia::fetch_or_cache` + `vector::upsert`) to extend the
   confusant list. Always merges in the curated pairs even if the model's
   tool loop fails or times out — curated data must never be lost to an LLM
   hiccup. Output is condensed to one short phrase per confusant (species +
   danger level + a ≤100-char distinguishing note) — no checklist arrays,
   no paragraph notes.
4. **Visual match** (`verify_visual_match`, TOP CANDIDATE ONLY, runs after
   steps 2+3 finish) — an error-detection sanity check, not a safety
   signal: fetches the real Wikipedia photo URL step 2 captured for the top
   candidate, re-encodes it as a `data:` URL (capped at 8 MiB, 10s
   timeout), and asks one `chat_json_vision` call on the `visual_match_model`
   setting whether it plausibly shows the same organism as the forager's
   own (already-loaded) photo. Infallible: no reference photo, a fetch
   failure, or an LLM error all degrade to `(None, "")` — "not checked",
   never blocking the pipeline or treated as "no match".

`run_identification`'s lifecycle: INSERT a `pending` row immediately → run
the candidates gatherer → UPDATE to `partial` (or `insufficient` if no
candidates were found, in which case the pipeline stops here) → run the
facts+risks gatherers concurrently per top candidate (capped to the top 3 by
confidence) → run the visual-match check for the single top candidate →
`compile_identification` (pure Rust, NOT an LLM call: merges everything,
enforces every text-field length cap, sorts candidates by confidence
descending, caps to 3) → UPDATE to `complete`. On any unrecoverable error at
any step, UPDATE to `failed` rather than leaving the row stuck at
`pending`/`partial` forever.

Exact response/stored JSON shape (`IdentificationResultDto`):

```json
{
  "status": "pending|partial|complete|insufficient|failed",
  "created_at": "...",
  "candidates": [
    {
      "species": "Laetiporus sulphureus",
      "common_name": "Chicken of the woods",
      "confidence": 0.74,
      "edible": true, "medicinal": false, "psychoactive": false, "poisonous": false,
      "wikipedia_url": "https://en.wikipedia.org/wiki/...",
      "risk_note": "short phrase, ≤140 chars",
      "confusants": [ {"species": "...", "danger_level": "unknown|mild|toxic|deadly_toxic", "note": "≤100 chars", "wikipedia_url": "..."} ],
      "visual_match": true,
      "visual_match_note": "short phrase, may be empty, ≤140 chars"
    }
  ],
  "missing_info": ["..."]
}
```

Booleans may be `null` when unknown (never a forced guess). `visual_match`/
`visual_match_note` are only ever populated on `candidates[0]` (the
top-confidence candidate) — every other candidate always has
`visual_match: null`; `#[serde(default)]` on both fields so rows persisted
before gatherer 4 existed (including the `0011` triage/deepdive backfill)
still deserialize.

## Frontend screens (SvelteKit, `web/src/routes/`)

- `/` — list of sightings (newest first), each row: thumbnail, place/time,
  status badge (Insufficient / top candidate species+confidence / a danger
  badge when the identification found a risky candidate or confusant).
- `/sightings/new` — capture flow: `<input type="file" accept="image/*"
  capture="environment">` (works in both browser and the Tauri Android
  webview — no native camera plugin needed), geolocation via
  `@tauri-apps/plugin-geolocation` (native) falling back to
  `navigator.geolocation` (web), date/time defaults to now but is editable.
  Creates the sighting, uploads the first photo, redirects to its detail page.
- `/sightings/[id]` — photos at the very top of the page (gallery + an
  always-visible "add more photos" control; re-runs identification
  automatically), then one identification card per top candidate: status,
  confidence + edible/medicinal/psychoactive/poisonous flags + Wikipedia
  link + risk note, a badge for `visual_match` (only ever shown on the top
  candidate: ✅ when `true`, an amber "worth a second look" note when
  `false`, nothing when `null`/"not checked"), or the "I'm not sure yet —
  send a photo of…" prompt rendered prominently when `insufficient`; a
  visually distinct **"Could be confused with"** section per confusant
  (danger-level colored badge: grey=unknown, yellow=mild, orange=toxic,
  red=deadly toxic) with its short distinguishing note. A manual "Re-run
  identification" action stays available (e.g. after adding more photos or
  notes).
- `/settings` — 4 dropdowns (one per gatherer: candidate/facts/risk/visual
  match), populated from `available_models`, pre-selected to the current
  effective model; Save PUTs all 4 at once. Linked from a "⚙ Settings" link
  in the header.

## Safety disclaimer + poisonous warning (frontend-only; not enforced by the backend)

Replaces the old passive banner with two gated modals
(`web/src/lib/components/SafetyAckModal.svelte` /
`PoisonousWarningModal.svelte`), both pure frontend UX — the backend has no
corresponding check, token, or enforcement for either:

- **`SafetyAckModal`**: shown once per session before any identification
  result is visible. Exact text: "Results are fetched by AI and can
  therefore be wildly inaccurate. Say it with me: never munch on a hunch."
  The user must type "never munch on a hunch" (case-insensitive) before a
  "Continue" button enables.
- **`PoisonousWarningModal`**: a separate alert triggered the first time a
  `complete` identification result has any candidate with `poisonous ===
  true`. Exact text: "⚠️ There's a chance this specimen is poisonous! Be
  very careful!" Shown once per sighting view (not re-triggered on every
  poll tick), resets on manual re-identify.
