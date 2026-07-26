use anyhow::Context;
use sqlx::{PgPool, postgres::PgPoolOptions};

use crate::{cache::Cache, configuration::Settings};

#[derive(Debug, Clone)]
pub struct AppState {
    pub db_pool: PgPool,
    pub cache: Cache,
    pub base_url: String,
}

impl AppState {
    /// `connect`, not `connect_lazy`: opens the first connection right away,
    /// so a database that is down fails the boot, not the first request.
    ///
    /// Redis is checked here too. A cache that is down at boot is almost always
    /// a misconfiguration, and failing loudly beats serving every read from
    /// Postgres without anyone noticing. Once running, cache errors are
    /// swallowed — see [`Cache`].
    pub async fn connect(settings: &Settings) -> anyhow::Result<Self> {
        let db_pool = PgPoolOptions::new()
            .max_connections(settings.database.max_connections)
            .acquire_timeout(settings.database.acquire_timeout())
            .connect_with(settings.database.connect_options())
            .await
            .context("failed to connect to Postgres")?;

        let cache = Cache::connect(&settings.cache)
            .await
            .context("failed to connect to Redis")?;

        Ok(Self {
            db_pool,
            cache,
            base_url: settings.application.base_url.clone(),
        })
    }
}
