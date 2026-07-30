use redis::{AsyncCommands, Client, ExistenceCheck, SetExpiry, SetOptions, aio::ConnectionManager};

use crate::configuration::CacheSettings;

/// Namespace prefix, so a future key type cannot collide with a shortcode.
///
/// Bumped to `u2:` when the stored value gained an ownership flag (stage 12): the
/// old `u:` entries hold a bare URL and would be misread under the new format, so
/// a new namespace abandons them to LRU and repopulates. The cache is disposable,
/// so this is a format bump, not a migration.
const KEY_PREFIX: &str = "u2:";

/// Marks "no URL is registered under this code". Cannot be mistaken for a real
/// value because a stored positive value always begins with an ownership flag.
const MISSING: &str = "\u{0}";

/// First character of a stored positive value: this code has an owner.
const OWNED_FLAG: char = '1';
/// First character of a stored positive value: this code is anonymous.
const ANON_FLAG: char = '0';

/// What the cache knows about a code. `None` from [`Cache::get`] means the cache
/// has no opinion — go ask Postgres.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cached {
    Url {
        long_url: String,
        /// Whether the code has an owner. Immutable — a per-owner code is owned
        /// or anonymous at creation and never changes — so it lives in the cache
        /// value with no invalidation. It decides 301 vs 302 and whether the
        /// redirect records a click.
        owned: bool,
    },
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

/// Opens the one connection everything Redis-backed shares.
///
/// `ConnectionManager` multiplexes and is cheap to clone, so the cache and the
/// session store take a handle to the same connection rather than opening two.
/// Separate connections would double the socket count for no isolation — a
/// failure takes both down either way.
pub async fn connect(settings: &CacheSettings) -> Result<ConnectionManager, redis::RedisError> {
    let client = Client::open(settings.url())?;
    let config = redis::aio::ConnectionManagerConfig::new()
        .set_connection_timeout(Some(settings.connect_timeout()));
    ConnectionManager::new_with_config(client, config).await
}

impl Cache {
    pub const fn new(conn: ConnectionManager, negative_ttl_seconds: u64) -> Self {
        Self {
            conn: Some(conn),
            negative_ttl_seconds,
        }
    }

    /// A cache that stores nothing and knows nothing.
    pub const fn disabled() -> Self {
        Self {
            conn: None,
            negative_ttl_seconds: 0,
        }
    }

    /// Reads survive Redis being down; sessions cannot. Handing the connection
    /// out lets the session store share it while keeping the two concerns in
    /// separate types — they disagree about what a failure means.
    pub fn connection(&self) -> Option<ConnectionManager> {
        self.conn.clone()
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

        // The counter carries the same three outcomes as the log line, as one
        // label. A log answers "what happened to this code"; the counter answers
        // "what is the hit rate right now", which is the number that decides
        // whether the cache is doing its job — the original study only found out
        // it was at 50% after the fact.
        match value {
            None => {
                tracing::debug!(code, cache = "miss", "cache lookup");
                metrics::counter!("cache_lookups_total", "outcome" => "miss").increment(1);
                None
            }
            Some(value) if value == MISSING => {
                tracing::debug!(code, cache = "hit_negative", "cache lookup");
                metrics::counter!("cache_lookups_total", "outcome" => "hit_negative").increment(1);
                Some(Cached::Missing)
            }
            Some(value) => {
                // The stored value is a one-character ownership flag followed by
                // the URL. `strip_prefix` avoids indexing (denied) and, if the
                // value somehow has neither flag — a stray old-format entry — it
                // reads as a miss rather than serving a corrupt URL.
                let cached = value
                    .strip_prefix(OWNED_FLAG)
                    .map(|url| (url, true))
                    .or_else(|| value.strip_prefix(ANON_FLAG).map(|url| (url, false)));

                let Some((long_url, owned)) = cached else {
                    tracing::warn!(code, "cache value missing its ownership flag");
                    metrics::counter!("cache_lookups_total", "outcome" => "miss").increment(1);
                    return None;
                };

                tracing::debug!(code, cache = "hit", "cache lookup");
                metrics::counter!("cache_lookups_total", "outcome" => "hit").increment(1);
                Some(Cached::Url {
                    long_url: long_url.to_owned(),
                    owned,
                })
            }
        }
    }

    /// No TTL: the mapping is immutable, so the entry can never go stale.
    /// Eviction is Redis's job, via `maxmemory` + `allkeys-lru`.
    ///
    /// The value is the ownership flag then the URL, so a read learns both in one
    /// round trip and the redirect never touches Postgres to decide 301 vs 302.
    pub async fn set_url(&self, code: &str, long_url: &str, owned: bool) {
        let Some(mut conn) = self.conn.clone() else {
            return;
        };

        let flag = if owned { OWNED_FLAG } else { ANON_FLAG };
        let value = format!("{flag}{long_url}");

        if let Err(err) = conn.set::<_, _, ()>(key(code), value).await {
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
