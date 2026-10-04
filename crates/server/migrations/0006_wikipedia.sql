-- Cache of fetched Wikipedia page summaries, keyed by canonical title. Owned
-- by the `wikipedia` module. See docs/ARCHITECTURE.md's "Database schema"
-- section (this file must match it verbatim).

CREATE TABLE wikipedia_pages (
    title       TEXT PRIMARY KEY,   -- canonical page title, used as cache key
    pageid      INTEGER,
    url         TEXT NOT NULL,
    extract     TEXT NOT NULL,      -- plain-text extract (REST API "extracts")
    fetched_at  TEXT NOT NULL
);
