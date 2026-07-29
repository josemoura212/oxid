use anyhow::Context;
use sqlx::{PgPool, postgres::PgPoolOptions};

use crate::{
    analytics::ClickSink,
    auth::{
        password::{Decoy, Hasher},
        session::SessionStore,
    },
    cache::{self, Cache},
    configuration::Settings,
};

#[derive(Debug, Clone)]
pub struct AppState {
    pub db_pool: PgPool,
    pub cache: Cache,
    pub sessions: SessionStore,
    /// Where click events go. `Disabled` unless the config selects a backend, and
    /// nothing writes to it yet — the hot-path emit and the batching worker are
    /// later slices. Held here so a misconfigured backend fails the boot.
    pub clicks: ClickSink,
    pub base_url: String,
    /// Runs Argon2 off the runtime and under a concurrency cap. Holds the decoy,
    /// so the unknown-e-mail path costs the same as the known one — and takes a
    /// slot like any other, or it would be a way around the cap.
    pub hasher: Hasher,
    /// Derived from `base_url`, not configured separately: a `Secure` cookie is
    /// dropped by the browser over plain HTTP, so hardcoding it would break
    /// local development in a way that looks like a bug in the login code.
    pub secure_cookies: bool,
    pub session_ttl_seconds: i64,
}

impl AppState {
    /// `connect`, not `connect_lazy`: opens the first connection right away,
    /// so a database that is down fails the boot, not the first request.
    ///
    /// Redis is checked here too. A cache that is down at boot is almost always
    /// a misconfiguration, and failing loudly beats serving every read from
    /// Postgres without anyone noticing. Once running, cache errors are
    /// swallowed — see [`Cache`]. Sessions do not get that treatment, because a
    /// session that fails to store is a login that only appears to work.
    pub async fn connect(settings: &Settings) -> anyhow::Result<Self> {
        let db_pool = PgPoolOptions::new()
            .max_connections(settings.database.max_connections)
            .acquire_timeout(settings.database.acquire_timeout())
            .connect_with(settings.database.connect_options())
            .await
            .context("failed to connect to Postgres")?;

        let conn = cache::connect(&settings.cache)
            .await
            .context("failed to connect to Redis")?;

        let cache = Cache::new(conn.clone(), settings.cache.negative_ttl_seconds);
        let sessions = SessionStore::new(conn, settings.session.ttl_seconds);

        let decoy = Decoy::generate().context("failed to build the login decoy hash")?;
        let hasher = Hasher::new(
            settings.rate_limit.hash_concurrency,
            settings.rate_limit.hash_wait(),
            decoy,
        );

        // Off by default, so a plain boot never needs ClickHouse running. When a
        // backend is selected, this connects and creates the table — failing the
        // boot if it cannot, rather than dropping clicks silently later.
        let clicks = ClickSink::connect(&settings.analytics)
            .await
            .context("failed to connect the analytics backend")?;

        let base_url = settings.application.base_url.clone();
        let secure_cookies = base_url.starts_with("https://");

        let session_ttl_seconds = i64::try_from(settings.session.ttl_seconds)
            .context("session ttl does not fit in i64")?;

        Ok(Self {
            db_pool,
            cache,
            sessions,
            clicks,
            base_url,
            hasher,
            secure_cookies,
            session_ttl_seconds,
        })
    }
}
