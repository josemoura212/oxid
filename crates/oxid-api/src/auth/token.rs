//! API tokens: the credential for clients that are not a browser tab.
//!
//! The browser extension cannot use the session cookie — extensions do not share
//! cookies with the site in any way that holds across browsers — and should not
//! want to: a token is revocable on its own, so uninstalling the extension or
//! losing the laptop it runs on does not mean signing out everywhere.
//!
//! **Why this is not hashed like a password.** `users.password_hash` uses Argon2,
//! and copying that here would be wrong twice over. Argon2 is slow deliberately,
//! to price guessing a low-entropy human secret; a token is 256 bits from the
//! operating system's random source, so there is no dictionary to walk and the
//! slowness buys nothing. And a per-row salt would make lookup impossible —
//! authenticating would mean trying every row in the table. A plain digest is a
//! key, which is exactly what is needed.
//!
//! The table still stores only the digest, so a database read hands nobody
//! something they can replay.

use argon2::password_hash::rand_core::{OsRng, RngCore};
use sha2::{Digest, Sha256};

/// Bytes of randomness behind a token. 32 is 256 bits — past any brute force
/// worth modelling, and the same width the session id uses.
const TOKEN_BYTES: usize = 32;

/// Marks the string as ours.
///
/// Useful in logs and in a support message, but the reason it is worth the
/// characters is the secret scanners: `GitGuardian` already runs on this
/// repository's CI, and a prefixed token is one it can be taught to recognise
/// when somebody pastes it into a commit.
pub const TOKEN_PREFIX: &str = "oxid_pat_";

/// A freshly minted token: the secret to hand over once, and the digest to store.
///
/// The two are separate fields rather than one struct with a method, so a caller
/// cannot accidentally persist the secret — the only thing that reaches the
/// database is the field named for it.
#[derive(Debug)]
pub struct Minted {
    /// Shown to the person exactly once. Never stored, never recoverable.
    pub secret: String,
    /// What goes in `api_tokens.token_hash`.
    pub hash: String,
}

/// Mints a token.
///
/// The secret is returned rather than stored anywhere, and the caller is expected
/// to show it once. That is not ceremony: it is what makes storing only a digest
/// meaningful. A token the server could reproduce is a token the server could
/// leak.
pub fn mint() -> Minted {
    let mut bytes = [0u8; TOKEN_BYTES];
    OsRng.fill_bytes(&mut bytes);

    let secret = format!("{TOKEN_PREFIX}{}", hex::encode(bytes));
    let hash = digest(&secret);

    Minted { secret, hash }
}

/// The lookup key for a presented token.
///
/// Hex of SHA-256 over the whole string, prefix included — the prefix is part of
/// what the person pastes, so it has to be part of what is hashed, or a token
/// would verify with or without it.
pub fn digest(secret: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(secret.as_bytes());
    hex::encode(hasher.finalize())
}

/// Pulls a bearer token out of an `Authorization` header value.
///
/// Returns `None` for anything that is not `Bearer <our token>`, which includes
/// a `Basic` credential, a bearer token belonging to something else, and the
/// empty string. Checking the prefix here means a malformed header costs no
/// database round trip.
///
/// The scheme match is case-insensitive because RFC 9110 says it is, and a client
/// sending `bearer` is not wrong.
pub fn from_header(value: &str) -> Option<&str> {
    let (scheme, credential) = value.split_once(' ')?;

    if !scheme.eq_ignore_ascii_case("bearer") {
        return None;
    }

    let credential = credential.trim();
    credential.starts_with(TOKEN_PREFIX).then_some(credential)
}

#[cfg(test)]
mod tests {
    use super::{TOKEN_PREFIX, digest, from_header, mint};

    #[test]
    fn a_minted_token_carries_the_prefix_and_hashes_to_its_digest() {
        let minted = mint();

        assert!(minted.secret.starts_with(TOKEN_PREFIX));
        assert_eq!(digest(&minted.secret), minted.hash);
        // Hex of SHA-256 is 64 characters; anything else means the digest
        // changed shape and the column's UNIQUE constraint is now guarding a
        // different thing.
        assert_eq!(minted.hash.len(), 64);
    }

    /// The property the whole design rests on: two tokens never collide, so the
    /// digest can be a lookup key.
    #[test]
    fn two_tokens_are_never_the_same() {
        let a = mint();
        let b = mint();

        assert_ne!(a.secret, b.secret);
        assert_ne!(a.hash, b.hash);
    }

    /// The stored digest must not be reversible to the secret by any path the
    /// code offers — including hashing the hash, which is the mistake that would
    /// make the column self-verifying.
    #[test]
    fn the_digest_is_not_the_secret() {
        let minted = mint();

        assert_ne!(minted.hash, minted.secret);
        assert_ne!(digest(&minted.hash), minted.hash);
    }

    #[test]
    fn only_a_bearer_token_of_ours_is_extracted() {
        let minted = mint();
        let header = format!("Bearer {}", minted.secret);

        assert_eq!(from_header(&header), Some(minted.secret.as_str()));
        // RFC 9110 makes the scheme case-insensitive.
        assert_eq!(
            from_header(&format!("bearer {}", minted.secret)),
            Some(minted.secret.as_str())
        );

        assert_eq!(from_header("Basic dXNlcjpwYXNz"), None);
        assert_eq!(from_header("Bearer some-other-services-token"), None);
        assert_eq!(from_header("Bearer"), None);
        assert_eq!(from_header(""), None);
    }
}
