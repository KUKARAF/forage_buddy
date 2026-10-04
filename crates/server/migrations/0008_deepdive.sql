-- The slower "deep dive" pass: one row per deep-dive attempt for a sighting
-- (history kept, never overwritten). Owned by the `deepdive` module. See
-- docs/ARCHITECTURE.md's "Database schema" section (this file must match it
-- verbatim).

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
