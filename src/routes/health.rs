use std::sync::Arc;

use axum::{Json, extract::State};
use serde_json::{Value, json};

use crate::state::AppState;

pub async fn health(State(_state): State<Arc<AppState>>) -> Json<Value> {
    Json(json!({ "status": "ok" }))
}
