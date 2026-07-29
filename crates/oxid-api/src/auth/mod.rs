//! Accounts, sessions and the extractors that read them.

pub mod password;
pub mod session;

use std::sync::Arc;

use axum::{extract::FromRequestParts, http::request::Parts};
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};

use crate::{error::AppError, state::AppState};

/// Cookie carrying the session id.
pub const SESSION_COOKIE: &str = "oxid_session";

/// Builds the session cookie.
///
/// `HttpOnly` keeps it out of reach of any script on the page, which is what
/// turns an XSS into a defaced page rather than a stolen account. `SameSite=Lax`
/// stops it riding along on cross-site form posts — the shape a CSRF takes here,
/// since `POST /v1/shorten` would otherwise accept a session from anywhere.
///
/// `Secure` is decided by the configured base URL rather than hardcoded: over
/// plain HTTP the browser would drop the cookie entirely, and local development
/// would fail in a way that looks like broken login code.
pub fn session_cookie(id: String, secure: bool, max_age_seconds: i64) -> Cookie<'static> {
    Cookie::build((SESSION_COOKIE, id))
        .http_only(true)
        .secure(secure)
        .same_site(SameSite::Lax)
        .path("/")
        .max_age(time::Duration::seconds(max_age_seconds))
        .build()
}

/// The cookie that replaces a session on logout.
///
/// Same name, path and attributes, empty value, zero lifetime. A cookie is
/// identified by name **and** path, so clearing it with a different path leaves
/// the original in place and the browser keeps sending it.
pub fn expired_cookie(secure: bool) -> Cookie<'static> {
    Cookie::build((SESSION_COOKIE, String::new()))
        .http_only(true)
        .secure(secure)
        .same_site(SameSite::Lax)
        .path("/")
        .max_age(time::Duration::ZERO)
        .build()
}

/// An authenticated caller. Rejects with 401 when there is no valid session.
#[derive(Debug, Clone, Copy)]
pub struct Session {
    pub user_id: i64,
}

/// A caller who may or may not be signed in.
///
/// Separate type rather than `Option<Session>` so a handler cannot forget which
/// one it asked for: `POST /v1/shorten` must keep working without a session, and
/// the type is what says so.
#[derive(Debug, Clone, Copy)]
pub struct MaybeSession(pub Option<i64>);

async fn resolve(parts: &Parts, state: &Arc<AppState>) -> Option<i64> {
    let jar = CookieJar::from_headers(&parts.headers);
    let id = jar.get(SESSION_COOKIE)?.value().to_owned();

    state.sessions.user_id(&id).await
}

impl FromRequestParts<Arc<AppState>> for Session {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &Arc<AppState>,
    ) -> Result<Self, Self::Rejection> {
        resolve(parts, state)
            .await
            .map(|user_id| Self { user_id })
            .ok_or(AppError::Unauthorized)
    }
}

impl FromRequestParts<Arc<AppState>> for MaybeSession {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &Arc<AppState>,
    ) -> Result<Self, Self::Rejection> {
        Ok(Self(resolve(parts, state).await))
    }
}
