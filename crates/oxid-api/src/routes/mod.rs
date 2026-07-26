use std::sync::Arc;

use axum::{Router, routing::get};
use tower_http::trace::TraceLayer;

use crate::state::AppState;

pub mod health;

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/health", get(health::health))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}
