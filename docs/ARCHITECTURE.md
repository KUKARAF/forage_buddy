# Forage Buddy — Architecture & Module Contract

> **Working name.** An AI foraging helper: you photograph a plant/fungus/etc.
> in the field, the app records photo + GPS location + timestamp, a fast
> "triage" pass tells you the likely genus (or says "I don't know, send more
> photos" when it can't), and a slower "deep dive" pass grounds the best
> guess in its Wikipedia article and surfaces dangerous look-alikes
> ("confusant species") with a checklist of what to check to rule them out.
>
> **Safety is the entire point of the deep-dive stage.** Misidentifying a
> mushroom or plant can kill. Every LLM prompt in this app, every API
> response, and every screen that shows a species guess MUST carry a
> prominent disclaimer: this is a research aid, not a field guide substitute,
> and nothing it identifies should be eaten/used on the strength of its
> output alone. Never soften or omit this.

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
  code** (this app has no payments), **plus vision**: the triage pass sends
  photos as `image_url` content parts (OpenAI vision format), so the
  configured model must support image input. The existing default,
  `openrouter/~anthropic/claude-haiku-latest`, already does.
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
crates/server   the axum binary: auth, db, llm, photos, sightings, triage, species, wikipedia, vector, deepdive
crates/mobile   Tauri 2 Android app wrapping web/build
web/            SvelteKit SPA (capture → triage → deep-dive screens)
crates/server/migrations/
  0001_init.sql            users, device_tokens
  0002_sightings.sql       sightings table
  0003_photos.sql          photos table
  0004_triage.sql          triage_results table
  0005_species_reference.sql   curated species_reference + confusant_pairs (seeded)
  0006_wikipedia.sql       wikipedia_pages cache
  0007_vectors.sql         embeddings table (RAG, shared by wikipedia + confusant kinds)
  0008_deepdive.sql        deepdive_results table
```

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
| `FORAGEBUDDY_CHAT_MODEL` | `openrouter/~anthropic/claude-haiku-latest` | Must be vision-capable — used for triage AND deep-dive |
| `FORAGEBUDDY_ALLOWED_CHAT_MODELS` | same + `gemma4-26b` | comma-separated |
| `FORAGEBUDDY_EMBEDDING_MODEL` / `FORAGEBUDDY_EMBEDDING_DIM` | `bge-m3` / `1024` | |
| `FORAGEBUDDY_CORS_ORIGINS` | dev localhost + `http://tauri.localhost` | |
| `FORAGEBUDDY_MAX_PHOTO_BYTES` | `15728640` (15 MiB) | per-photo upload cap before resize |

## Domain types (`crates/core/src/domain.rs`)

String enums (same `str_enum!` macro pattern as `ai_buddy`), stored as
lowercase TEXT in SQLite, serialized as the same string over JSON:

```rust
SightingStatus { Open => "open", Archived => "archived" }
TriageStatus   { Insufficient => "insufficient", GenusCandidate => "genus_candidate", SpeciesCandidate => "species_candidate" }
DangerLevel    { Unknown => "unknown", Safe => "safe", Caution => "caution", Toxic => "toxic", DeadlyToxic => "deadly_toxic" }
```

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

-- 0004_triage.sql
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

-- 0008_deepdive.sql
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

    /// Raw tool-calling turn (OpenAI `tools` format) — used by the deep-dive
    /// Wikipedia-agentic-query loop.
    pub async fn chat_tools(&self, model: &str, messages: Vec<serde_json::Value>, tools: &serde_json::Value) -> AppResult<ToolTurn>;

    pub async fn embed(&self, text: &str) -> AppResult<Vec<f32>>;
}
```

```rust
// crates/server/src/vector/mod.rs — owned by the "deepdive" agent (ported from ai_buddy)
pub async fn upsert(db: &SqlitePool, kind: &str, source_id: &str, model: &str, text: &str, vec: &[f32]) -> AppResult<()>;
pub async fn search(db: &SqlitePool, kind: &str, query_vec: &[f32], top_k: usize) -> AppResult<Vec<(String /* source_id */, String /* text */, f32 /* score */)>>;
```

```rust
// crates/server/src/photos/mod.rs — owned by the "photos" agent
pub fn router() -> Router<AppState>;  // POST /api/sightings/{id}/photos (multipart), GET /api/photos/{id}/file

