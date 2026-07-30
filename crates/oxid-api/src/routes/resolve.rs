use std::{hash::Hasher, sync::Arc};

use axum::{
    extract::{Path, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use sqlx::types::chrono::Utc;

use crate::{analytics::ClickEvent, cache::Cached, codec, error::AppError, repo, state::AppState};

/// A malformed code answers 404, not 400. Answering 400 for bad syntax and 404
/// for well-formed-but-missing would leak the shortcode format to anyone probing
/// the endpoint with a handful of requests.
pub(super) async fn resolve(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(code): Path<String>,
) -> Result<Response, AppError> {
    // Cache first, and before decoding: a scanner walking random codes should
    // cost one Redis round trip, not a database query each.
    match state.cache.get(&code).await {
        Some(Cached::Url { long_url, owned }) => {
            return finish(&state, &code, &long_url, owned, &headers);
        }
        Some(Cached::Missing) => return Err(AppError::NotFound),
        None => {}
    }

    let id = codec::resolve(&code).ok_or(AppError::NotFound)?;
    let id = i64::try_from(id).map_err(|_| AppError::NotFound)?;

    let Some(resolved) = repo::resolve_code(&state.db_pool, id).await? else {
        // Cache the absence too, otherwise every request for a nonexistent code
        // reaches Postgres — a free denial-of-service vector.
        state.cache.set_missing(&code).await;
        return Err(AppError::NotFound);
    };

    state
        .cache
        .set_url(&code, &resolved.long_url, resolved.owned)
        .await;

    finish(&state, &code, &resolved.long_url, resolved.owned, &headers)
}

/// Emits the click when the code has an owner, then redirects.
///
/// The 301/302 split is the whole reason analytics can exist: a 301 is cached by
/// the browser, so the second click never reaches the server and cannot be
/// counted. Only an owned code becomes a 302 — the one that has a dashboard to
/// feed. Anonymous codes stay 301 and cacheable, which is the path the load tests
/// measure.
fn finish(
    state: &AppState,
    code: &str,
    long_url: &str,
    owned: bool,
    headers: &HeaderMap,
) -> Result<Response, AppError> {
    let location = HeaderValue::from_str(long_url)
        .map_err(|_| AppError::Internal("stored url is not a valid header value"))?;

    if !owned {
        // 301, not `Redirect::permanent` — that emits 308, which preserves the
        // method. Only GET reaches this route, and 301 is what shorteners have
        // always used, so caches and old clients handle it best.
        return Ok((
            StatusCode::MOVED_PERMANENTLY,
            [(header::LOCATION, location)],
        )
            .into_response());
    }

    // Decoding again here rather than threading the id through: on a cache hit it
    // was never decoded, and the bijection inverse is a single modular multiply.
    // Only owned codes reach this, so it runs on the minority of redirects.
    if let Some(id) = codec::resolve(code).and_then(|id| i64::try_from(id).ok()) {
        state.clicks_tx.emit(ClickEvent {
            created_at: Utc::now(),
            code_id: id,
            // Enrichment (country, device, referer, language) is a later slice.
            country: String::new(),
            device: String::new(),
            os: String::new(),
            browser: String::new(),
            referer_host: String::new(),
            lang: String::new(),
            is_bot: 0,
            visitor_hash: visitor_hash(headers),
        });
    }

    // 302, so the browser does not cache it and each click reaches the server to
    // be counted. The cost — a request per click instead of one then silence — is
    // the point of counting.
    Ok((StatusCode::FOUND, [(header::LOCATION, location)]).into_response())
}

/// A per-visitor, per-day identifier that stores nobody's address.
///
/// `hash(ip + user-agent + day)`. The day is the salt: it rotates the hash every
/// midnight, so the same person is a different value tomorrow and cannot be
/// followed across days. The raw address is never stored, only this fold of it.
///
/// `DefaultHasher` has fixed keys, so the value is stable across requests and
/// restarts — which is what lets `uniq()` count one visitor as one. It is not a
/// cryptographic secret; a keyed daily salt is a later refinement.
fn visitor_hash(headers: &HeaderMap) -> u64 {
    let header = |name: &str| {
        headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
    };

    // First hop of X-Forwarded-For is the client the CDN saw; fall back to
    // X-Real-IP. Absent both (a direct request), the empty string still yields a
    // stable-per-UA value.
    let ip = header("x-forwarded-for")
        .split(',')
        .next()
        .unwrap_or("")
        .trim();
    let user_agent = header("user-agent");
    let day = Utc::now().format("%Y-%m-%d").to_string();

    // `write` the bytes with a separator between, rather than `Hash::hash` each
    // field: the separator stops "ab"+"c" from colliding with "a"+"bc", and
    // reading `.as_bytes()` is a use clippy recognizes where `.hash()` was not.
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    hasher.write(ip.as_bytes());
    hasher.write(b"\0");
    hasher.write(user_agent.as_bytes());
    hasher.write(b"\0");
    hasher.write(day.as_bytes());
    hasher.finish()
}
