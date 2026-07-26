//! Prometheus metrics, served on their own port.
//!
//! **Not** a route on the public router. Traefik forwards everything that is not
//! the front end to this service, so a `/metrics` path would be readable from
//! the internet — handing out request volumes, latency distributions and cache
//! behaviour to anyone who asks. A separate listener is reachable from inside
//! the cluster and nowhere else.

use std::{future::IntoFuture, net::SocketAddr, time::Instant};

use anyhow::Context;
use axum::{
    Router,
    extract::{MatchedPath, Request, State},
    middleware::Next,
    response::Response,
    routing::get,
};
use metrics_exporter_prometheus::{Matcher, PrometheusBuilder, PrometheusHandle};
use sqlx::PgPool;
use tokio::net::TcpListener;

/// Latency buckets in seconds, and they are the whole point of the histogram.
///
/// The default set spreads evenly over a range this service will never use.
/// Stage 10 targets p95 under 50 ms on reads, and a cache hit answers in single
/// milliseconds, so the resolution has to be where the answers actually land —
/// dense below 100 ms, then coarse enough to still show a stall.
const LATENCY_BUCKETS: &[f64] = &[
    0.0005, 0.001, 0.0025, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0,
];

/// Installs the recorder process-wide and returns the handle that renders it.
///
/// Must run before anything records a measurement: metrics emitted while no
/// recorder is installed are dropped silently, which looks exactly like a route
/// that is never called.
pub fn install() -> anyhow::Result<PrometheusHandle> {
    PrometheusBuilder::new()
        .set_buckets_for_metric(
            Matcher::Full("http_request_duration_seconds".to_owned()),
            LATENCY_BUCKETS,
        )
        .context("invalid latency buckets")?
        .install_recorder()
        .context("failed to install the Prometheus recorder")
}

/// Records duration and outcome of every request.
///
/// The route label comes from [`MatchedPath`] — the pattern, not the URL. On
/// `/{code}` those differ by design: the real path is a different string on
/// every single request, so labelling with it would create one time series per
/// shortcode and take the Prometheus server down long before the service. This
/// is the one line in the file that has to be right.
pub async fn track(request: Request, next: Next) -> Response {
    let route = request
        .extensions()
        .get::<MatchedPath>()
        .map_or_else(|| "unmatched".to_owned(), |path| path.as_str().to_owned());
    let method = request.method().to_string();

    let started = Instant::now();
    let response = next.run(request).await;
    let elapsed = started.elapsed().as_secs_f64();

    let status = response.status().as_u16().to_string();

    metrics::histogram!(
        "http_request_duration_seconds",
        "route" => route.clone(),
        "method" => method.clone(),
    )
    .record(elapsed);

    metrics::counter!(
        "http_requests_total",
        "route" => route,
        "method" => method,
        "status" => status,
    )
    .increment(1);

    response
}

/// Serves `GET /metrics` until the process ends.
///
/// Bound separately from the API and never behind the proxy.
pub async fn serve(handle: PrometheusHandle, pool: PgPool, addr: SocketAddr) -> anyhow::Result<()> {
    let app = Router::new()
        .route("/metrics", get(render))
        .with_state(MetricsState { handle, pool });

    let listener = TcpListener::bind(addr)
        .await
        .with_context(|| format!("failed to bind the metrics listener on {addr}"))?;

    tracing::info!(%addr, "metrics listening");

    axum::serve(listener, app)
        .into_future()
        .await
        .context("metrics server exited with error")
}

#[derive(Clone)]
struct MetricsState {
    handle: PrometheusHandle,
    pool: PgPool,
}

/// Pool gauges are sampled here rather than by a background task on a timer.
/// Read at scrape time they are never staler than the scrape itself, and there
/// is no second clock to reason about.
///
/// `size` counts connections the pool holds, idle or busy; `num_idle` counts the
/// ones sitting free. Busy is the difference, and it is the number that matters:
/// when it stays at `max_connections`, requests are queueing on the pool rather
/// than on the database.
async fn render(State(state): State<MetricsState>) -> String {
    let total = state.pool.size();
    // `num_idle` is a usize; going through u32 keeps the conversion to f64
    // lossless instead of an `as` cast that silently rounds.
    let idle = u32::try_from(state.pool.num_idle()).unwrap_or(u32::MAX);

    metrics::gauge!("db_pool_connections", "state" => "total").set(f64::from(total));
    metrics::gauge!("db_pool_connections", "state" => "idle").set(f64::from(idle));

    state.handle.render()
}
