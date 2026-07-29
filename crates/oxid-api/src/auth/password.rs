//! Password hashing.
//!
//! Argon2id is slow on purpose — that is the entire defence against an offline
//! attack on a leaked table. The cost is paid by this server on every login, so
//! the parameters below are a deliberate trade, not a default carried over from
//! an example.

use std::{sync::Arc, time::Duration};

use argon2::{
    Algorithm, Argon2, Params, PasswordHash, PasswordHasher, PasswordVerifier, Version,
    password_hash::{SaltString, rand_core::OsRng},
};
use tokio::sync::{Semaphore, SemaphorePermit};

use crate::error::AppError;

/// Memory cost, in kibibytes. 19 MiB is the OWASP recommendation for Argon2id
/// at `t = 2`.
///
/// It is also what decides how expensive a login flood is: this much is
/// allocated per verification in flight. On a small node that ceiling matters
/// more than the hash strength itself, which is why the login route carries its
/// own rate limit instead of sharing the one on writes.
const MEMORY_KIB: u32 = 19 * 1024;

/// Iterations, paired with the memory cost above.
const ITERATIONS: u32 = 2;

/// Lanes. One, because parallelism buys attack resistance only if the defender
/// also has cores to spare — and this one does not.
const PARALLELISM: u32 = 1;

fn hasher() -> Result<Argon2<'static>, AppError> {
    let params = Params::new(MEMORY_KIB, ITERATIONS, PARALLELISM, None)
        .map_err(|_| AppError::Internal("invalid argon2 parameters"))?;

    Ok(Argon2::new(Algorithm::Argon2id, Version::V0x13, params))
}

/// Returns a PHC string: algorithm, parameters and salt travel with the digest,
/// so raising the cost later does not invalidate stored passwords.
pub fn hash(password: &str) -> Result<String, AppError> {
    let salt = SaltString::generate(&mut OsRng);

    hasher()?
        .hash_password(password.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|_| AppError::Internal("failed to hash password"))
}

/// Verifies against a stored hash.
///
/// A malformed stored hash verifies as `false` rather than erroring: it means
/// one row is corrupt, and answering 500 would point an attacker at exactly
/// which account that is.
pub fn verify(password: &str, stored: &str) -> bool {
    let Ok(parsed) = PasswordHash::new(stored) else {
        tracing::error!("stored password hash is malformed");
        return false;
    };

    let Ok(argon2) = hasher() else {
        return false;
    };

    argon2.verify_password(password.as_bytes(), &parsed).is_ok()
}

/// A hash of a password nobody knows, verified against when the e-mail does not
/// exist so both paths cost the same.
///
/// Built at boot rather than hardcoded. A constant would have to be a real PHC
/// string, and an invalid one fails to parse — sending [`verify`] down its early
/// return, spending none of the CPU this exists to spend and silently restoring
/// the timing oracle. Generating it means it cannot be wrong.
///
/// Without it, response time answers "does this account exist?": sub-millisecond
/// for no, tens of milliseconds for yes. Returning identical messages in both
/// cases does not help if the clock says otherwise.
#[derive(Debug, Clone)]
pub struct Decoy(String);

impl Decoy {
    /// Fails the boot rather than degrading. A service that cannot build this
    /// cannot serve logins without leaking which accounts exist.
    pub fn generate() -> Result<Self, AppError> {
        let salt = SaltString::generate(&mut OsRng);
        hash(salt.as_str()).map(Self)
    }
}

/// Runs the hashing, off the runtime and under a cap.
///
/// Two problems, and the second is why this type exists at all.
///
/// **Argon2 blocks.** It is CPU-bound and synchronous, so calling it straight
/// from an `async` handler occupies a Tokio worker for the whole verification.
/// On a two-core node there are two workers: two simultaneous logins occupy both
/// and the runtime stops serving anything — redirects included. An attacker does
/// not need to saturate the CPU, only to open two connections. Hence
/// `spawn_blocking`, which uses the pool meant for work that blocks.
///
/// **Identifying the caller is not reliable.** The per-IP limit sits in front of
/// this, and it has already failed silently once behind a CDN. The semaphore
/// does not care who is calling: it bounds how much Argon2 can be in flight at
/// all, so a flood becomes a queue instead of a saturated node. That is the
/// difference between defence in depth and two copies of the same assumption.
#[derive(Debug, Clone)]
pub struct Hasher {
    slots: Arc<Semaphore>,
    wait: Duration,
    decoy: Decoy,
}

