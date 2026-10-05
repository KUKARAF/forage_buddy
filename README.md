# Forage Buddy

> Working name. An AI foraging-identification helper: you photograph a
> plant/fungus/etc. in the field, the app records photo + GPS location +
> timestamp, a fast **triage** pass tells you the likely genus (or says "I
> don't know, send more photos" when it can't), and a slower **deep dive**
> pass grounds the best guess in its Wikipedia article and surfaces
> dangerous look-alikes ("confusant species") with a checklist of what to
> check to rule them out.

**Safety is the entire point of the deep-dive stage.** Misidentifying a
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
  default, OpenRouter as the revert switch), **plus vision** — the triage pass sends
  photos as `image_url` content parts, so the configured model must support image
  input. No wallet/billing/cost-estimation code (this app has no payments).
- **RAG:** brute-force cosine search over a `BLOB` embeddings column in SQLite — no
  extra service — for two corpora: Wikipedia article chunks and curated confusant
  notes.
- **Frontend:** SvelteKit (adapter-static) + Tauri 2 Android.
- **No wallet, no Stripe, no circles, no MCP, no FCM/push, no GeoIP.** Forage Buddy is
  a single-purpose identification tool; it stays that small.

## Workspace layout
```
crates/core     forage-buddy-core: shared domain enums + device-token gen (no axum/sqlx/tauri)
crates/server   the axum binary: auth, db, llm, photos, sightings, triage, species, wikipedia, vector, deepdive
crates/mobile   Tauri 2 Android app wrapping web/build
web/            SvelteKit SPA (capture → triage → deep-dive screens)
crates/server/migrations/
  0001_init.sql                users, device_tokens
  0002_sightings.sql           sightings table
  0003_photos.sql               photos table
  0004_triage.sql               triage_results table
  0005_species_reference.sql   curated species_reference + confusant_pairs (seeded)
  0006_wikipedia.sql            wikipedia_pages cache
  0007_vectors.sql               embeddings table (RAG, shared by wikipedia + confusant kinds)
  0008_deepdive.sql              deepdive_results table
```
Full design + module contracts: **`docs/ARCHITECTURE.md`**.

## Run locally (dev mode — auth bypassed, user "admin")
```bash
# Backend (bypasses OIDC, binds loopback only):
FORAGEBUDDY_ENV=dev cargo run -p server
# → listening on 127.0.0.1:8080 ; GET /health → ok ; GET /auth/me → the admin user

# To exercise triage/deep-dive, add an LLM key (needs a vision-capable model):
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
| `FORAGEBUDDY_CHAT_MODEL` | `openrouter/~anthropic/claude-haiku-latest` | Must be vision-capable — used for triage AND deep-dive |
| `FORAGEBUDDY_ALLOWED_CHAT_MODELS` | same + `gemma4-26b` | comma-separated |
| `FORAGEBUDDY_EMBEDDING_MODEL` / `FORAGEBUDDY_EMBEDDING_DIM` | `bge-m3` / `1024` | |
| `FORAGEBUDDY_CORS_ORIGINS` | dev localhost + `http://tauri.localhost` | Comma-separated; replaces the list |
| `FORAGEBUDDY_MAX_PHOTO_BYTES` | `15728640` (15 MiB) | per-photo upload cap before resize |

## API surface (all `/api/*` require auth)
- `GET /health` · OIDC `/auth/login|/auth/callback|/auth/logout|/auth/me`
- **Sightings:**
  `POST /api/sightings` · `GET /api/sightings` · `GET /api/sightings/{id}`
  (sighting + photos + latest triage + latest deepdive, one aggregate view) ·
  `PATCH /api/sightings/{id}`
- **Photos:** `POST /api/sightings/{id}/photos` (multipart, field `photo`) ·
  `GET /api/photos/{id}/file`
- **Triage:** `GET /api/sightings/{id}/triage` (history) · `POST /api/sightings/{id}/triage`
  (manual re-run)
- **Deep dive:** `POST /api/sightings/{id}/deepdive` · `GET /api/sightings/{id}/deepdive`

Every error response is `{"message": "..."}`. Full request/response shapes and the
triage/deep-dive LLM pipeline design: **`docs/ARCHITECTURE.md`**.

## Deploy / CI
- GitHub Actions: `ci-required` (fmt + build + test + clippy gate + frontend),
  `docker-publish` (GHCR image), `android-nightly` / `android-release` (signed APKs).
  See `.github/workflows/`.
- Single-container image (`Dockerfile`, 3-stage: frontend build, backend build,
  runtime) + `deploy/docker-compose.yml` (GHCR image behind an external Caddy proxy
  network, named volumes for the DB and uploaded photos).
- Deploy secrets live in `deploy/.env` (gitignored).

## Safety disclaimer (shown on every screen with a species guess)

> "Forage Buddy gives a best-effort AI guess, not a confirmed identification.
> Never eat, use, or handle anything based solely on this app. Misidentifying
> a wild plant or fungus can cause severe illness or death. Confirm with a
> qualified local expert, a spore print, and multiple field guides before
> consuming or using anything you forage."

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
   Without one, triage and deep-dive requests return a clear "LLM API key not
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
