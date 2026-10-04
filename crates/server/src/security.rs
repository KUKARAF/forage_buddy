//! Defense-in-depth HTTP response security headers.
//!
//! This is a backstop: even if some future rendering path has a sanitization
//! flaw, these headers (primarily the Content-Security-Policy) block script
//! execution from remote origins and block data exfiltration via
//! `connect-src`/`object-src`. Applied to EVERY response.

use axum::extract::Request;
use axum::http::header::{
    HeaderName, HeaderValue, CONTENT_SECURITY_POLICY, REFERRER_POLICY, X_CONTENT_TYPE_OPTIONS,
    X_FRAME_OPTIONS,
};
use axum::middleware::Next;
use axum::response::Response;

/// The Content-Security-Policy served with the website (same-origin SPA + API).
///
/// Directive rationale:
/// - `default-src 'self'` — deny-by-default.
/// - `base-uri 'self'` — stop injected `<base>` tags from rewriting relative-URL resolution.
/// - `object-src 'none'` — no legacy plugin-based script/exfil vectors.
/// - `frame-ancestors 'none'` — clickjacking defense.
/// - `form-action 'self'`.
/// - `img-src 'self' data: blob:` — the app renders uploaded sighting photos
///   (served same-origin) plus local data/blob previews during capture.
/// - `script-src 'self' 'unsafe-inline'` — the SvelteKit static build emits an
///   inline bootstrap `<script>` with no nonce; this still blocks all remote
///   script loads.
/// - `style-src 'self' 'unsafe-inline'` — Svelte injects `<style>` blocks.
/// - `connect-src 'self'` — the key exfiltration guard: same-origin API only.
/// - `font-src 'self' data:`.
const WEB_CSP: &str = "default-src 'self'; \
base-uri 'self'; \
object-src 'none'; \
frame-ancestors 'none'; \
form-action 'self'; \
img-src 'self' data: blob:; \
script-src 'self' 'unsafe-inline'; \
style-src 'self' 'unsafe-inline'; \
connect-src 'self'; \
font-src 'self' data:";

/// `Permissions-Policy` has no associated constant in the `http` crate, so the
/// name is built from a static string (infallible, `const`-friendly).
const PERMISSIONS_POLICY_HEADER: HeaderName = HeaderName::from_static("permissions-policy");

/// Unlike `ai_buddy`, this app's whole point is capturing a photo + GPS
/// location in the browser/webview, so geolocation and camera must be
/// allowed for same-origin use (camera access here is via a plain
/// `<input capture>`, which some browsers also gate on this policy).
const PERMISSIONS_POLICY_VALUE: &str = "geolocation=(self), camera=(self), microphone=()";

/// Axum middleware that stamps defense-in-depth security headers onto every
/// response. Applied once, at the outermost layer in `main.rs`.
///
/// Note: HSTS is intentionally NOT set here — TLS/HSTS is terminated and
/// managed by the external reverse proxy (Caddy).
pub async fn set_security_headers(req: Request, next: Next) -> Response {
    let mut res = next.run(req).await;
    let headers = res.headers_mut();

    headers.insert(CONTENT_SECURITY_POLICY, HeaderValue::from_static(WEB_CSP));
    headers.insert(X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    headers.insert(
        REFERRER_POLICY,
        HeaderValue::from_static("strict-origin-when-cross-origin"),
    );
    headers.insert(X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    headers.insert(
        PERMISSIONS_POLICY_HEADER,
        HeaderValue::from_static(PERMISSIONS_POLICY_VALUE),
    );

    res
}
