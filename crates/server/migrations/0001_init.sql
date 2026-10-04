-- Initial schema for Forage Buddy.
--
-- `tower-sessions-sqlx-store` manages its own `tower_sessions` table against
-- this same pool/file; it is intentionally not created here.

CREATE TABLE users (
    id           TEXT PRIMARY KEY,   -- OIDC `sub`
    email        TEXT,
    display_name TEXT,
    created_at   TEXT NOT NULL
);

-- Long-lived bearer credentials for the mobile app. Only SHA-256(raw) is
-- stored. Ported from ai_buddy's device_tokens table.
CREATE TABLE device_tokens (
    token_hash   TEXT PRIMARY KEY,
    user_id      TEXT NOT NULL REFERENCES users(id),
    label        TEXT NOT NULL,
    created_at   TEXT NOT NULL,
    last_used_at TEXT NOT NULL,
    expires_at   TEXT NOT NULL
);

CREATE INDEX idx_device_tokens_user ON device_tokens(user_id);
