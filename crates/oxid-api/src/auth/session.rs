//! Session storage.
//!
//! Sessions live in Redis rather than in a signed cookie, for one reason:
//! logout has to actually revoke. A self-contained token stays valid until it
//! expires no matter what the server thinks, so "sign out everywhere" would be
//! a lie. The cost is a Redis round trip on authenticated routes — which the
//! redirect path, the one that matters for load, never pays.

use argon2::password_hash::rand_core::{OsRng, RngCore};
use redis::{AsyncCommands, aio::ConnectionManager};

/// Namespace prefix, so a session id cannot collide with a cached shortcode.
const KEY_PREFIX: &str = "s:";

/// 128 bits. The id is a bearer credential with no other factor behind it, so
/// it has to be infeasible to guess rather than merely inconvenient.
const ID_BYTES: usize = 16;

#[derive(Debug, Clone)]
pub struct SessionStore {
    /// `None` stores nothing and authenticates nobody. That is the honest
    /// behaviour without a backing store — not "everyone is anonymous, carry
    /// on", but "logging in cannot succeed", which is what [`Self::create`]
    /// returns.
    conn: Option<ConnectionManager>,
    ttl_seconds: u64,
}

impl SessionStore {
    pub const fn new(conn: ConnectionManager, ttl_seconds: u64) -> Self {
        Self {
            conn: Some(conn),
            ttl_seconds,
        }
    }

    /// A store with nothing behind it. Used by tests that exercise the anonymous
    /// surface and have no Redis.
    pub const fn disabled() -> Self {
        Self {
            conn: None,
            ttl_seconds: 0,
        }
    }

    /// Creates a session and returns its id.
    ///
    /// Unlike the URL cache, failure here is **not** swallowed. A cache miss
    /// degrades to a database lookup; a session that silently fails to store
    /// hands the caller a cookie that authenticates nothing, and the symptom is
    /// a login that appears to work and then does not.
    pub async fn create(&self, user_id: i64) -> Result<String, SessionError> {
        let mut conn = self.conn.clone().ok_or(SessionError::Unavailable)?;

        let mut bytes = [0u8; ID_BYTES];
        OsRng.fill_bytes(&mut bytes);
        let id = hex::encode(bytes);

        // The TTL is the session lifetime. Sliding it on every request would
        // mean a write on every authenticated call, and an idle tab that never
        // expires.
        let _: () = conn.set_ex(key(&id), user_id, self.ttl_seconds).await?;

        Ok(id)
    }

    /// Resolves a session id to its user.
    ///
    /// A Redis failure reads as "no session" rather than an error. Sessions are
    /// an availability concern here: answering 500 on the whole authenticated
    /// surface because the cache blinked is worse than treating the caller as
    /// anonymous, and the anonymous path works.
    pub async fn user_id(&self, id: &str) -> Option<i64> {
        let mut conn = self.conn.clone()?;

        match conn.get::<_, Option<i64>>(key(id)).await {
            Ok(user_id) => user_id,
            Err(err) => {
                tracing::warn!(%err, "session lookup failed");
                None
            }
        }
    }

    /// Revokes a session. Idempotent — deleting an id that is already gone is
    /// the same outcome the caller wanted.
    pub async fn revoke(&self, id: &str) {
        let Some(mut conn) = self.conn.clone() else {
            return;
        };

        if let Err(err) = conn.del::<_, ()>(key(id)).await {
            tracing::warn!(%err, "session revoke failed");
        }
    }
}

/// Why a session could not be created.
///
/// Distinguished from a plain `RedisError` so the handler can tell "the store is
/// not configured" from "the store refused", and neither is reported to the
/// client as anything other than an internal error.
#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("session storage is not available")]
    Unavailable,

    #[error(transparent)]
    Redis(#[from] redis::RedisError),
}

fn key(id: &str) -> String {
    format!("{KEY_PREFIX}{id}")
}