/// Called by the upload handler after the file is persisted and the row
/// inserted. Fire-and-forget from the HTTP handler's point of view: spawn
/// it, log failures, never fail the upload response on a triage error.
pub async fn on_photo_uploaded(state: AppState, sighting_id: String);
```

```rust
// crates/server/src/triage/mod.rs — owned by the "triage" agent
pub async fn run_triage(state: &AppState, sighting_id: &str) -> AppResult<TriageResultDto>;
pub fn router() -> Router<AppState>;  // GET /api/sightings/{id}/triage (history), POST .../triage (manual re-run)
```

```rust
// crates/server/src/deepdive/mod.rs — owned by the "deepdive" agent
pub async fn run_deepdive(state: &AppState, sighting_id: &str) -> AppResult<DeepDiveDto>;
pub fn router() -> Router<AppState>;  // POST /api/sightings/{id}/deepdive (trigger), GET .../deepdive (latest)
```

```rust
// crates/server/src/sightings/mod.rs — owned by the "photos" agent (sightings + photos are one agent's scope)
pub fn router() -> Router<AppState>;
```

`photos::on_photo_uploaded` calls `triage::run_triage` internally — that's
the ONE cross-agent function call in the whole backend. Both agents must
match the signature above exactly (`AppState` owned, `sighting_id: &str`,
returns `AppResult<TriageResultDto>`); `on_photo_uploaded` ignores the `Ok`
value and logs `Err`.

## REST API surface (all `/api/*` require auth; same auth model as `ai_buddy`)

- `GET /health` · OIDC `/auth/login|/auth/callback|/auth/logout|/auth/me`
- **Sightings:**
  `POST /api/sightings` `{lat?, lon?, location_accuracy_m?, place_label?, observed_at, notes?}` → sighting
  `GET /api/sightings` → list, newest first
  `GET /api/sightings/{id}` → sighting + its photos + latest triage + latest deepdive (one aggregate view — the frontend should need exactly one request to render the detail screen)
  `PATCH /api/sightings/{id}` `{notes?, status?}`
- **Photos:**
  `POST /api/sightings/{id}/photos` — multipart, field `photo` (+ optional `taken_at`, `lat`, `lon` fields overriding the sighting's own if the user moved between shots); triggers `on_photo_uploaded` after commit
  `GET /api/photos/{id}/file` — streams the original image (auth + ownership checked)
- **Triage:** `GET /api/sightings/{id}/triage` (full history, newest first) · `POST /api/sightings/{id}/triage` (manual re-run, e.g. after notes edited)
- **Deep dive:** `POST /api/sightings/{id}/deepdive` · `GET /api/sightings/{id}/deepdive`

Every error response is `{"message": "..."}` (see `ai_buddy`'s `AppError`,
ported as-is minus `PaymentRequired`).

## The two LLM-driven stages, in detail

### 1. Triage (fast, cheap, runs automatically on every photo upload)

System prompt directs the model to: identify the most likely **genus**
(and, if confident, species) of the organism in the photo(s), using the
photos + the sighting's lat/lon + observed_at (season/geography narrows
candidates a lot) + any prior triage attempt's `missing_info` (so follow-up
photos answer what was asked). It MUST be willing to return
`status: "insufficient"` with concrete `missing_info` (e.g. "photo of the
gill attachment to the stem", "photo of the underside/pores", "a spore
print", "the full plant including root/base") rather than guess. Output is
one `chat_json_vision` call, schema:

```json
{
  "status": "insufficient" | "genus_candidate" | "species_candidate",
  "genus": "Amanita" | null,
  "candidate_species": [{"species": "Amanita phalloides", "common_name": "Death cap", "confidence": 0.62}],
  "missing_info": ["a clear photo of the gills", "..."],
  "reasoning": "one or two sentences"
}
```

### 2. Deep dive (slower, user- or auto-triggered once a genus candidate exists)

A short in-process pipeline of LLM calls (NOT separate processes — just
named, narrowly-scoped calls, matching the "a few more subagents" framing
from the product brief):

1. **Wikipedia grounding** — resolve the best Wikipedia article for the
   top candidate species (try `species_reference.wikipedia_title` first,
   else the species name, else the genus) via the public REST API
   (`https://en.wikipedia.org/api/rest_v1/page/summary/{title}` for the
   extract, `.../page/html/{title}` or the `action=query&prop=extracts` API
   for a longer body if the summary is too thin). Cache in
   `wikipedia_pages`, chunk (~1000 chars, paragraph-aligned) and embed into
   `embeddings` with `kind='wikipedia'`.
2. **Look-alike / confusant finder** — query `confusant_pairs` for the
   candidate (and its genus-mates) first (curated, trustworthy, offline);
   then run one `chat_tools` loop where the model can call a `search_wikipedia`
   tool (semantic search over the embeddings from step 1, plus it may fetch
   ONE more article via the same Wikipedia client if it names a specific
   look-alike not yet cached) to extend/verify the confusant list and fill in
   `distinguishing_features`. Always merge in the curated pairs even if the
   model's tool loop fails or times out — curated data must never be lost to
   an LLM hiccup.
3. **Synthesis** — one final `chat_json` call (text-only) that combines the
   candidate, the Wikipedia extract, and the merged confusant list into the
   `DeepDiveDto` stored in `deepdive_results`. `safety_notes` always starts
   with the standard disclaimer (hardcode a constant the LLM text is
   appended to — never rely on the model to include it).

## Frontend screens (SvelteKit, `web/src/routes/`)

- `/` — list of sightings (newest first), each row: thumbnail, place/time,
  status badge (Insufficient / Genus: X / Species: X / Deep-dive done).
- `/sightings/new` — capture flow: `<input type="file" accept="image/*"
  capture="environment">` (works in both browser and the Tauri Android
  webview — no native camera plugin needed), geolocation via
  `@tauri-apps/plugin-geolocation` (native) falling back to
  `navigator.geolocation` (web), date/time defaults to now but is editable.
  Creates the sighting, uploads the first photo, redirects to its detail page.
- `/sightings/[id]` — photo gallery + "add another photo" (re-runs triage),
  triage card (status, genus/species guesses with confidence bars, or the
  "I'm not sure yet — send a photo of…" prompt rendered prominently),
  "Run deep dive" button (enabled once status ≠ insufficient), deep-dive
  card: best match, Wikipedia summary + link, a visually distinct
  **"Could be confused with"** section per confusant (danger-level colored
  badge: grey=unknown, green=safe, yellow=caution, orange=toxic, red=deadly
  toxic) each with its distinguishing-features checklist rendered as an
  actual `<ul>` of checkboxes the user can tick off while re-examining their
  specimen. A persistent, unmissable safety banner on every screen that
  shows a species guess.

## Safety disclaimer (exact text, used in both backend constant and frontend)

> "Forage Buddy gives a best-effort AI guess, not a confirmed identification.
> Never eat, use, or handle anything based solely on this app. Misidentifying
> a wild plant or fungus can cause severe illness or death. Confirm with a
> qualified local expert, a spore print, and multiple field guides before
> consuming or using anything you forage."