impl Hasher {
    pub fn new(concurrency: usize, wait: Duration, decoy: Decoy) -> Self {
        Self {
            slots: Arc::new(Semaphore::new(concurrency.max(1))),
            wait,
            decoy,
        }
    }

    /// Waits briefly for a slot, then gives up with 503.
    ///
    /// Bounded on purpose. Waiting indefinitely would trade a saturated CPU for
    /// an unbounded queue, which fails later and less legibly — and every caller
    /// in it is holding a connection.
    async fn slot(&self) -> Result<SemaphorePermit<'_>, AppError> {
        match tokio::time::timeout(self.wait, self.slots.acquire()).await {
            Ok(Ok(permit)) => Ok(permit),
            // The semaphore is never closed, so this arm is unreachable in
            // practice — reported as overload rather than pretending otherwise.
            Ok(Err(_)) | Err(_) => Err(AppError::Overloaded),
        }
    }

    pub async fn hash(&self, password: String) -> Result<String, AppError> {
        let _permit = self.slot().await?;

        tokio::task::spawn_blocking(move || hash(&password))
            .await
            .map_err(|_| AppError::Internal("password hashing failed"))?
    }

    pub async fn verify(&self, password: String, stored: String) -> Result<bool, AppError> {
        let _permit = self.slot().await?;

        tokio::task::spawn_blocking(move || verify(&password, &stored))
            .await
            .map_err(|_| AppError::Internal("password verification failed"))
    }

    /// Burns what a real verification burns, and always fails.
    ///
    /// Takes a slot like any other, which matters: if the decoy path were
    /// exempt, an attacker could bypass the cap entirely by only ever sending
    /// e-mails that do not exist.
    pub async fn spend_decoy(&self, password: String) -> Result<(), AppError> {
        let stored = self.decoy.0.clone();
        self.verify(password, stored).await.map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_then_verify_roundtrips() {
        let stored = hash("correct horse battery staple").unwrap();
        assert!(verify("correct horse battery staple", &stored));
        assert!(!verify("wrong password", &stored));
    }

    #[test]
    fn same_password_hashes_differently() {
        // Distinct salts, so a leaked table cannot be scanned for repeated
        // digests to find which accounts share a password.
        assert_ne!(hash("same").unwrap(), hash("same").unwrap());
    }

    #[test]
    fn decoy_parses_and_never_matches() {
        // The decoy must be a valid PHC string: an invalid one makes `verify`
        // return early on the malformed branch, which costs nothing and is
        // exactly the leak the decoy exists to close.
        let decoy = Decoy::generate().unwrap();
        assert!(PasswordHash::new(&decoy.0).is_ok());
        assert!(!verify("anything at all", &decoy.0));
    }

    #[test]
    fn malformed_stored_hash_does_not_panic() {
        assert!(!verify("password", "not a phc string"));
    }

    /// The cap has to bite, and the wait has to end.
    ///
    /// With one slot already taken and no time to wait for another, a hash must
    /// be refused rather than queued — that refusal is the entire mechanism, and
    /// a semaphore that silently waits forever would pass every other test here.
    #[tokio::test]
    async fn hashing_beyond_the_cap_is_refused_rather_than_queued() {
        let hasher = Hasher::new(1, Duration::from_millis(0), Decoy::generate().unwrap());

        let held = Arc::clone(&hasher.slots).acquire_owned().await.unwrap();

        let refused = hasher.hash("a password long enough".to_owned()).await;
        assert!(matches!(refused, Err(AppError::Overloaded)));

        drop(held);
        assert!(
            hasher
                .hash("a password long enough".to_owned())
                .await
                .is_ok()
        );
    }
}
