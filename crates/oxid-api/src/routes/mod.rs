use std::sync::Arc;

use axum::{
    Router,
    routing::{get, post},
};
use tower_http::trace::TraceLayer;

use crate::state::AppState;

mod health;
mod resolve;
mod shorten;

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/health", get(health::health))
        .route("/v1/shorten", post(shorten::shorten))
        // The redirect sits at the root: `/{code}` is the product. Prefixing it
        // with `/v1/urls/` would spend 9 characters on a URL whose whole point is
        // being short. Literal routes win over the parameter, so `/health` and
        // `/v1/shorten` still match themselves.
        .route("/{code}", get(resolve::resolve))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}
