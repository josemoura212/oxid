use redis::{AsyncCommands, Client, ExistenceCheck, SetExpiry, SetOptions, aio::ConnectionManager};

use crate::configuration::CacheSettings;

/// Namespace prefix, so a future key type cannot collide with a shortcode.
const KEY_PREFIX: &str = "u:";

/// Marks "no URL is registered under this code". Cannot be mistaken for a real
/// value because every stored URL is `http`/`https`, checked at write time.
const MISSING: &str = "\u{0}";

/// What the cache knows about a code. `None` from [`Cache::get`] means the cache
/// has no opinion — go ask Postgres.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cached {
    Url(String),
    Missing,
}

#[derive(Clone)]
pub struct Cache {
    /// `None` disables the cache entirely — every call becomes a no-op and reads
    /// fall through to Postgres. Used by tests that do not care about caching,
    /// and available as a switch to measure the cache's actual contribution in
    /// the load-testing stages.
    conn: Option<ConnectionManager>,
    negative_ttl_seconds: u64,
}

// `ConnectionManager` has no Debug, and AppState derives it.
impl std::fmt::Debug for Cache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Cache")
            .field("negative_ttl_seconds", &self.negative_ttl_seconds)
            .finish_non_exhaustive()
    }
}

impl Cache {
    pub async fn connect(settings: &CacheSettings) -> Result<Self, redis::RedisError> {
        let client = Client::open(settings.url())?;
        let config = redis::aio::ConnectionManagerConfig::new()
            .set_connection_timeout(Some(settings.connect_timeout()));
        let conn = ConnectionManager::new_with_config(client, config).await?;

        Ok(Self {
            conn: Some(conn),
            negative_ttl_seconds: settings.negative_ttl_seconds,
        })
    }

    /// A cache that stores nothing and knows nothing.
    pub const fn disabled() -> Self {
        Self {
            conn: None,
            negative_ttl_seconds: 0,
        }
    }

    /// Redis being down must not take reads down with it. Every failure here is
    /// logged and swallowed into `None`, which degrades to a Postgres lookup —
    /// slower, still correct. That is why nothing in this module hands a
    /// `Result` to the caller: the cache is a performance dependency, not a
    /// correctness one.
    pub async fn get(&self, code: &str) -> Option<Cached> {
        let mut conn = self.conn.clone()?;
        let value: Option<String> = match conn.get(key(code)).await {
            Ok(value) => value,
            Err(err) => {
                tracing::warn!(%err, code, "cache read failed");
                return None;
            }
        };

        match value {
            None => {
                tracing::debug!(code, cache = "miss", "cache lookup");
                None
            }
            Some(value) if value == MISSING => {
                tracing::debug!(code, cache = "hit_negative", "cache lookup");
                Some(Cached::Missing)
            }
            Some(url) => {
                tracing::debug!(code, cache = "hit", "cache lookup");
                Some(Cached::Url(url))
            }
        }
    }

    /// No TTL: the mapping is immutable, so the entry can never go stale.
    /// Eviction is Redis's job, via `maxmemory` + `allkeys-lru`.
    pub async fn set_url(&self, code: &str, long_url: &str) {
        let Some(mut conn) = self.conn.clone() else {
            return;
        };

        if let Err(err) = conn.set::<_, _, ()>(key(code), long_url).await {
            tracing::warn!(%err, code, "cache write failed");
        }
    }

    /// `SET NX`, not `SET`. Without the condition this race loses data:
    ///
    /// ```text
    /// T1  GET /aaaaaaa    → miss in cache, miss in database
    /// T2                    POST creates a URL that happens to yield "aaaaaaa"
    /// T2                    writes aaaaaaa → https://...
    /// T1  writes "missing" → overwrites the good value
    /// ```
    ///
    /// A negative entry has no invalidation path, so that code would answer 404
    /// until the TTL expired. `NX` makes the write conditional; the short TTL is
    /// the second line of defence.
    pub async fn set_missing(&self, code: &str) {
        let Some(mut conn) = self.conn.clone() else {
            return;
        };

        let options = SetOptions::default()
            .conditional_set(ExistenceCheck::NX)
            .with_expiration(SetExpiry::EX(self.negative_ttl_seconds));

        if let Err(err) = conn
            .set_options::<_, _, ()>(key(code), MISSING, options)
            .await
        {
            tracing::warn!(%err, code, "negative cache write failed");
        }
    }
}

fn key(code: &str) -> String {
    format!("{KEY_PREFIX}{code}")
}
