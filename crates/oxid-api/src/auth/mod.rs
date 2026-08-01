//! Accounts, sessions and the extractors that read them.

pub mod password;
pub mod session;
pub mod token;

use std::sync::Arc;

use axum::{
    extract::FromRequestParts,
    http::{header, request::Parts},
};
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};

use crate::{error::AppError, repo, state::AppState};

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

/// A caller holding the **session cookie**, never a token.
///
/// [`Session`] accepts either credential, which is right for the endpoints the
/// extension uses. It is wrong for the ones that manage credentials: a token that
/// could mint another token turns a single stolen token into a permanent
/// foothold, since revoking the one that leaked leaves behind whatever it
/// created. Issuing and revoking stays behind the login, which is the credential
/// the person can see and revoke from the screen they are already on.
///
/// A separate type rather than a check inside the handlers, because a check is
/// something the next handler can forget to copy.
#[derive(Debug, Clone, Copy)]
pub struct WebSession {
    pub user_id: i64,
}

/// Who is calling: the session cookie, or a bearer token.
///
/// The cookie is tried first because it is the overwhelmingly common case — every
/// request from the site carries one, and only the extension carries a token. A
/// caller presenting both gets the cookie, which is the safer precedence: the
/// cookie is the credential the person can see and revoke from the screen they
/// are already looking at.
async fn resolve(parts: &Parts, state: &Arc<AppState>) -> Option<i64> {
    if let Some(user_id) = resolve_cookie(parts, state).await {
        return Some(user_id);
    }

    resolve_token(parts, state).await
}

/// The session cookie alone. Split out because [`WebSession`] needs exactly this
/// and must not fall through to a token.
async fn resolve_cookie(parts: &Parts, state: &Arc<AppState>) -> Option<i64> {
    let jar = CookieJar::from_headers(&parts.headers);
    let cookie = jar.get(SESSION_COOKIE)?;

    state.sessions.user_id(cookie.value()).await
}

/// Resolves an `Authorization: Bearer` token to its owner.
///
/// A malformed header costs no round trip — [`token::from_header`] rejects
/// anything without our prefix before the database is touched, so an endpoint
/// sprayed with junk credentials does not turn into a query per attempt.
///
/// A database failure reads as "not authenticated" rather than an error, matching
/// how the session store already treats a Redis blink: answering 500 across the
/// authenticated surface because a lookup failed is worse than treating the
/// caller as anonymous, and the anonymous path works.
async fn resolve_token(parts: &Parts, state: &Arc<AppState>) -> Option<i64> {
    let header = parts.headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let secret = token::from_header(header)?;

    repo::touch_token(&state.db_pool, &token::digest(secret))
        .await
        .ok()
        .flatten()
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

impl FromRequestParts<Arc<AppState>> for WebSession {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &Arc<AppState>,
    ) -> Result<Self, Self::Rejection> {
        resolve_cookie(parts, state)
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
