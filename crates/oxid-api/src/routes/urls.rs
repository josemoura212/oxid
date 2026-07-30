//! The signed-in owner's links.

use std::sync::Arc;

use axum::{
    Json,
    extract::{Path, Query, State, rejection::JsonRejection},
};
use oxid_shared::{
    ClickPoint, ClickStats, ImportRequest, ImportResponse, LinkPage, MAX_IMPORT, MAX_URL_LEN,
    OwnedLink,
};
use serde::Deserialize;
use sqlx::types::chrono::{DateTime, Utc};
use url::Url;

use crate::{analytics::DateRange, auth::Session, codec, error::AppError, repo, state::AppState};

/// Page size ceiling. A client asking for more gets this — the limit exists to
/// bound one query's work, so honouring a larger request would defeat it.
const MAX_LIMIT: i64 = 100;
const DEFAULT_LIMIT: i64 = 20;

/// The dashboard offers 7/14/21/28-day windows; the data only lives 30 days
/// (the ClickHouse TTL), so anything longer is clamped to that.
const DEFAULT_STATS_DAYS: i64 = 7;
const MAX_STATS_DAYS: i64 = 30;

#[derive(Debug, Deserialize)]
pub(super) struct Pagination {
    /// Opaque to the client: `<rfc3339>|<code>`. Encoding both halves is what
    /// makes the cursor total — `created_at` alone is not unique, and two links
    /// created in the same instant would straddle the page boundary with one of
    /// them never appearing.
    cursor: Option<String>,
    limit: Option<i64>,
}

fn parse_cursor(raw: &str) -> Result<(DateTime<Utc>, i64), AppError> {
    let invalid = || AppError::InvalidInput("cursor is not valid");

    let (at, code) = raw.split_once('|').ok_or_else(invalid)?;

    let created_at = DateTime::parse_from_rfc3339(at)
        .map_err(|_| invalid())?
        .with_timezone(&Utc);

    let id = codec::resolve(code).ok_or_else(invalid)?;
    let id = i64::try_from(id).map_err(|_| invalid())?;

    Ok((created_at, id))
}

pub(super) async fn list(
    State(state): State<Arc<AppState>>,
    session: Session,
    Query(params): Query<Pagination>,
) -> Result<Json<LinkPage>, AppError> {
    let limit = params.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);

    let cursor = params.cursor.as_deref().map(parse_cursor).transpose()?;

    // One extra row, purely to learn whether another page exists. Answering
    // that with a second `COUNT(*)` query would double the work on every page
    // of a table this design expects to be very large.
    let fetch = limit.saturating_add(1);
    let mut rows = repo::list_owned(&state.db_pool, session.user_id, cursor, fetch).await?;

    let has_more = i64::try_from(rows.len()).unwrap_or(i64::MAX) > limit;
    if has_more {
        rows.pop();
    }

    let base = state.base_url.trim_end_matches('/');
    let mut links = Vec::with_capacity(rows.len());

    for row in rows {
        let id =
            u64::try_from(row.code_id).map_err(|_| AppError::Internal("row id is negative"))?;
        let code = codec::shortcode(id)
            .ok_or(AppError::Internal("row id outside the shortcode domain"))?;

        links.push(OwnedLink {
            short_url: format!("{base}/{code}"),
            code,
            long_url: row.long_url,
            created_at: row.created_at.to_rfc3339(),
        });
    }

    // Built from the last row actually returned, so it points at the boundary
    // the client has seen rather than at the row that was peeked and dropped.
    let next_cursor = has_more
        .then(|| links.last())
        .flatten()
        .map(|last| format!("{}|{}", last.created_at, last.code));

    Ok(Json(LinkPage { links, next_cursor }))
}

/// Claims a batch of URLs for the signed-in account.
///
/// One request rather than one `POST /v1/shorten` per link, and that is the
/// point: the write limit is five a second, so a browser replaying a list of
/// twenty would stall and one of fifty would fail halfway — leaving an import
/// that half happened, which is worse than one that did not.
///
/// Invalid URLs are counted, not rejected as a whole. The list comes from this
/// browser's own history and may hold something the current validation no longer
/// accepts; failing the batch would strand every good entry behind one bad one.
pub(super) async fn import(
    State(state): State<Arc<AppState>>,
    session: Session,
    payload: Result<Json<ImportRequest>, JsonRejection>,
) -> Result<Json<ImportResponse>, AppError> {
    let Json(body) = payload.map_err(|err| AppError::InvalidBody(err.body_text()))?;

    if body.urls.len() > MAX_IMPORT {
        return Err(AppError::InvalidInput("too many urls in one import"));
    }

    let mut imported = 0usize;
    let mut rejected = 0usize;

    for raw in &body.urls {
        let Ok(url) = Url::parse(raw) else {
            rejected = rejected.saturating_add(1);
            continue;
        };

        let acceptable = matches!(url.scheme(), "http" | "https")
            && url.has_host()
            && url.as_str().len() <= MAX_URL_LEN;

        if !acceptable {
            rejected = rejected.saturating_add(1);
            continue;
        }

        let long_url = url.as_str();
        let url_id = repo::upsert_url(&state.db_pool, long_url).await?;
        let code_id = repo::upsert_code(&state.db_pool, url_id, Some(session.user_id)).await?;

        let id = u64::try_from(code_id).map_err(|_| AppError::Internal("row id is negative"))?;
        let code = codec::shortcode(id)
            .ok_or(AppError::Internal("row id outside the shortcode domain"))?;

        // Warm on write, same reasoning as `shorten`: whoever just imported a
        // list is about to look at it. Always owned — an import claims the code
        // for the signed-in account.
        state.cache.set_url(&code, long_url, true).await;

        imported = imported.saturating_add(1);
    }

    Ok(Json(ImportResponse { imported, rejected }))
}

#[derive(Debug, Deserialize)]
pub(super) struct StatsQuery {
    days: Option<i64>,
}

/// Click analytics for one of the caller's codes.
///
/// The code has to belong to the caller, and a code that does not — whether it
/// exists under someone else or not at all — answers the same 404. Returning a
/// different status for "exists but not yours" would let one account probe which
/// codes another owns.
pub(super) async fn stats(
    State(state): State<Arc<AppState>>,
    session: Session,
    Path(code): Path<String>,
    Query(query): Query<StatsQuery>,
) -> Result<Json<ClickStats>, AppError> {
    let id = codec::resolve(&code).ok_or(AppError::NotFound)?;
    let code_id = i64::try_from(id).map_err(|_| AppError::NotFound)?;

    if !repo::owns_code(&state.db_pool, code_id, session.user_id).await? {
        return Err(AppError::NotFound);
    }

    let days = query
        .days
        .unwrap_or(DEFAULT_STATS_DAYS)
        .clamp(1, MAX_STATS_DAYS);
    let to = Utc::now();
    // Timestamp arithmetic keeps the whole thing checked, and the clamp above
    // means the seconds never overflow: at most 30 days of them.
    let window = days.saturating_mul(86_400);
    let from = DateTime::from_timestamp(to.timestamp().saturating_sub(window), 0).unwrap_or(to);

    let summary = state
        .clicks
        .summary(code_id, DateRange { from, to })
        .await
        .map_err(|_| AppError::Internal("failed to read analytics"))?;

    let series = summary
        .series
        .into_iter()
        .map(|point| ClickPoint {
            at: point.at.to_rfc3339(),
            clicks: point.clicks,
        })
        .collect();

    Ok(Json(ClickStats {
        total: summary.total,
        unique: summary.unique,
        series,
    }))
}
