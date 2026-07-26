use std::sync::Arc;

use axum::{
    Json,
    extract::{State, rejection::JsonRejection},
};
use oxid_shared::{ShortenRequest, ShortenResponse};
use url::Url;

use crate::{codec, error::AppError, repo, state::AppState};

/// Parsing alone is not enough: `javascript:alert(1)` and `file:///etc/passwd`
/// are syntactically valid URLs. Without pinning the scheme, the shortener
/// becomes an XSS vector wearing a trusted domain.
const ALLOWED_SCHEMES: [&str; 2] = ["http", "https"];

fn validate(raw: &str) -> Result<Url, AppError> {
    let url = Url::parse(raw).map_err(|_| AppError::InvalidUrl("could not parse url"))?;

    if !ALLOWED_SCHEMES.contains(&url.scheme()) {
        return Err(AppError::InvalidUrl(
            "only http and https urls are accepted",
        ));
    }

    if !url.has_host() {
        return Err(AppError::InvalidUrl("url must have a host"));
    }

    Ok(url)
}

pub(super) async fn shorten(
    State(state): State<Arc<AppState>>,
    payload: Result<Json<ShortenRequest>, JsonRejection>,
) -> Result<Json<ShortenResponse>, AppError> {
    // Taking the rejection as a value, instead of letting the extractor answer on
    // its own, keeps every error in the `ErrorResponse` shape the front end
    // expects — axum's built-in rejection replies in plain text.
    let Json(payload) = payload.map_err(|err| AppError::InvalidBody(err.body_text()))?;

    let url = validate(&payload.url)?;
    // `as_str` yields the WHATWG-normalized form, so `https://a.com` and
    // `https://a.com/` hash to one row instead of two.
    let long_url = url.as_str();

    let id = repo::insert_url(&state.db_pool, long_url).await?;
    let id = u64::try_from(id).map_err(|_| AppError::Internal("row id is negative"))?;
    let code =
        codec::shortcode(id).ok_or(AppError::Internal("row id outside the shortcode domain"))?;

    let short_url = format!("{}/{code}", state.base_url.trim_end_matches('/'));

    Ok(Json(ShortenResponse {
        code,
        short_url,
        long_url: long_url.to_owned(),
    }))
}
