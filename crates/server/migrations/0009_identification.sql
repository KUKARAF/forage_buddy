-- The single "identification" pipeline: one row per identification attempt
-- for a sighting (history kept, never overwritten), replacing the old
-- separate triage_results + deepdive_results two-stage flow. Owned by the
-- `identification` module.
--
-- `status` lifecycle: `pending` (row just created, gatherer 1 not finished
-- yet) -> `partial` (candidates found, facts/risks gatherers still running
-- or one of them failed) -> `complete` (fully enriched) or `insufficient`
-- (gatherer 1 found no usable candidates) or `failed` (an unrecoverable
-- error occurred partway through — never left stuck at `pending`/`partial`
-- forever).
CREATE TABLE identification_results (
    id                 TEXT PRIMARY KEY,
    sighting_id        TEXT NOT NULL REFERENCES sightings(id),
    created_at         TEXT NOT NULL,
    updated_at         TEXT NOT NULL,
    status             TEXT NOT NULL CHECK(status IN ('pending','partial','complete','insufficient','failed')),
    model              TEXT NOT NULL,
    candidates_json    TEXT NOT NULL DEFAULT '[]',   -- JSON array, see IdentificationResultDto
    missing_info_json  TEXT NOT NULL DEFAULT '[]',   -- JSON array of strings
    photos_considered  INTEGER NOT NULL
);
CREATE INDEX idx_identification_sighting ON identification_results(sighting_id, created_at DESC);
