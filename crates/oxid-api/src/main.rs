use std::{net::SocketAddr, process::ExitCode, sync::Arc};

use anyhow::Context;
use oxid::{configuration, metrics, routes::router, state::AppState, telemetry};
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

    // Before anything can record: a measurement taken with no recorder
    // installed is dropped without a word, which looks identical to a route
    // nobody calls.
    let metrics_handle = metrics::install()?;

    // No `.context` here: `AppState::connect` already names which dependency
    // failed. Wrapping it would put a second, less precise sentence in front.
    let state = AppState::connect(&settings).await?;

    let listener = TcpListener::bind(addr)
        .await
        .with_context(|| format!("failed to bind {addr}"))?;

    tracing::info!(%addr, "listening");

    // Spawned rather than selected on: if the metrics listener dies the service
    // should keep serving traffic. Losing observability is bad; taking the
    // product down to preserve it would be worse.
    let metrics_pool = state.db_pool.clone();
    let metrics_addr = settings.application.metrics_addr();
    tokio::spawn(async move {
        if let Err(err) = metrics::serve(metrics_handle, metrics_pool, metrics_addr).await {
            tracing::error!(%err, "metrics server stopped");
        }
    });

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
