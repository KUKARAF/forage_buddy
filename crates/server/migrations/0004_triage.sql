-- Fast "triage" pass results: one row per LLM attempt (or per no-photo /
-- unreadable-photo early-return) for a sighting. History is kept (never
-- overwritten) so the frontend can show progression as more photos answer
-- earlier `missing_info` asks.
CREATE TABLE triage_results (
    id             TEXT PRIMARY KEY,
    sighting_id    TEXT NOT NULL REFERENCES sightings(id),
    created_at     TEXT NOT NULL,
    model          TEXT NOT NULL,        -- "none" when no LLM call was made (e.g. zero photos)
    status         TEXT NOT NULL,        -- TriageStatus
    genus          TEXT,
    candidate_species_json TEXT NOT NULL, -- JSON array of {species, common_name, confidence}
    missing_info_json      TEXT NOT NULL, -- JSON array of strings, e.g. "photo of gill attachment"
    reasoning      TEXT NOT NULL,         -- short, shown to user as "why"
    photos_considered INTEGER NOT NULL    -- how many photos were actually sent to the model this attempt
);
CREATE INDEX idx_triage_sighting ON triage_results(sighting_id, created_at DESC);
