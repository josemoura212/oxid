//! Signup, login, logout and "who am I".

use std::sync::Arc;

use axum::{
    Json,
    extract::{State, rejection::JsonRejection},
    response::{IntoResponse, Response},
};
use axum_extra::extract::cookie::CookieJar;
use oxid_shared::{AccountResponse, CredentialsRequest, MAX_PASSWORD_LEN, MIN_PASSWORD_LEN};

use crate::{
    auth::{MaybeSession, SESSION_COOKIE, Session, expired_cookie, session_cookie},
    error::AppError,
    repo,
    state::AppState,
};

/// Deliberately permissive. Anything stricter than "one @, something either
/// side, no spaces" rejects addresses that are perfectly valid — and the only
/// check that proves an address works is sending mail to it.
fn validate_email(raw: &str) -> Result<&str, AppError> {
    let email = raw.trim();

    let valid = email.split_once('@').is_some_and(|(local, domain)| {
        !local.is_empty() && domain.contains('.') && !domain.starts_with('.')
    }) && !email.contains(char::is_whitespace)
        && email.len() <= 254;

    if valid {
        Ok(email)
    } else {
        Err(AppError::InvalidInput("email is not valid"))
    }
}

fn validate_password(password: &str) -> Result<(), AppError> {
    // Counted in characters, not bytes: a passphrase in Portuguese or Japanese
    // would otherwise clear the bar on accents alone.
    let length = password.chars().count();

    if length < MIN_PASSWORD_LEN {
        return Err(AppError::InvalidInput(
            "password must be at least 12 characters",
        ));
    }

    if length > MAX_PASSWORD_LEN {
        return Err(AppError::InvalidInput("password is too long"));
    }

    Ok(())
}

fn parse(
    payload: Result<Json<CredentialsRequest>, JsonRejection>,
) -> Result<CredentialsRequest, AppError> {
    payload
        .map(|Json(body)| body)
        .map_err(|err| AppError::InvalidBody(err.body_text()))
}

pub(super) async fn signup(
    State(state): State<Arc<AppState>>,
    jar: CookieJar,
    payload: Result<Json<CredentialsRequest>, JsonRejection>,
) -> Result<Response, AppError> {
    let body = parse(payload)?;
    let email = validate_email(&body.email)?;
    validate_password(&body.password)?;

    // Off the runtime and under the concurrency cap. Signup hashes too, so it is
    // the same denial-of-service lever as login and gets the same treatment.
    let hash = state.hasher.hash(body.password.clone()).await?;

    // `None` means the unique index refused it. Checking first and inserting
    // second would be a race that two concurrent signups win together.
    //
    // KNOWN TRADE-OFF: answering 409 tells the caller this address has an
    // account, which is the enumeration the login path works hard to prevent —
    // identical message, identical `type`, a decoy burning the same CPU. Here it
    // is given away for free.
    //
    // Closing it properly needs e-mail: answer 200, create nothing, and send a
    // message to the address, so the owner finds out and a prober does not.
    // Without a mail path, the alternative is answering 200 and lying, which
    // leaves someone who typed a registered address with no idea why they are
    // not signed in. Registered in ROADMAP.md rather than hidden here.
    let Some(user_id) = repo::create_user(&state.db_pool, email, &hash).await? else {
        return Err(AppError::EmailTaken);
    };

    let id = state
        .sessions
        .create(user_id)
        .await
        .map_err(|_| AppError::Internal("failed to create session"))?;

    let cookie = session_cookie(id, state.secure_cookies, state.session_ttl_seconds);

    Ok((
        jar.add(cookie),
        Json(AccountResponse {
            id: user_id,
            email: email.to_owned(),
        }),
    )
        .into_response())
}

pub(super) async fn login(
    State(state): State<Arc<AppState>>,
    jar: CookieJar,
    payload: Result<Json<CredentialsRequest>, JsonRejection>,
) -> Result<Response, AppError> {
    let body = parse(payload)?;
    // Not validated for shape here, on purpose: rejecting a malformed e-mail
    // early would answer faster than a real lookup, and that difference is
    // exactly the oracle the decoy below exists to close.
    let email = body.email.trim();

    let found = repo::find_credentials(&state.db_pool, email).await?;

    let Some(credentials) = found else {
        // Spend what a real verification costs. Without this, "no such account"
        // returns in microseconds and "wrong password" in tens of milliseconds
        // — the response time enumerates accounts regardless of what the body
        // says.
        //
        // The `?` matters: under overload this answers 503, the same as the real
        // path would. Swallowing the error here would make the unknown-e-mail
        // case the only one that never returns 503 — an oracle rebuilt out of
        // status codes instead of timing.
        state.hasher.spend_decoy(body.password.clone()).await?;
        return Err(AppError::InvalidCredentials);
    };

    let matches = state
        .hasher
        .verify(body.password.clone(), credentials.password_hash.clone())
        .await?;

    if !matches {
        return Err(AppError::InvalidCredentials);
    }

    let id = state
        .sessions
        .create(credentials.user_id)
        .await
        .map_err(|_| AppError::Internal("failed to create session"))?;

    let cookie = session_cookie(id, state.secure_cookies, state.session_ttl_seconds);

    Ok((
        jar.add(cookie),
        Json(AccountResponse {
            id: credentials.user_id,
            email: email.to_owned(),
        }),
    )
        .into_response())
}

/// Revokes server-side and clears the cookie.
///
/// Both halves matter. Clearing only the cookie leaves a session id that still
/// authenticates anyone who captured it; revoking only server-side leaves the
/// browser sending a dead cookie on every request.
///
/// Answers 204 whether or not there was a session — "you are signed out" is
/// true either way, and distinguishing them tells a caller whether a stolen
/// cookie was still live.
pub(super) async fn logout(
    State(state): State<Arc<AppState>>,
    jar: CookieJar,
) -> Result<Response, AppError> {
    if let Some(cookie) = jar.get(SESSION_COOKIE) {
        state.sessions.revoke(cookie.value()).await;
    }

    Ok((
        jar.add(expired_cookie(state.secure_cookies)),
        axum::http::StatusCode::NO_CONTENT,
    )
        .into_response())
}

pub(super) async fn me(
    State(state): State<Arc<AppState>>,
    session: Session,
) -> Result<Json<AccountResponse>, AppError> {
    let email = repo::find_email(&state.db_pool, session.user_id)
        .await?
        // The session outlived its user row. Possible if an account is deleted
        // while signed in; treating it as unauthenticated is the honest answer.
        .ok_or(AppError::Unauthorized)?;

    Ok(Json(AccountResponse {
        id: session.user_id,
        email,
    }))
}

/// Whether the caller is signed in, without requiring it.
///
/// Exists so the front end can render the right thing on first paint without
/// treating a 401 from `/v1/me` as an error worth showing.
pub(super) async fn session_state(MaybeSession(user_id): MaybeSession) -> Json<Option<i64>> {
    Json(user_id)
}
