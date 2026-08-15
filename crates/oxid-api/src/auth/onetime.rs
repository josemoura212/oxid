//! One-time tokens for the two things e-mail proves: that an address is yours,
//! and that you can get back into an account whose password you lost.
//!
//! Opaque and stored, not signed. A JWT would carry the user id in the token
//! itself and save a Redis round trip — and would be impossible to revoke, which
//! is the one property both of these need. A reset link has to die the moment it
//! is used, and a signed token that has "expired in 2 hours" written inside it
//! stays valid for two hours no matter what the server decides.
//!
//! Single use is enforced by `GETDEL`, not by read-then-delete. Two clicks
//! arriving together would both read the same token and both succeed; `GETDEL`
//! is one round trip, so exactly one of them gets a value back.

use argon2::password_hash::rand_core::{OsRng, RngCore};
use redis::{AsyncCommands, aio::ConnectionManager};

const DEFAULT_NAMESPACE: &str = "oxid";

/// 128 bits, the same reasoning as a session id: a bearer credential with no
/// second factor behind it has to be infeasible to guess.
const TOKEN_BYTES: usize = 16;

/// A day to confirm an address.
///
/// Generous because the cost of expiring is a person who cannot sign in and does
/// not know why, and the token proves possession of a mailbox rather than
/// granting access to an account. Delivery delays, spam folders and "I'll do it
/// tonight" all fit inside a day.
pub const VERIFY_TTL_SECONDS: u64 = 24 * 60 * 60;

/// Two hours to redefine a password.
///
/// Much shorter than the confirmation, and deliberately so: this one **grants
/// access** to an account rather than proving an address. It is also the most
/// likely account-takeover path in the product, which is why it is the one with
/// the tightest window.
pub const RESET_TTL_SECONDS: u64 = 2 * 60 * 60;

/// The e-mail copy quotes this, so it is derived rather than written twice.
pub const RESET_TTL_HOURS: u64 = RESET_TTL_SECONDS / 3600;

/// What a token is for.
///
/// Part of the key, so a confirmation token can never be spent as a reset. They
/// have different lifetimes and very different consequences, and sharing a
/// keyspace would make "prove you own this address" interchangeable with "let me
/// into this account".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Purpose {
    Verify,
    Reset,
}

impl Purpose {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Verify => "v",
            Self::Reset => "r",
        }
    }

    pub const fn ttl_seconds(self) -> u64 {
        match self {
            Self::Verify => VERIFY_TTL_SECONDS,
            Self::Reset => RESET_TTL_SECONDS,
        }
    }
}

#[derive(Debug, Clone)]
pub struct OneTimeTokens {
    /// `None` issues nothing and validates nothing. Same posture as
    /// [`crate::auth::session::SessionStore`]: without a store, the honest
    /// behaviour is that the flow cannot succeed, not that it succeeds blindly.
    conn: Option<ConnectionManager>,
    namespace: String,
}

impl OneTimeTokens {
    pub fn new(conn: ConnectionManager) -> Self {
        Self::with_namespace(conn, DEFAULT_NAMESPACE)
    }

    pub fn with_namespace(conn: ConnectionManager, namespace: &str) -> Self {
        Self {
            conn: Some(conn),
            namespace: namespace.to_owned(),
        }
    }

    pub fn disabled() -> Self {
        Self {
            conn: None,
            namespace: DEFAULT_NAMESPACE.to_owned(),
        }
    }

    /// `{ns}:ot:{purpose}:{token}` — the token, mapped to its user.
    fn token_key(&self, purpose: Purpose, token: &str) -> String {
        format!("{}:ot:{}:{token}", self.namespace, purpose.as_str())
    }

    /// `{ns}:otu:{purpose}:{user_id}` — the newest token a user holds for this
    /// purpose, so issuing another can retire it.
    fn user_key(&self, purpose: Purpose, user_id: i64) -> String {
        format!("{}:otu:{}:{user_id}", self.namespace, purpose.as_str())
    }

