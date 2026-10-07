-- Runtime-editable overrides for which LLM model each identification
-- gatherer uses (see `settings` module). A singleton row (id always 1);
-- absence of the row, or a NULL/empty column, means "use the
-- FORAGEBUDDY_*_MODEL env var default for that gatherer" — this table only
-- ever holds explicit overrides, never the defaults themselves.

CREATE TABLE model_settings (
    id                 INTEGER PRIMARY KEY CHECK (id = 1),
    candidate_model    TEXT,
    facts_model        TEXT,
    risk_model         TEXT,
    visual_match_model TEXT,
    updated_at         TEXT NOT NULL
);
