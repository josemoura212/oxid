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
    let shorten_limit = GovernorConfigBuilder::default()
        .per_second(rate_limit.shorten_per_second)
        .burst_size(rate_limit.shorten_burst)
        .key_extractor(SmartIpKeyExtractor)
        .use_headers()
        .finish()
        .context("invalid shorten rate limit configuration")?;

    // A second, tighter limiter for the routes where one request is expensive.
    // Separate from the write limit on purpose: sharing would let a burst of
    // ordinary shortening exhaust the budget protecting the costly paths.
    //
    // Covers three routes, for two different reasons. Login and signup each
    // spend an Argon2 verification — signup even when the e-mail turns out to be
    // taken, and login even when it does not exist. Import spends up to a hundred
    // round trips to Postgres in one call, holding a connection from a pool of
    // eight while it does.
    let expensive_limit = GovernorConfigBuilder::default()
        .per_second(rate_limit.login_per_second)
        .burst_size(rate_limit.login_burst)
        .key_extractor(SmartIpKeyExtractor)
        .use_headers()
        .finish()
        .context("invalid rate limit configuration for expensive routes")?;

    let expensive_limit = GovernorLayer::new(Arc::new(expensive_limit));

    Ok(Router::new()
        .route("/health", get(health::health))
        .route(
            "/v1/shorten",
            post(shorten::shorten).layer(GovernorLayer::new(Arc::new(shorten_limit))),
        )
        .route(
            "/v1/signup",
            post(accounts::signup).layer(expensive_limit.clone()),
        )
        .route(
            "/v1/login",
            post(accounts::login).layer(expensive_limit.clone()),
        )
        .route("/v1/logout", post(accounts::logout))
        // "Sign out everywhere" — revokes every session the caller has, for after
        // a suspected compromise. Needs a valid session: you can only revoke your
        // own.
        .route("/v1/logout-all", post(accounts::logout_all))
        .route("/v1/me", get(accounts::me))
        .route("/v1/session", get(accounts::session_state))
        .route("/v1/urls", get(urls::list))
        // Batched deliberately: one request instead of one write per saved link,
        // so an import is not throttled into a half-finished state.
        //
        // Which is exactly why it needs its own ceiling. Requiring a session is
        // not a limit — an account is free, and one call can be a hundred writes.
        .route("/v1/urls/import", post(urls::import).layer(expensive_limit))
        // Click analytics for one owned code. Under `/v1/urls/` so it never
        // collides with a shortcode, which lives at the root.
        .route("/v1/urls/{code}/stats", get(urls::stats))
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

mod accounts;
mod health;
mod resolve;
mod shorten;
mod urls;
