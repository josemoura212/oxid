use std::sync::Arc;

use axum::{
    Json,
    extract::{State, rejection::JsonRejection},
};
use oxid_shared::{MAX_URL_LEN, ShortenRequest, ShortenResponse};
use url::Url;

use crate::{auth::MaybeSession, codec, error::AppError, repo, state::AppState};

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

    // Measured after parsing, because normalization can lengthen the string —
    // `as_str` is what actually gets stored, so it is what has to fit.
    //
    // The database keeps the same CHECK, and this does not make it redundant:
    // without this the only way to learn a URL is too long is to send it to
    // Postgres and be refused, spending one of eight pooled connections to say
    // no. Rejecting here also keeps the message and the limit reading from one
    // constant instead of drifting apart.
    if url.as_str().len() > MAX_URL_LEN {
        return Err(AppError::UrlTooLong);
    }

    Ok(url)
}

pub(super) async fn shorten(
    State(state): State<Arc<AppState>>,
    // `MaybeSession`, never `Session`: shortening without an account is the
    // default path and stays that way. The session only decides who the code
    // belongs to.
    MaybeSession(owner_id): MaybeSession,
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

    // Two steps, no transaction. If the first commits and the second fails, the
    // leftover is a URL with no code — invisible, harmless, and reused by the
    // next attempt. A transaction would cost more on the write path to prevent
    // litter that costs nothing.
    let url_id = repo::upsert_url(&state.db_pool, long_url).await?;
    let code_id = repo::upsert_code(&state.db_pool, url_id, owner_id).await?;

    let id = u64::try_from(code_id).map_err(|_| AppError::Internal("row id is negative"))?;
    let code =
        codec::shortcode(id).ok_or(AppError::Internal("row id outside the shortcode domain"))?;

    // Populate on write, not only on read miss: whoever just created the link is
    // about to click it, so this turns a guaranteed miss into a hit. The cached
    // value carries whether the code has an owner, so the redirect decides 301
    // vs 302 without a database hit.
    state
        .cache
        .set_url(&code, long_url, owner_id.is_some())
        .await;

    let short_url = format!("{}/{code}", state.base_url.trim_end_matches('/'));

    Ok(Json(ShortenResponse {
        code,
        short_url,
        long_url: long_url.to_owned(),
    }))
}