    /// Issues a token, retiring whatever the user held for the same purpose.
    ///
    /// **Retiring the previous one is a security property, not tidiness.** Asking
    /// for three reset links should leave one that works, not three. Otherwise
    /// every request widens the window, and a link forwarded or logged an hour
    /// ago stays live alongside the one just requested.
    pub async fn issue(&self, purpose: Purpose, user_id: i64) -> Result<String, TokenError> {
        let mut conn = self.conn.clone().ok_or(TokenError::Unavailable)?;

        let mut bytes = [0u8; TOKEN_BYTES];
        OsRng.fill_bytes(&mut bytes);
        let token = hex::encode(bytes);

        let _: () = conn
            .set_ex(
                self.token_key(purpose, &token),
                user_id,
                purpose.ttl_seconds(),
            )
            .await?;

        // **`SET ... GET` in one command, not `GET` then `SET`.** Read and write
        // as two round trips, two concurrent issues both read the same pointer,
        // both overwrite it, and both delete the same stale token — leaving two
        // *new* tokens live, one of which no future issue will ever retire
        // because the pointer only remembers the other. A double-clicked "send it
        // again" was enough. Redis serialises the single command, so exactly one
        // caller gets the outgoing value back.
        let previous: Option<String> = redis::cmd("SET")
            .arg(self.user_key(purpose, user_id))
            .arg(&token)
            .arg("EX")
            .arg(purpose.ttl_seconds())
            .arg("GET")
            .query_async(&mut conn)
            .await?;

        if let Some(stale) = previous {
            // Best effort: the new token is already live and usable, and a stale
            // one that survives this expires on its own. Failing the whole issue
            // here would turn a cleanup problem into "you cannot reset at all".
            //
            // Logged rather than dropped, because the doc above calls retiring the
            // previous token a security property — and a security property that
            // fails without a trace is a hope. If this warns, someone is holding
            // two live reset links for the rest of the TTL.
            if let Err(err) = conn.del::<_, ()>(self.token_key(purpose, &stale)).await {
                tracing::warn!(
                    %err,
                    purpose = purpose.as_str(),
                    "the previous one-time token survived and is still valid"
                );
            }
        }

        Ok(token)
    }

    /// Reads a token without spending it.
    ///
    /// This is what the reset screen calls when it opens: the link has to be
    /// checked before showing a password form, or someone types a new password
    /// into a form that was never going to work. Consuming here instead would
    /// mean the token dies on page load — and a refresh, a second monitor, or a
    /// mail client that pre-fetches links would kill it before anyone typed
    /// anything.
    ///
    /// **`Ok(None)` and `Err` are different answers and the caller must keep them
    /// apart.** `Ok(None)` is "this link is not valid"; `Err` is "we could not
    /// find out". Collapsing them — which this did, by returning a bare `Option`
    /// — told someone holding a perfectly good link that it had expired, and sent
    /// them to ask for another that the same broken Redis could not issue either.
    pub async fn peek(&self, purpose: Purpose, token: &str) -> Result<Option<i64>, TokenError> {
        let mut conn = self.conn.clone().ok_or(TokenError::Unavailable)?;

        Ok(conn
            .get::<_, Option<i64>>(self.token_key(purpose, token))
            .await?)
    }

    /// Spends a token. The second caller gets `Ok(None)`.
    ///
    /// `GETDEL` rather than `GET` then `DEL`: two clicks arriving together would
    /// both read the same value and both proceed, which is exactly the race a
    /// single-use token exists to prevent.
    pub async fn consume(&self, purpose: Purpose, token: &str) -> Result<Option<i64>, TokenError> {
        let mut conn = self.conn.clone().ok_or(TokenError::Unavailable)?;

        let Some(user_id) = conn
            .get_del::<_, Option<i64>>(self.token_key(purpose, token))
            .await?
        else {
            return Ok(None);
        };

        // The pointer is now dangling. Left behind, the next issue would try to
        // delete a token key that is already gone — harmless, but it would also
        // mean a stale pointer outliving its token for the whole TTL.
        if let Err(err) = conn.del::<_, ()>(self.user_key(purpose, user_id)).await {
            tracing::warn!(
                %err,
                purpose = purpose.as_str(),
                "spent token left a dangling pointer"
            );
        }

        Ok(Some(user_id))
    }
}

#[derive(Debug, thiserror::Error)]
pub enum TokenError {
    #[error("one-time token storage is not available")]
    Unavailable,

    #[error(transparent)]
    Redis(#[from] redis::RedisError),
}
