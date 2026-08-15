//! Signup, login, logout, "who am I", and the four e-mail flows around them.
//!
//! Three of the handlers here answer the same thing whether or not the address
//! exists — signup, resend, forgot-password. That is not politeness, it is the
//! whole point: an endpoint that takes an address and answers differently
//! depending on whether it is registered is an account enumerator, and a
//! shortener's user list is a list of people worth phishing.
//!
//! The difference travels by e-mail, which is the one channel the person asking
//! does not control unless they own the mailbox.

use std::sync::Arc;

use axum::{
    Json,
    extract::{Query, State, rejection::JsonRejection},
    http::HeaderMap,
    response::{IntoResponse, Response},
};
use axum_extra::extract::cookie::CookieJar;
use oxid_shared::{
    AccountResponse, CredentialsRequest, EmailRequest, MAX_PASSWORD_LEN, MIN_PASSWORD_LEN,
    ResetPasswordRequest, SignupResponse, TokenRequest,
};
use serde::Deserialize;

use crate::{
    auth::{
        MaybeSession, SESSION_COOKIE, Session, expired_cookie,
        onetime::{Purpose, RESET_TTL_HOURS, TokenError},
        session_cookie,
    },
    email::{Lang, Mailer, Message},
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

fn parse<T: serde::de::DeserializeOwned>(
    payload: Result<Json<T>, JsonRejection>,
) -> Result<T, AppError> {
    payload
        .map(|Json(body)| body)
        .map_err(|err| AppError::InvalidBody(err.body_text()))
}

/// The language the browser asked for, for the message about to be sent.
fn lang_of(headers: &HeaderMap) -> Lang {
    Lang::from_header(
        headers
            .get(axum::http::header::ACCEPT_LANGUAGE)
            .and_then(|value| value.to_str().ok()),
    )
}

/// Hands a message to a background task and returns immediately.
///
/// **The send must not be on the response path.** Resend is an HTTPS call to
/// somebody else's service with a ten-second ceiling; waiting for it would make
/// signup as slow as the slowest thing between here and them, and a provider
/// outage would turn "your account was created" into a timeout.
///
/// The failure is logged and goes no further, which is the same decision stated
/// in [`crate::email`]: the account already exists, and the person can ask for
/// another link. Tying account creation to a successful send would make a mail
/// provider being down an outage of signup.
///
/// **Counted as well as logged, because nobody else is watching.** The person who
/// signed up is told a message was sent and has no way to learn it was not; a log
/// line only helps whoever happens to be reading logs at that moment. The counter
/// is what makes "no e-mail is going out" alertable, the same way
/// `click_events_dropped_total` does for the analytics worker.
///
/// `error` rather than `warn`: a message that does not leave is the failure of a
/// whole flow, not a caution.
fn deliver(mailer: &Mailer, message: Message) {
    let mailer = mailer.clone();

    tokio::spawn(async move {
        match mailer.send(&message).await {
            Ok(()) => metrics::counter!("email_send_total", "outcome" => "sent").increment(1),
            Err(err) => {
                metrics::counter!("email_send_total", "outcome" => "failed").increment(1);
                tracing::error!(
                    %err,
                    to = %message.to,
                    subject = %message.subject,
                    "could not send email"
                );
            }
        }
    });
}

/// Turns a token lookup into the answer the caller owes the client.
///
/// `Ok(None)` is a dead link and answers 400. An error is **ours**, and answering
/// 400 for it would tell someone with a perfectly good link to throw it away —
/// and send them to ask for another that the same broken Redis cannot issue.
///
/// A 500 here does not reopen enumeration: it happens identically for a valid
/// token, a spent one and one that never existed.
fn spent(outcome: Result<Option<i64>, TokenError>, purpose: &'static str) -> Result<i64, AppError> {
    match outcome {
        Ok(Some(user_id)) => Ok(user_id),
        Ok(None) => Err(AppError::InvalidToken),
        Err(err) => {
            tracing::error!(%err, purpose, "the one-time token store did not answer");
            Err(AppError::Internal("could not check this link"))
        }
    }
}

/// Issues a token, logging why if the store refuses.
///
/// `AppError::Internal` carries a `&'static str`, so converting with `|_|` threw
/// the reason away: "Redis is not configured" and "Redis is out of memory" both
/// arrived in the log as the same sentence.
async fn issue(state: &AppState, purpose: Purpose, user_id: i64) -> Result<String, AppError> {
    state.tokens.issue(purpose, user_id).await.map_err(|err| {
        tracing::error!(%err, ?purpose, user_id, "could not issue a one-time token");
        AppError::Internal("failed to issue a link")
    })
}

/// Creates an account and mails a confirmation link — or mails a warning to an
/// address that already has one.
///
/// **Answers the same 200 either way, and that is the feature.** The old
/// behaviour returned 409 for a registered address, which handed anyone with a
/// list of e-mails a free membership check. Now the response body, the status and
/// the work done are identical; only the message differs, and only its owner
/// reads it.
///
/// No session is created. Nobody is signed in yet — the address is unconfirmed,
/// and confirming it is what the next step is for.
pub(super) async fn signup(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    payload: Result<Json<CredentialsRequest>, JsonRejection>,
) -> Result<Json<SignupResponse>, AppError> {
    let body = parse(payload)?;
    let email = validate_email(&body.email)?;
    validate_password(&body.password)?;
    let lang = lang_of(&headers);

    // Hashed before the branch, so both paths spend the same Argon2 cost. Moving
    // this inside the `Some` arm would make the taken-address case measurably
    // faster and rebuild the oracle out of timing — the same trap the login path
    // already spends a decoy hash to avoid.
    let hash = state.hasher.hash(body.password.clone()).await?;

    // `None` means the unique index refused it. Checking first and inserting
    // second would be a race that two concurrent signups win together.
    match repo::create_user(&state.db_pool, email, &hash).await? {
        Some(user_id) => {
            // With confirmation off there is nothing to confirm: the account is
            // marked verified at creation and no link is sent. That is the whole
            // of the flag's effect on this handler, and it is also where the
            // enumeration it reopens begins — the account works immediately, so a
            // login right after tells a free address from a taken one.
            if !state.require_confirmation {
                repo::mark_email_verified(&state.db_pool, user_id).await?;
                return Ok(Json(SignupResponse {
                    email: email.to_owned(),
                }));
            }

            let token = issue(&state, Purpose::Verify, user_id).await?;

            deliver(
                &state.mailer,
                Message::confirm(email, &state.site_url, &token, lang),
            );
        }
        None => {
            // Nothing is created and nothing changes. The message exists so the
            // owner of the address finds out someone tried, and it deliberately
            // carries no link — see `Message::already_registered`.
            deliver(
                &state.mailer,
                Message::already_registered(email, &state.site_url, lang),
            );
        }
    }

    Ok(Json(SignupResponse {
        email: email.to_owned(),
    }))
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

    // **The same 401 as a wrong password, and that is not imprecision.**
    //
    // A distinct 403 here reads as more helpful and rebuilds the exact oracle the
    // rest of this flow gives up a 409 to close. Two requests per address were
    // enough: sign up with a random password, then sign in with it. A free
    // address answered 403 (the signup created it, the password matches, it is
    // unconfirmed); a taken one answered 401 (the signup did nothing, the
    // password does not match). Deterministic, no timing involved.
    //
    // What it costs is real: someone whose password is correct is told it is not.
    // That is paid for on the sign-in screen, which offers to send another
    // confirmation link beside "forgot your password" — unconditionally, so the
    // offer itself says nothing about the address either.
    if state.require_confirmation && credentials.email_verified_at.is_none() {
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

/// Spends a confirmation link.
///
/// Answers 204 on a token that was valid, whether or not it had already been
/// applied — the address ends up confirmed either way, and that is what the
/// caller asked for. A spent token, an expired one and one that never existed
/// are all the same 400, because distinguishing them tells someone holding a
/// stolen link whether it still had value.
pub(super) async fn verify_email(
    State(state): State<Arc<AppState>>,
    payload: Result<Json<TokenRequest>, JsonRejection>,
) -> Result<Response, AppError> {
    let body = parse(payload)?;

    let user_id = spent(
        state
            .tokens
            .consume(Purpose::Verify, body.token.trim())
            .await,
        "verify",
    )?;

    // The boolean says whether this call is what confirmed it. Nothing branches
    // on it — a token is single-use, so a second confirmation cannot arrive
    // through this path anyway.
    let _confirmed = repo::mark_email_verified(&state.db_pool, user_id).await?;

    Ok(axum::http::StatusCode::NO_CONTENT.into_response())
}

/// Sends another confirmation link.
///
/// Always 204, for an unknown address as much as a known one. The alternative
/// leaks membership through an endpoint that needs no password at all — strictly
/// worse than the signup did, since it costs an attacker nothing.
///
/// A confirmed address gets nothing, silently. There is nothing to confirm, and
/// sending "here is your link" to someone already confirmed would be a message
/// with no action in it that anyone could trigger at will.
pub(super) async fn resend_verification(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    payload: Result<Json<EmailRequest>, JsonRejection>,
) -> Result<Response, AppError> {
    let body = parse(payload)?;
    let email = body.email.trim();
    let lang = lang_of(&headers);

    if let Some(account) = repo::find_account(&state.db_pool, email).await?
        && account.email_verified_at.is_none()
    {
        let token = issue(&state, Purpose::Verify, account.user_id).await?;

        deliver(
            &state.mailer,
            Message::confirm(email, &state.site_url, &token, lang),
        );
    }

    Ok(axum::http::StatusCode::NO_CONTENT.into_response())
}

/// Starts a password reset.
///
/// Always 204. Confirming that an address has an account here would reopen the
/// oracle the signup just closed, through a cheaper door — and the temptation is
/// real, because "no account with that e-mail" is genuinely helpful to someone
/// who typed it wrong. They get the same answer as everyone else: whoever does
/// not receive a message either mistyped it or has no account, and the product
/// does not say which.
///
/// An unconfirmed address is allowed through. Reading a message sent to it is
/// the same proof the confirmation asks for, so completing a reset confirms it —
/// see [`repo::update_password`].
pub(super) async fn forgot_password(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    payload: Result<Json<EmailRequest>, JsonRejection>,
) -> Result<Response, AppError> {
    let body = parse(payload)?;
    let email = body.email.trim();
    let lang = lang_of(&headers);

    if let Some(account) = repo::find_account(&state.db_pool, email).await? {
        let token = issue(&state, Purpose::Reset, account.user_id).await?;

        deliver(
            &state.mailer,
            Message::reset(email, &state.site_url, &token, RESET_TTL_HOURS, lang),
        );
    }

    Ok(axum::http::StatusCode::NO_CONTENT.into_response())
}

#[derive(Debug, Deserialize)]
pub(super) struct TokenQuery {
    token: String,
}

/// Checks a reset link without spending it.
///
/// This is what the reset screen calls when it opens, and the reason the token
/// store has a `peek` at all: consuming on page load would kill the link before
/// anyone typed a password. A refresh, a second tab, or a mail client that
/// pre-fetches links would each be enough.
///
/// The link is spent by [`reset_password`], on save.
pub(super) async fn check_reset(
    State(state): State<Arc<AppState>>,
    Query(query): Query<TokenQuery>,
) -> Result<Response, AppError> {
    spent(
        state.tokens.peek(Purpose::Reset, query.token.trim()).await,
        "reset",
    )?;

    Ok(axum::http::StatusCode::NO_CONTENT.into_response())
}

/// Sets a new password and ends every session.
///
/// **Revoking everything is the point, not housekeeping.** Someone reaching for
/// a reset has usually lost control of the account; leaving the old sessions
/// alive would hand the new password to the owner and leave the intruder exactly
/// where they were.
///
/// The token is spent between the hash and the write. Hashing first keeps an
/// expired link from paying for Argon2, and consuming before the update means two
/// simultaneous submissions cannot both apply — the loser gets a 400.
pub(super) async fn reset_password(
    State(state): State<Arc<AppState>>,
    payload: Result<Json<ResetPasswordRequest>, JsonRejection>,
) -> Result<Response, AppError> {
    let body = parse(payload)?;
    validate_password(&body.password)?;
    let token = body.token.trim();

    // **Spent before the hash, not after.** Peeking first and hashing second let
    // one live token be submitted N times in parallel: every one passed the peek
    // before any consume won, and every one paid an Argon2. With
    // `hash_concurrency: 1` that is the whole authentication surface — login and
    // signup included — answering 503 from a single valid reset link.
    //
    // `GETDEL` makes exactly one submission win, so the losers are refused before
    // spending anything. The cost is that a hash failure burns the link; that is
    // fail-closed and recoverable by asking for another.
    let user_id = spent(state.tokens.consume(Purpose::Reset, token).await, "reset")?;

    let hash = state.hasher.hash(body.password.clone()).await?;

    // **Before the write, not after, and the order is the whole point.** Revoking
    // is what a reset exists to do — someone reaching for one has usually lost
    // control of the account — so it must not be the step that fails *after* the
    // password has already changed. In that order a Redis blink answered 500 while
    // the new password silently worked, the link was spent, and the intruder's
    // session was still alive: every signal the person had said "it failed", and
    // the one thing that mattered had failed silently.
    //
    // Failing here instead is benign: sessions are gone, the password is
    // unchanged, and the spent link is replaced by asking for another.
    state.sessions.revoke_all(user_id).await.map_err(|err| {
        tracing::error!(%err, user_id, "could not revoke sessions during a reset");
        AppError::Internal("failed to revoke sessions")
    })?;

    // **API tokens too, not just sessions.** A stolen session can mint a personal
    // access token, and that token has no expiry and is untouched by a session
    // revoke. Without this, someone who reset their password after a compromise
    // would keep every cookie session dead and hand the intruder a credential
    // that still shortens into their account — one they would have to find and
    // revoke by hand, under a name the intruder chose.
    let revoked = repo::revoke_all_tokens(&state.db_pool, user_id).await?;
    if revoked > 0 {
        tracing::info!(user_id, revoked, "password reset revoked API tokens");
    }

    // `false` means no row matched — the token outlived the account it pointed
    // at. Unreachable today (nothing deletes accounts), which is exactly why it
    // is worth catching now: answering 204 would tell someone their password
    // changed when it did not.
    if !repo::update_password(&state.db_pool, user_id, &hash).await? {
        tracing::error!(user_id, "a reset token pointed at an account that is gone");
        return Err(AppError::Internal("could not set the password"));
    }

    Ok(axum::http::StatusCode::NO_CONTENT.into_response())
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

/// Signs the caller out of every device by revoking all their sessions.
///
/// Requires a valid session — you can only nuke your own. Clears this browser's
/// cookie too, since the session it points at is among the ones just revoked.
///
/// Unlike `logout`, a storage failure here is surfaced as 500 rather than
/// swallowed: someone reaching for "sign out everywhere" is usually responding
/// to a compromise, and a silent partial revoke would tell them they are safe
/// when they are not.
pub(super) async fn logout_all(
    State(state): State<Arc<AppState>>,
    jar: CookieJar,
    session: Session,
) -> Result<Response, AppError> {
    state
        .sessions
        .revoke_all(session.user_id)
        .await
        .map_err(|_| AppError::Internal("failed to revoke sessions"))?;

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
