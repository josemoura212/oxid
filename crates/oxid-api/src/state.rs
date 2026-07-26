use sqlx::{PgPool, postgres::PgPoolOptions};

use crate::configuration::Settings;

#[derive(Debug, Clone)]
pub struct AppState {
    pub db_pool: PgPool,
    pub base_url: String,
}

impl AppState {
    /// `connect`, not `connect_lazy`: opens the first connection right away,
    /// so a database that is down fails the boot, not the first request.
    pub async fn connect(settings: &Settings) -> Result<Self, sqlx::Error> {
        let db_pool = PgPoolOptions::new()
            .max_connections(settings.database.max_connections)
            .acquire_timeout(settings.database.acquire_timeout())
            .connect_with(settings.database.connect_options())
            .await?;

        Ok(Self {
            db_pool,
            base_url: settings.application.base_url.clone(),
        })
    }
}
