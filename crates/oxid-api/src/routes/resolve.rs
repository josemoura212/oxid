use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};

use crate::{cache::Cached, codec, error::AppError, repo, state::AppState};

/// A malformed code answers 404, not 400. Answering 400 for bad syntax and 404
/// for well-formed-but-missing would leak the shortcode format to anyone probing
/// the endpoint with a handful of requests.
pub(super) async fn resolve(
    State(state): State<Arc<AppState>>,
    Path(code): Path<String>,
) -> Result<Response, AppError> {
    // Cache first, and before decoding: a scanner walking random codes should
    // cost one Redis round trip, not a database query each.
    match state.cache.get(&code).await {
        Some(Cached::Url(long_url)) => return redirect(&long_url),
        Some(Cached::Missing) => return Err(AppError::NotFound),
        None => {}
    }

    let id = codec::resolve(&code).ok_or(AppError::NotFound)?;
    let id = i64::try_from(id).map_err(|_| AppError::NotFound)?;

    let Some(long_url) = repo::get_url(&state.db_pool, id).await? else {
        // Cache the absence too, otherwise every request for a nonexistent code
        // reaches Postgres — a free denial-of-service vector.
        state.cache.set_missing(&code).await;
        return Err(AppError::NotFound);
    };

    state.cache.set_url(&code, &long_url).await;

    redirect(&long_url)
}

fn redirect(long_url: &str) -> Result<Response, AppError> {
    let location = HeaderValue::from_str(long_url)
        .map_err(|_| AppError::Internal("stored url is not a valid header value"))?;

    // 301, not `Redirect::permanent` — that one emits 308, which preserves the
    // request method. Irrelevant here (only GET reaches this route) and 301 is
    // what shorteners have always used, so caches and old clients handle it best.
    //
    // Browsers cache 301 aggressively, so repeat visits never reach the server.
    // Great for load, and the reason analytics are out of scope.
    Ok((
        StatusCode::MOVED_PERMANENTLY,
        [(header::LOCATION, location)],
    )
        .into_response())
}
