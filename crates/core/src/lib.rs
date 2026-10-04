//! `forage-buddy-core`: shared, platform-independent domain logic for Forage
//! Buddy.
//!
//! Used by both the `server` crate and the `mobile` (Tauri) crate, so it must
//! stay free of server-only or mobile-only dependencies (no `axum`, no
//! `tauri`, no `sqlx`).

pub mod device_token;
pub mod domain;
