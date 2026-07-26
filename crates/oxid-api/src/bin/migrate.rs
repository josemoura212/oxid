//! Applies pending migrations, then exits.
//!
//! A separate binary rather than something the server does at boot: with more
//! than one replica, every pod would race on the same schema. As a Job that runs
//! before the rollout, exactly one process migrates. `sqlx` also locks the
//! `_sqlx_migrations` table, so a concurrent run would block rather than corrupt
//! — but relying on that is a worse story than not racing at all.

use std::process::ExitCode;

use anyhow::Context;
use oxid::configuration;
use sqlx::postgres::PgPoolOptions;

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("../../migrations");

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("{err:#}");
            ExitCode::FAILURE
        }
    }
}

#[tokio::main]
async fn run() -> anyhow::Result<()> {
    oxid::telemetry::init();

    let settings = configuration::load()?;

    // One connection is enough, and a small pool keeps a stuck migration from
    // holding several slots on a database sized for the application.
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(settings.database.acquire_timeout())
        .connect_with(settings.database.connect_options())
        .await
        .context("failed to connect to Postgres")?;

    MIGRATOR
        .run(&pool)
        .await
        .context("failed to apply migrations")?;

    tracing::info!("migrations up to date");

    Ok(())
}
