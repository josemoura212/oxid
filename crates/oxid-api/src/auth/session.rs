//! Session storage.
//!
//! Sessions live in Redis rather than in a signed cookie, for one reason:
//! logout has to actually revoke. A self-contained token stays valid until it
//! expires no matter what the server thinks, so "sign out everywhere" would be
//! a lie. The cost is a Redis round trip on authenticated routes — which the
//! redirect path, the one that matters for load, never pays.

use argon2::password_hash::rand_core::{OsRng, RngCore};
use redis::{AsyncCommands, aio::ConnectionManager};

/// Default key namespace. Tests pass their own so a shared Redis stays isolated
/// per run — session ids are random and never collide, but the per-user index
/// below is keyed by a small integer that repeats across test databases.
const DEFAULT_NAMESPACE: &str = "oxid";

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
    namespace: String,
}

impl SessionStore {
    pub fn new(conn: ConnectionManager, ttl_seconds: u64) -> Self {
        Self::with_namespace(conn, ttl_seconds, DEFAULT_NAMESPACE)
    }

    pub fn with_namespace(conn: ConnectionManager, ttl_seconds: u64, namespace: &str) -> Self {
        Self {
            conn: Some(conn),
            ttl_seconds,
            namespace: namespace.to_owned(),
        }
    }

    /// A store with nothing behind it. Used by tests that exercise the anonymous
    /// surface and have no Redis.
    pub fn disabled() -> Self {
        Self {
            conn: None,
            ttl_seconds: 0,
            namespace: DEFAULT_NAMESPACE.to_owned(),
        }
    }

    /// `{ns}:s:{id}` — a session id mapped to its user.
    fn session_key(&self, id: &str) -> String {
        format!("{}:s:{id}", self.namespace)
    }

    /// `{ns}:u:{user_id}` — the set of a user's live session ids, so every one
    /// can be revoked at once. Without it, "sign out everywhere" is impossible:
    /// a compromised account has no way to know which sessions exist.
    fn user_key(&self, user_id: i64) -> String {
        format!("{}:u:{user_id}", self.namespace)
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
        let _: () = conn
            .set_ex(self.session_key(&id), user_id, self.ttl_seconds)
            .await?;

        // Add to the user's index, and give the index the same lifetime as the
        // longest-lived session — refreshed on every new one, so an active user
        // never loses their index. A stale id lingering in the set is harmless:
        // revoke deletes by key, which no-ops if the session already expired.
        let _: () = conn.sadd(self.user_key(user_id), &id).await?;
        let _: () = conn.expire(self.user_key(user_id), self.ttl_i64()).await?;

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

        match conn.get::<_, Option<i64>>(self.session_key(id)).await {
            Ok(user_id) => user_id,
            Err(err) => {
                tracing::warn!(%err, "session lookup failed");
                None
            }
        }
    }

    /// Revokes one session. Idempotent — deleting an id that is already gone is
    /// the same outcome the caller wanted.
    ///
    /// Reads the user first so the id can also leave the per-user index. If that
    /// read fails, the session key is still deleted — the credential dies either
    /// way; only the index entry lingers, and it is harmless.
    pub async fn revoke(&self, id: &str) {
        let Some(mut conn) = self.conn.clone() else {
            return;
        };

        if let Ok(Some(user_id)) = conn.get::<_, Option<i64>>(self.session_key(id)).await {
            let _: Result<(), _> = conn.srem(self.user_key(user_id), id).await;
        }

        if let Err(err) = conn.del::<_, ()>(self.session_key(id)).await {
            tracing::warn!(%err, "session revoke failed");
        }
    }

    /// Revokes every session a user has — the "sign out everywhere" an account
    /// reaches for after a suspected compromise.
    ///
    /// Best-effort and idempotent: any id already expired simply no-ops on
    /// delete. The index set is dropped last, so a partial failure leaves the
    /// still-live sessions listed for a retry rather than orphaned.
    pub async fn revoke_all(&self, user_id: i64) -> Result<(), SessionError> {
        let mut conn = self.conn.clone().ok_or(SessionError::Unavailable)?;

        let ids: Vec<String> = conn.smembers(self.user_key(user_id)).await?;
        if !ids.is_empty() {
            let keys: Vec<String> = ids.iter().map(|id| self.session_key(id)).collect();
            let _: () = conn.del(keys).await?;
        }
        let _: () = conn.del(self.user_key(user_id)).await?;

        Ok(())
    }

    /// The TTL as `i64` for the `EXPIRE` command, saturating rather than wrapping
    /// on the (impossible for a 7-day TTL) overflow.
    fn ttl_i64(&self) -> i64 {
        i64::try_from(self.ttl_seconds).unwrap_or(i64::MAX)
    }
}

/// Why a session operation could not complete.
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
