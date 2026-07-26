use std::{net::SocketAddr, process::ExitCode, sync::Arc};

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

    // No `.context` here: `AppState::connect` already names which dependency
    // failed. Wrapping it would put a second, less precise sentence in front.
    let state = AppState::connect(&settings).await?;

    let listener = TcpListener::bind(addr)
        .await
        .with_context(|| format!("failed to bind {addr}"))?;

    tracing::info!(%addr, "listening");

    let app = router(Arc::new(state), settings.rate_limit)?;

    // `into_make_service_with_connect_info` is what puts the peer address in the
    // request extensions. Without it the rate limiter has no key to fall back to
    // when X-Forwarded-For is absent — a direct hit on the service (bypassing
    // the proxy) would answer 500 instead of being limited.
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await
    .context("axum server exited with error")?;

    Ok(())
}
