use std::sync::Arc;

use anyhow::Context;
use axum::{
    Router,
    routing::{get, post},
};
use tower_governor::{
    GovernorLayer, governor::GovernorConfigBuilder, key_extractor::SmartIpKeyExtractor,
};
use tower_http::trace::TraceLayer;

use crate::{configuration::RateLimitSettings, state::AppState};

/// Fails only on invalid rate-limit settings, which is a bootstrap problem, not
/// a request one — hence `anyhow` rather than `AppError`.
pub fn router(state: Arc<AppState>, rate_limit: RateLimitSettings) -> anyhow::Result<Router> {
    // The default extractor is `PeerIpKeyExtractor`, and it is wrong here twice
    // over: it needs `ConnectInfo` (absent unless the server is built with
    // `into_make_service_with_connect_info`, and absent in tests), and behind a
    // reverse proxy every client shares the proxy's IP — one global limit
    // instead of one per client.
    //
    // `SmartIpKeyExtractor` reads X-Forwarded-For / X-Real-IP and falls back to
    // the socket address. Those headers are client-supplied, so this is only
    // sound because Traefik overwrites them before the request arrives. Exposing
    // this service directly would let anyone bypass the limit by setting the
    // header themselves.
    let config = GovernorConfigBuilder::default()
        .per_second(rate_limit.shorten_per_second)
        .burst_size(rate_limit.shorten_burst)
        .key_extractor(SmartIpKeyExtractor)
        .use_headers()
        .finish()
        .context("invalid rate limit configuration")?;

    Ok(Router::new()
        .route("/health", get(health::health))
        .route(
            "/v1/shorten",
            post(shorten::shorten).layer(GovernorLayer::new(Arc::new(config))),
        )
        // The redirect sits at the root: `/{code}` is the product. Prefixing it
        // with `/v1/urls/` would spend 9 characters on a URL whose whole point is
        // being short. Literal routes win over the parameter, so `/health` and
        // `/v1/shorten` still match themselves.
        .route("/{code}", get(resolve::resolve))
        .layer(TraceLayer::new_for_http())
        // Outside the TraceLayer, so the measured span covers the same work the
        // trace describes, and after the routes, so `MatchedPath` is already in
        // the extensions — a layer added before them would only ever see
        // "unmatched".
        .layer(axum::middleware::from_fn(crate::metrics::track))
        .with_state(state))
}

mod health;
mod resolve;
mod shorten;
