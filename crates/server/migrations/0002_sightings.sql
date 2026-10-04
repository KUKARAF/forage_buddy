-- A sighting is one foraging observation: a photo (or several), an optional
-- GPS fix, when it was observed, and free-text notes. Owned by the
-- `sightings` module. See docs/ARCHITECTURE.md's "Database schema" section
-- (this file must match it verbatim).

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
