-- Brute-force-cosine RAG store: embeddings as little-endian f32 BLOBs, no
-- vector-DB dependency. Shared by two independent corpora distinguished by
-- `kind` ('wikipedia' article chunks, 'confusant' curated notes). Owned by
-- the `vector` module. See docs/ARCHITECTURE.md's "Database schema" section
-- (this file must match it verbatim).

CREATE TABLE embeddings (
    id         TEXT PRIMARY KEY,
    kind       TEXT NOT NULL,   -- 'wikipedia' | 'confusant'
    source_id  TEXT NOT NULL,   -- wikipedia_pages.title, or confusant_pairs.id
    model      TEXT NOT NULL,
    dim        INTEGER NOT NULL,
    vec        BLOB NOT NULL,   -- dim * 4 bytes, little-endian f32
    text       TEXT NOT NULL,   -- chunk text, returned by search for RAG
    created_at TEXT NOT NULL
);
CREATE INDEX idx_embeddings_kind_source ON embeddings(kind, source_id);
