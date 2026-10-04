//! Public Wikipedia REST client (no API key needed), with a SQLite cache
//! (migration `0006_wikipedia.sql`).
//!
//! Deliberately kept free of [`crate::llm::LlmClient`]: this module's only
//! job is "get me a page's text, from cache or the network". Chunking the
//! extract into embeddable pieces is a pure text operation and lives here
//! too ([`chunk_extract`]), but *calling* the embeddings endpoint and
//! writing into the `vector` store is the `deepdive` module's job — that
//! keeps this module's dependency graph trivial to reason about and testable
//! without any LLM/network mocking.
//!
//! Wikipedia being slow, down, or returning something unparseable must never
//! break deep-dive entirely (see `docs/ARCHITECTURE.md`'s fallback-to-seed-
//! data requirement): any network or parse failure from the live API makes
//! [`fetch_or_cache`] return `Ok(None)`, not an error. Only a genuine local
//! database error propagates as `Err`.

use std::time::Duration;

use anyhow::Context;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

use crate::error::{AppError, AppResult};

/// Per-request timeout for the Wikipedia REST API.
const HTTP_TIMEOUT: Duration = Duration::from_secs(15);

/// Lower bound, in characters, for a chunk produced by [`chunk_extract`]
/// (except possibly the final chunk).
const MIN_CHUNK_CHARS: usize = 800;
/// Upper bound, in characters, for a chunk produced by [`chunk_extract`].
const MAX_CHUNK_CHARS: usize = 1200;

/// A resolved Wikipedia page, from cache or freshly fetched.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WikipediaPage {
    pub title: String,
    pub pageid: Option<i64>,
    pub url: String,
    pub extract: String,
}

/// Build the `reqwest::Client` this module's HTTP calls use. Deliberately
/// separate from [`crate::llm::LlmClient`]'s own client — Wikipedia is an
/// unrelated outbound dependency with its own lifecycle. No redirects
/// (defense in depth / SSRF hygiene, same rationale as the OIDC and LLM
/// clients) and a bounded timeout so a slow/hanging Wikipedia can't stall a
/// deep-dive request indefinitely.
pub fn build_client() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(HTTP_TIMEOUT)
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

/// Looks up the cache (`wikipedia_pages` table) first; on miss, fetches from
/// the live REST summary API, caches, and returns. On any network/parse
/// failure returns `Ok(None)` rather than an error — a flaky/unreachable
/// Wikipedia must never fail a deep-dive outright, just leave it ungrounded.
/// A genuine local database error (reading or writing the cache) still
/// propagates as `Err`.
pub async fn fetch_or_cache(
    db: &SqlitePool,
    http: &reqwest::Client,
    title: &str,
) -> AppResult<Option<WikipediaPage>> {
    let title = title.trim();
    if title.is_empty() {
        return Ok(None);
    }

    if let Some(cached) = load_cached(db, title).await? {
        return Ok(Some(cached));
    }

    match fetch_live(http, title).await {
        Ok(Some(page)) => {
            if let Err(err) = store_cached(db, &page).await {
                tracing::warn!(error = %err, title, "wikipedia: failed to cache fetched page");
            }
            Ok(Some(page))
        }
        Ok(None) => Ok(None),
        Err(err) => {
            tracing::warn!(
                error = %err,
                title,
                "wikipedia: live fetch failed; continuing without grounding"
            );
            Ok(None)
        }
    }
}

