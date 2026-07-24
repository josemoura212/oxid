use std::{net::SocketAddr, process::ExitCode, sync::Arc};

use anyhow::Context;
use oxid::{routes::router, state::AppState, telemetry};
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

    let state = Arc::new(AppState);
    let addr = SocketAddr::from(([127, 0, 0, 1], 3000));
    let listener = TcpListener::bind(addr)
        .await
        .with_context(|| format!("failed to bind {addr}"))?;

    tracing::info!(%addr, "listening");

    axum::serve(listener, router(state))
        .await
        .context("axum server exited with error")?;

    Ok(())
}
