//! Device-token generation.
//!
//! A device token is a cryptographically random, URL-safe, unguessable string
//! that is a long-lived, full-account bearer credential for the mobile app
//! (see `server::auth::device_token` — sent as `Authorization: Bearer` on REST
//! calls). The server stores only `SHA-256(raw)`, so the token gets a 256-bit
//! entropy budget to match that digest's preimage space.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;

/// Bytes of randomness per token: 32 bytes = 256 bits of entropy.
const TOKEN_BYTES: usize = 32;

/// Generate a new, cryptographically-random, URL-safe device token.
///
/// Backed by `rand`'s OS-seeded CSPRNG, base64url-encoded without padding so
/// the result can ride in a URL query parameter with no escaping. 32 bytes
/// encodes to 43 characters.
pub fn generate() -> String {
    let bytes: [u8; TOKEN_BYTES] = rand::random();
    URL_SAFE_NO_PAD.encode(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_url_safe_and_unpadded() {
        let token = generate();
        assert!(!token.is_empty());
        assert!(
            token
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
            "token {token:?} contains a character unsafe for a URL"
        );
        assert!(!token.contains('='), "no-pad encoding must not add '='");
    }

    #[test]
    fn tokens_have_256_bits_of_entropy() {
        let token = generate();
        assert_eq!(token.len(), 43);
    }

    #[test]
    fn tokens_are_unique() {
        let tokens: std::collections::HashSet<String> = (0..2000).map(|_| generate()).collect();
        assert_eq!(
            tokens.len(),
            2000,
            "2000 generated tokens must all be distinct"
        );
    }
}
