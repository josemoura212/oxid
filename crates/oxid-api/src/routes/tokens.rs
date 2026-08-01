//! Minting, listing and revoking API tokens.
//!
//! Every handler takes [`WebSession`] — the session cookie, never a token. That
//! distinction is the point: [`crate::auth::Session`] accepts either credential,
//! which is right for the endpoints the extension calls and wrong for these. A
//! token that could mint another token turns one stolen token into a permanent
//! foothold, because revoking the leaked one leaves behind whatever it created.
//! Issuing and revoking stay behind the login the person can see and revoke from.

use std::sync::Arc;

use axum::{
    Json,
    extract::{Path, State, rejection::JsonRejection},
    http::StatusCode,
};
use oxid_shared::{ApiTokenSummary, CreateTokenRequest, CreatedToken, MAX_TOKEN_NAME_LEN};

use crate::{
    auth::{WebSession, token},
    error::AppError,
    repo,
    state::AppState,
};

/// How many tokens one account may hold at once.
///
/// Not a licensing decision — a bound on what a stolen session can create. Twelve
/// is past any plausible number of devices, and small enough that a list stays
/// readable on the screen that has to make revoking easy.
const MAX_TOKENS: usize = 12;

fn summary(row: repo::ApiToken) -> ApiTokenSummary {
    ApiTokenSummary {
        id: row.id,
        name: row.name,
        created_at: row.created_at.to_rfc3339(),
        last_used_at: row.last_used_at.map(|at| at.to_rfc3339()),
    }
}

/// Mints a token and returns it **once**.
///
/// The secret is in this response and nowhere else — the database holds a digest
/// the server cannot reverse. If it is lost, the answer is to revoke and mint
/// again, which is a worse experience than storing it and a much better property.
pub(super) async fn create(
    State(state): State<Arc<AppState>>,
    session: WebSession,
    payload: Result<Json<CreateTokenRequest>, JsonRejection>,
) -> Result<(StatusCode, Json<CreatedToken>), AppError> {
    let Json(body) = payload.map_err(|err| AppError::InvalidBody(err.body_text()))?;

    let name = body.name.trim();
    if name.is_empty() || name.chars().count() > MAX_TOKEN_NAME_LEN {
        return Err(AppError::InvalidInput("token name is not valid"));
    }

    // Counted before minting rather than enforced by a constraint: the limit is
    // about how many a caller may hold, and a UNIQUE index cannot express that.
    let existing = repo::list_tokens(&state.db_pool, session.user_id).await?;
    if existing.len() >= MAX_TOKENS {
        return Err(AppError::InvalidInput("too many tokens"));
    }

    let minted = token::mint();
    let id = repo::create_token(&state.db_pool, session.user_id, name, &minted.hash).await?;

    let created = repo::list_tokens(&state.db_pool, session.user_id)
        .await?
        .into_iter()
        .find(|row| row.id == id)
        .ok_or(AppError::Internal("token vanished after being created"))?;

    Ok((
        StatusCode::CREATED,
        Json(CreatedToken {
            token: summary(created),
            secret: minted.secret,
        }),
    ))
}

pub(super) async fn list(
    State(state): State<Arc<AppState>>,
    session: WebSession,
) -> Result<Json<Vec<ApiTokenSummary>>, AppError> {
    let rows = repo::list_tokens(&state.db_pool, session.user_id).await?;

    Ok(Json(rows.into_iter().map(summary).collect()))
}

/// Revokes a token. 404 when it is not the caller's, which is the same answer as
/// one that does not exist — revoking is scoped in the WHERE clause, so probing
/// for someone else's token ids learns nothing.
pub(super) async fn revoke(
    State(state): State<Arc<AppState>>,
    session: WebSession,
    Path(id): Path<i64>,
) -> Result<StatusCode, AppError> {
    if repo::revoke_token(&state.db_pool, id, session.user_id).await? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(AppError::NotFound)
    }
}