/// Splits `text` into paragraph-aligned chunks of roughly
/// [`MIN_CHUNK_CHARS`]..=[`MAX_CHUNK_CHARS`] characters, suitable for
/// embedding. Paragraphs are detected by blank lines; a single giant
/// paragraph (or any extract shorter than the minimum) is returned as one
/// chunk rather than split mid-sentence.
pub fn chunk_extract(text: &str) -> Vec<String> {
    let paragraphs: Vec<&str> = text
        .split("\n\n")
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .collect();

    let mut chunks = Vec::new();
    let mut current = String::new();

    for para in paragraphs {
        let would_overflow = !current.is_empty()
            && current.chars().count() + para.chars().count() + 2 > MAX_CHUNK_CHARS;
        if would_overflow {
            chunks.push(std::mem::take(&mut current));
        }
        if !current.is_empty() {
            current.push_str("\n\n");
        }
        current.push_str(para);

        if current.chars().count() >= MIN_CHUNK_CHARS {
            chunks.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        chunks.push(current);
    }

    if chunks.is_empty() {
        let trimmed = text.trim();
        if !trimmed.is_empty() {
            chunks.push(trimmed.to_string());
        }
    }

    chunks
}

async fn load_cached(db: &SqlitePool, title: &str) -> AppResult<Option<WikipediaPage>> {
    let row: Option<(Option<i64>, String, String)> =
        sqlx::query_as("SELECT pageid, url, extract FROM wikipedia_pages WHERE title = ?")
            .bind(title)
            .fetch_optional(db)
            .await
            .map_err(|e| AppError::Internal(e.into()))?;

    Ok(row.map(|(pageid, url, extract)| WikipediaPage {
        title: title.to_string(),
        pageid,
        url,
        extract,
    }))
}

async fn store_cached(db: &SqlitePool, page: &WikipediaPage) -> AppResult<()> {
    sqlx::query(
        "INSERT INTO wikipedia_pages (title, pageid, url, extract, fetched_at) \
         VALUES (?, ?, ?, ?, datetime('now')) \
         ON CONFLICT(title) DO UPDATE SET \
             pageid = excluded.pageid, url = excluded.url, extract = excluded.extract, \
             fetched_at = excluded.fetched_at",
    )
    .bind(&page.title)
    .bind(page.pageid)
    .bind(&page.url)
    .bind(&page.extract)
    .execute(db)
    .await
    .map_err(|e| AppError::Internal(e.into()))?;

    Ok(())
}

/// Raw shape of `GET /api/rest_v1/page/summary/{title}`.
#[derive(Debug, Deserialize)]
struct SummaryResponse {
    title: String,
    #[serde(default)]
    pageid: Option<i64>,
    #[serde(default)]
    extract: String,
    #[serde(default)]
    content_urls: Option<ContentUrls>,
}

#[derive(Debug, Deserialize)]
struct ContentUrls {
    desktop: Option<DesktopUrls>,
}

#[derive(Debug, Deserialize)]
struct DesktopUrls {
    page: Option<String>,
}

/// `https://en.wikipedia.org/wiki/{title}`, percent-encoded via `Url`'s path
/// segment API, used when the summary response has no `content_urls`.
fn default_page_url(title: &str) -> String {
    match reqwest::Url::parse("https://en.wikipedia.org/wiki") {
        Ok(mut url) => {
            if let Ok(mut segments) = url.path_segments_mut() {
                segments.push(title);
            }
            url.to_string()
        }
        // Base URL is a compile-time constant, so this branch is
        // unreachable in practice; fall back to a plain concatenation
        // rather than panicking.
        Err(_) => format!("https://en.wikipedia.org/wiki/{title}"),
    }
}

async fn fetch_live(http: &reqwest::Client, title: &str) -> anyhow::Result<Option<WikipediaPage>> {
    let mut url = reqwest::Url::parse("https://en.wikipedia.org/api/rest_v1/page/summary")
        .context("parsing wikipedia summary base url")?;
    url.path_segments_mut()
        .map_err(|_| anyhow::anyhow!("wikipedia summary url cannot be used as a base"))?
        .push(title);

    let resp = http
        .get(url)
        .header("User-Agent", "forage-buddy/0.1 (safety research aid)")
        .send()
        .await
        .context("requesting wikipedia summary")?;

    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !resp.status().is_success() {
        anyhow::bail!("wikipedia summary returned status {}", resp.status());
    }

    let body: SummaryResponse = resp
        .json()
        .await
        .context("parsing wikipedia summary json")?;
    if body.extract.trim().is_empty() {
        return Ok(None);
    }

    let page_url = body
        .content_urls
        .as_ref()
        .and_then(|c| c.desktop.as_ref())
        .and_then(|d| d.page.clone())
        .unwrap_or_else(|| default_page_url(&body.title));

    Ok(Some(WikipediaPage {
        title: body.title,
        pageid: body.pageid,
        url: page_url,
        extract: body.extract,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn test_pool() -> (tempfile::TempDir, SqlitePool) {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let pool = crate::db::init_pool(db_path.to_str().unwrap())
            .await
            .unwrap();
        (dir, pool)
    }

    #[test]
    fn chunk_extract_splits_long_text_on_paragraphs() {
        let para = "x".repeat(900);
        let text = format!("{para}\n\n{para}\n\n{para}");
        let chunks = chunk_extract(&text);
        assert!(chunks.len() >= 2);
        for chunk in &chunks {
            assert!(chunk.chars().count() <= MAX_CHUNK_CHARS + 2);
        }
    }

    #[test]
    fn chunk_extract_keeps_short_text_as_one_chunk() {
        let text = "A short Wikipedia extract.\n\nJust two short paragraphs.";
        let chunks = chunk_extract(text);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks.first().unwrap(), text);
    }

    #[test]
    fn chunk_extract_handles_empty_text() {
        assert!(chunk_extract("").is_empty());
        assert!(chunk_extract("   \n\n  ").is_empty());
    }

    #[tokio::test]
    async fn cache_roundtrip() {
        let (_dir, pool) = test_pool().await;

        assert!(load_cached(&pool, "Amanita phalloides")
            .await
            .unwrap()
            .is_none());

        let page = WikipediaPage {
            title: "Amanita phalloides".to_string(),
            pageid: Some(42),
            url: "https://en.wikipedia.org/wiki/Amanita_phalloides".to_string(),
            extract: "The death cap is a deadly poisonous mushroom.".to_string(),
        };
        store_cached(&pool, &page).await.unwrap();

        let cached = load_cached(&pool, "Amanita phalloides").await.unwrap();
        assert_eq!(cached.map(|p| p.extract), Some(page.extract));
    }

    #[tokio::test]
    async fn fetch_or_cache_returns_none_for_empty_title() {
        let (_dir, pool) = test_pool().await;
        let http = build_client();
        let result = fetch_or_cache(&pool, &http, "   ").await.unwrap();
        assert!(result.is_none());
    }
}
