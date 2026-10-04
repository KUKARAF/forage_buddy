-- A photo attached to a sighting. Owned by the `photos` module. See
-- docs/ARCHITECTURE.md's "Database schema" section (this file must match it
-- verbatim).

CREATE TABLE photos (
    id           TEXT PRIMARY KEY,
    sighting_id  TEXT NOT NULL REFERENCES sightings(id),
    file_path    TEXT NOT NULL,     -- relative to FORAGEBUDDY_PHOTO_DIR
    content_type TEXT NOT NULL,
    width        INTEGER,
    height       INTEGER,
    taken_at     TEXT NOT NULL,     -- RFC3339, client-supplied capture time (defaults to upload time)
    sort_order   INTEGER NOT NULL,
    created_at   TEXT NOT NULL
);
CREATE INDEX idx_photos_sighting ON photos(sighting_id, sort_order);
