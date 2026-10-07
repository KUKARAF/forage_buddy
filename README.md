# Forage Buddy

> Working name. An AI foraging-identification helper: you photograph a
> plant/fungus/etc. in the field, the app records photo + GPS location +
> timestamp, and ONE automatic **identification** pipeline runs on every
> upload: it finds candidate species (or honestly says "I don't know, send
> more photos" when it can't), grounds each top candidate in its Wikipedia
> article with edible/medicinal/psychoactive/poisonous facts, and surfaces
> dangerous look-alikes ("confusant species") with a short note on how to
> tell them apart.

**Safety is the entire point of this pipeline.** Misidentifying a
mushroom or plant can kill. Every screen that shows a species guess carries
a prominent disclaimer: this is a research aid, not a field guide
substitute, and nothing it identifies should be eaten/used on the strength
of its output alone.

Rust backend mirroring the [`ai_buddy`](../ai_buddy) stack. One service
serves a **web** frontend and an **Android APK** (Tauri).

## Stack
- **Backend:** Rust — axum 0.8, tokio, sqlx + SQLite (WAL), tower-sessions (SQLite store).
- **Auth:** OIDC (Authentik) via `openidconnect` (PKCE) for the web app + long-lived
  bearer device tokens for the Android app. `FORAGEBUDDY_ENV=dev` bypasses auth
  entirely (single implicit user `admin`) for zero-setup local use.
- **LLM:** OpenAI-compatible chat completions, provider-switchable (LiteLLM proxy by
  default, OpenRouter as the revert switch), **plus vision** — the identification
  pipeline's candidate-gathering pass sends photos as `image_url` content parts, so
  the configured model must support image input. No wallet/billing/cost-estimation
  code (this app has no payments).
- **RAG:** brute-force cosine search over a `BLOB` embeddings column in SQLite — no
  extra service — for two corpora: Wikipedia article chunks and curated confusant
  notes.
- **Frontend:** SvelteKit (adapter-static) + Tauri 2 Android.
- **No wallet, no Stripe, no circles, no MCP, no FCM/push, no GeoIP.** Forage Buddy is
  a single-purpose identification tool; it stays that small.

## Workspace layout
```
crates/core     forage-buddy-core: shared domain enums + device-token gen (no axum/sqlx/tauri)
crates/server   the axum binary: auth, db, llm, photos, sightings, identification, species, wikipedia, vector
crates/mobile   Tauri 2 Android app wrapping web/build
web/            SvelteKit SPA (capture → identification screens)
crates/server/migrations/
  0001_init.sql                users, device_tokens
  0002_sightings.sql           sightings table
  0003_photos.sql               photos table
  0004_triage.sql               triage_results table (old flow; kept for history)
  0005_species_reference.sql   curated species_reference + confusant_pairs (seeded)
  0006_wikipedia.sql            wikipedia_pages cache
  0007_vectors.sql               embeddings table (RAG, shared by wikipedia + confusant kinds)
  0008_deepdive.sql              deepdive_results table (old flow; kept for history)
  0009_identification.sql        identification_results table (the current pipeline)
  0010_species_facts_cache.sql   species_facts_cache table
  0011_identification_backfill.sql  backfills old triage/deepdive history into identification_results
```
Full design + module contracts: **`docs/ARCHITECTURE.md`**.

## Run locally (dev mode — auth bypassed, user "admin")
```bash
# Backend (bypasses OIDC, binds loopback only):
FORAGEBUDDY_ENV=dev cargo run -p server
# → listening on 127.0.0.1:8080 ; GET /health → ok ; GET /auth/me → the admin user

# To exercise the identification pipeline, add an LLM key (needs a vision-capable model):
FORAGEBUDDY_ENV=dev FORAGEBUDDY_LITELLM_API_KEY=sk-... cargo run -p server
# or, with the OpenRouter revert switch:
FORAGEBUDDY_ENV=dev FORAGEBUDDY_LLM_PROVIDER=openrouter FORAGEBUDDY_OPENROUTER_API_KEY=sk-or-... cargo run -p server

# Frontend (separate terminal; talks to the backend via CORS in dev):
cd web && npm install && npm run dev      # http://localhost:5173
```

Check everything the way CI does:
```bash
cargo fmt --all --check
cargo build --workspace --exclude mobile
cargo test  --workspace --exclude mobile
cargo clippy -p server -p forage-buddy-core --lib --bins -- -D warnings
cd web && npm ci && npm run check && npm run build
```

## Environment variables (all prefixed `FORAGEBUDDY_`)
| Var | Default | Notes |
| --- | --- | --- |
| `FORAGEBUDDY_ENV=dev` / `FORAGEBUDDY_DEV_MODE=true` | off | Bypass auth; loopback bind only |
| `FORAGEBUDDY_BASE_URL` | `http://localhost:8080` | Real public URL in prod |
| `FORAGEBUDDY_BIND_ADDR` | `127.0.0.1:8080` | `0.0.0.0:8080` in a container |
| `FORAGEBUDDY_SQLITE_PATH` | `./data/forage_buddy.db` | |
| `FORAGEBUDDY_PHOTO_DIR` | `./data/photos` | Original photos, one subdir per sighting id |
| `FORAGEBUDDY_STATIC_DIR` | unset | Path to `web/build` to serve the SPA from the backend |
| `FORAGEBUDDY_COOKIE_SIGNING_KEY` | insecure dev default | ≥32 bytes; **required** outside dev |
| `FORAGEBUDDY_AUTHENTIK_ISSUER_URL` / `FORAGEBUDDY_OIDC_CLIENT_ID` / `FORAGEBUDDY_OIDC_CLIENT_SECRET` / `FORAGEBUDDY_OIDC_REDIRECT_URI` | localhost dev values | OIDC (Public/PKCE client; secret optional) |
| `FORAGEBUDDY_LLM_PROVIDER` | `litellm` | `litellm` or `openrouter` |
| `FORAGEBUDDY_LITELLM_API_KEY` / `FORAGEBUDDY_LITELLM_BASE_URL` | unset / `https://litellm.osmosis.page/v1` | |
| `FORAGEBUDDY_OPENROUTER_API_KEY` | unset | Used when provider=openrouter, and always for embeddings fallback |
| `FORAGEBUDDY_CHAT_MODEL` | `openrouter/~anthropic/claude-haiku-latest` | Must be vision-capable — used by the identification pipeline's candidate-gathering pass |
| `FORAGEBUDDY_IDENTIFICATION_MODEL` | same as `FORAGEBUDDY_CHAT_MODEL` | Model for the identification pipeline's facts + risks gatherers — off the candidate gatherer's time-critical path, so a stronger/slower model is a reasonable choice |
| `FORAGEBUDDY_ALLOWED_CHAT_MODELS` | same + `gemma4-26b` | comma-separated |
| `FORAGEBUDDY_EMBEDDING_MODEL` / `FORAGEBUDDY_EMBEDDING_DIM` | `bge-m3` / `1024` | |
| `FORAGEBUDDY_CORS_ORIGINS` | dev localhost + `http://tauri.localhost` | Comma-separated; replaces the list |
| `FORAGEBUDDY_MAX_PHOTO_BYTES` | `15728640` (15 MiB) | per-photo upload cap before resize |

## API surface (all `/api/*` require auth)
- `GET /health` · OIDC `/auth/login|/auth/callback|/auth/logout|/auth/me`
- **Sightings:**
  `POST /api/sightings` · `GET /api/sightings` · `GET /api/sightings/{id}`
  (sighting + photos + latest identification, one aggregate view) ·
  `PATCH /api/sightings/{id}`
- **Photos:** `POST /api/sightings/{id}/photos` (multipart, field `photo`) ·
  `GET /api/photos/{id}/file`
- **Identification:** `GET /api/sightings/{id}/identification` (history) ·
  `POST /api/sightings/{id}/identification` (manual re-run/retry)
- **Weather:** `GET /api/weather?lat=&lon=` — last 14 days of daily historical
  weather (temp/precipitation/wind/humidity) for a location, via Open-Meteo's
  free Archive API. Groundwork for future season/weather-aware foraging
  suggestions; not surfaced in the UI yet.

Every error response is `{"message": "..."}`. Full request/response shapes and the
identification pipeline design: **`docs/ARCHITECTURE.md`**.

## Deploy / CI
- GitHub Actions: `ci-required` (fmt + build + test + clippy gate + frontend),
  `docker-publish` (GHCR image), `android-nightly` / `android-release` (signed APKs).
  See `.github/workflows/`.
- Single-container image (`Dockerfile`, 3-stage: frontend build, backend build,
  runtime) + `deploy/docker-compose.yml` (GHCR image behind an external Caddy proxy
  network, named volumes for the DB and uploaded photos).
- Deploy secrets live in `deploy/.env` (gitignored).

## Safety disclaimer + poisonous warning (frontend-only)

A forced-acknowledgment modal ("Results are fetched by AI and can therefore
be wildly inaccurate. Say it with me: never munch on a hunch." — the user
must type that phrase back to continue) gates every identification result,
plus a separate warning popup whenever any candidate is flagged
`poisonous: true`. Both are pure UI gates with no backend enforcement — see
`docs/ARCHITECTURE.md`'s "Safety disclaimer + poisonous warning" section.

---

## Manual steps before this is live

Everything above is built and wired, but the following need a human (an API key,
an account, a DNS entry — things Claude cannot create on its own):

1. **Create the Authentik OIDC application.** In your existing Authentik instance,
   add a new application named `forage-buddy` (Public client, PKCE, no client
   secret) — same way you presumably set up `ai_buddy`'s `buddy` application. Then
   fill in the real `FORAGEBUDDY_OIDC_CLIENT_ID` and `FORAGEBUDDY_AUTHENTIK_ISSUER_URL`
   in `deploy/.env`.
   - **Zero-setup alternative:** skip this entirely and set `FORAGEBUDDY_DEV_MODE=true`
     for pure personal/local use — auth is bypassed and everything runs as a single
     implicit `admin` user.
2. **Set a real `FORAGEBUDDY_COOKIE_SIGNING_KEY`** (≥32 random bytes) for any
   non-dev deployment — the built-in default is an insecure placeholder.
3. **Set an LLM API key** — `FORAGEBUDDY_LITELLM_API_KEY` (default provider) or
   `FORAGEBUDDY_OPENROUTER_API_KEY` (if you switch `FORAGEBUDDY_LLM_PROVIDER=openrouter`).
   Without one, identification requests return a clear "LLM API key not
   configured" error — they won't crash the app.
4. **(Optional) Generate an Android signing keystore** and set the four
   `ANDROID_KEYSTORE_BASE64` / `ANDROID_KEYSTORE_PASSWORD` / `ANDROID_KEY_ALIAS` /
   `ANDROID_KEY_PASSWORD` GitHub Actions repo secrets, for stable-signed nightly and
   release APKs. Without them, nightlies still build but are debug-signed with an
   unstable per-run identity (see the comments in `android-nightly.yml`).
5. **Make the GHCR package public** the first time `docker-publish.yml` runs (GitHub
   → Packages → `forage_buddy` → package settings), or `docker login ghcr.io` on the
   deploy host with a `read:packages` token — same one-time step as `ai_buddy`.
6. **Point a domain + Caddy entry at this container** on the `caddy_proxy` network.
   Explicitly out of scope here — you said you'll handle the actual subdomain and
   reverse-proxy config yourself. `deploy/docker-compose.yml`'s `FORAGEBUDDY_BASE_URL`
   is left as an editable placeholder (`https://foraging.osmosis.page`, a guess at your
   subdomain convention) — change it to match whatever you set up.
