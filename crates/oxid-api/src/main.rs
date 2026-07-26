use std::{process::ExitCode, sync::Arc};

use anyhow::Context;
use oxid::{configuration, routes::router, state::AppState, telemetry};
use tokio::net::TcpListener;

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
    telemetry::init();

    let settings = configuration::load()?;
    let addr = settings.application.addr();

    let state = AppState::connect(&settings)
        .await
        .context("falha ao conectar no Postgres")?;

    let listener = TcpListener::bind(addr)
        .await
        .with_context(|| format!("failed to bind {addr}"))?;

    tracing::info!(%addr, "listening");

    axum::serve(listener, router(Arc::new(state)))
        .await
        .context("axum server exited with error")?;

    Ok(())
}
