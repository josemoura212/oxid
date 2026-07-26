//! Tests for the instrumentation itself.
//!
//! `allow-unwrap-in-tests` in clippy.toml only covers functions the linter can
//! see are tests; the helpers below are plain functions, so the allow is stated
//! here.
#![allow(clippy::unwrap_used)]

use std::net::{Ipv4Addr, SocketAddr};

use axum::{Router, body::Body, http::Request, routing::get};
use metrics::with_local_recorder;
use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle, PrometheusRecorder};
use oxid::metrics::{install, serve_on, track};
use sqlx::PgPool;
use tokio::net::TcpListener;
use tower::ServiceExt;

/// A recorder that is *not* installed globally, so each test owns its registry.
/// Without that, counters would leak between tests and the assertions would
/// depend on the order they ran in.
fn local_recorder() -> (PrometheusRecorder, PrometheusHandle) {
    let recorder = PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();
    (recorder, handle)
}

/// Drives one request through the middleware against a local registry and hands
/// back what Prometheus would see.
fn render_after_request(
    route: &'static str,
    uri: &'static str,
    status: axum::http::StatusCode,
) -> String {
    let (recorder, handle) = local_recorder();

    with_local_recorder(&recorder, || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        runtime.block_on(async {
            let app = Router::new()
                .route(route, get(move || async move { status }))
                .layer(axum::middleware::from_fn(track));

            let request = Request::builder().uri(uri).body(Body::empty()).unwrap();

            app.oneshot(request).await.unwrap();
        });
    });

    handle.render()
}

/// The property the whole middleware hangs on.
///
/// `/{code}` receives a different path on every request. If the label were the
/// request path, each shortcode would open its own time series and the metrics
/// backend would fall over long before the service — an outage caused entirely
/// by monitoring. The label has to be the route pattern.
#[test]
fn route_label_is_the_pattern_not_the_path() {
    let rendered = render_after_request("/{code}", "/eDrBKMi", axum::http::StatusCode::OK);

    assert!(
        rendered.contains(r#"route="/{code}""#),
        "the matched path should be the label, got:\n{rendered}"
    );
    assert!(
        !rendered.contains("eDrBKMi"),
        "the shortcode must never reach a label, got:\n{rendered}"
    );
}

/// A request that matches no route still has to be counted — traffic hitting
/// nothing is exactly what you want to see during an incident. It just cannot
/// borrow the path as its label, for the same cardinality reason.
#[test]
fn unmatched_requests_are_counted_without_leaking_the_path() {
    let rendered =
        render_after_request("/health", "/v1/does-not-exist", axum::http::StatusCode::OK);

    assert!(
        rendered.contains(r#"route="unmatched""#),
        "unmatched requests should still be recorded, got:\n{rendered}"
    );
    assert!(
        !rendered.contains("does-not-exist"),
        "an unmatched path must not become a label, got:\n{rendered}"
    );
}

/// Distinguishing a flood of 404s from a flood of 500s is the first question
/// asked when something breaks, so the status has to reach the counter.
#[test]
fn status_is_recorded() {
    let rendered = render_after_request(
        "/health",
        "/health",
        axum::http::StatusCode::INTERNAL_SERVER_ERROR,
    );

    assert!(
        rendered.contains(r#"status="500""#),
        "the status label should carry the response code, got:\n{rendered}"
    );
}

/// Latency lands in the histogram, and the buckets are the ones this project
/// chose rather than the library's defaults.
#[test]
fn latency_histogram_uses_the_configured_buckets() {
    let rendered = render_after_request("/health", "/health", axum::http::StatusCode::OK);

    assert!(
        rendered.contains("http_request_duration_seconds_bucket"),
        "the histogram should be exported, got:\n{rendered}"
    );
    assert!(
        rendered.contains(r#"le="0.05""#),
        "the 50 ms bucket is what stage 10's p95 target is read against, got:\n{rendered}"
    );
}

/// Serves the endpoint on a real socket and reads it back — the only way to
/// exercise the pool gauges, which are sampled while rendering rather than
/// before.
///
/// This one installs the recorder globally on purpose: `render` runs inside the
/// spawned server, on another task, and `with_local_recorder` only overrides the
/// thread that calls it. It is also the only test here that installs, because
/// installing twice in one process fails.
#[sqlx::test]
async fn metrics_endpoint_reports_pool_gauges(pool: PgPool) {
    let handle = install().unwrap();

    // Port 0 lets the OS pick, so tests never collide on a fixed port.
    let listener = TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(serve_on(handle, pool, listener));

    let body = get_over_tcp(addr, "/metrics").await;

    assert!(
        body.contains("db_pool_connections"),
        "pool gauges should be sampled when the endpoint is scraped, got:\n{body}"
    );
    assert!(
        body.contains(r#"state="idle""#) && body.contains(r#"state="total""#),
        "both pool states should be present, got:\n{body}"
    );
}

/// A GET spelled out over a raw socket. Adding an HTTP client crate for one
/// request in one test would be a dependency to carry for nothing.
async fn get_over_tcp(addr: SocketAddr, path: &str) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
    let request = format!("GET {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n");

    stream.write_all(request.as_bytes()).await.unwrap();

    let mut response = String::new();
    stream.read_to_string(&mut response).await.unwrap();
    response
}
