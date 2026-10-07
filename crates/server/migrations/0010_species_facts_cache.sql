-- Read-through cache of per-species facts (edible/medicinal/psychoactive/
-- poisonous + a short risk note + Wikipedia link) gathered once per species
-- by `identification::gather_facts` and reused across sightings/users.
-- `species_key` is the lowercased "genus species" (or bare genus) string —
-- same normalization as the curated `species::lookup` matching uses,
-- trimmed and lowercased.
--
-- Booleans are nullable TEXT-as-INTEGER (0/1) SQLite booleans: NULL means
-- "unknown", never a forced guess.
CREATE TABLE species_facts_cache (
    species_key     TEXT PRIMARY KEY,
    wikipedia_title  TEXT,
    wikipedia_url    TEXT,
    edible           INTEGER,
    medicinal        INTEGER,
    psychoactive     INTEGER,
    poisonous        INTEGER,
    danger_level     TEXT,
    risk_note        TEXT NOT NULL,
    source_model     TEXT NOT NULL,
    fetched_at       TEXT NOT NULL
);
