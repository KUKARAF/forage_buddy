//! Authentication and authorization.
//!
//! `oidc` mounts the `/auth/*` routes (login/callback/logout/me), `session`
//! provides the `tower-sessions` helpers and the `RequireAuth` extractor
//! protected routes use, and `device_token` is the bearer-token credential
//! store for the mobile app.

pub mod device_token;
pub mod oidc;
pub mod session;
