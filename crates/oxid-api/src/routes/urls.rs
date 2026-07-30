//! The signed-in owner's links.

use std::{collections::HashMap, sync::Arc};

use axum::{
    Json,
    extract::{Path, Query, State, rejection::JsonRejection},
};
use oxid_shared::{
    ClickPoint, ClickStats, ImportRequest, ImportResponse, LinkPage, MAX_IMPORT, MAX_URL_LEN,
    OverviewLink, OverviewStats, OwnedLink,
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

/// Seconds in a day, for the overview's day grid.
const DAY_SECONDS: i64 = 86_400;

/// How many of an owner's codes the overview pulls from Postgres — the size of
/// the `IN` list the ClickHouse query then builds.
const MAX_OVERVIEW_FETCH: i64 = 50;

/// How many lines the overview chart actually draws, the busiest first. Beyond a
/// handful a multi-line chart stops being readable, so the rest are folded away
/// rather than crammed on.
const MAX_OVERVIEW_LINKS: usize = 8;

/// The half-open window a `days` count maps to, `[now - days, now]`.
///
/// The clamp upstream keeps `days` within the 30-day TTL, so the second
/// arithmetic never overflows: at most 30 days of them.
fn window(days: i64) -> DateRange {
    let to = Utc::now();
    let span = days.saturating_mul(DAY_SECONDS);
    let from = DateTime::from_timestamp(to.timestamp().saturating_sub(span), 0).unwrap_or(to);
    DateRange { from, to }
}

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

    let summary = state
        .clicks
        .summary(code_id, window(days))
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

/// The aggregate screen: every one of the caller's links on one shared day axis.
///
/// Owner-scoped by construction, not by a check: it reads only its own codes from
/// Postgres and asks ClickHouse about exactly those, so unlike [`stats`] there is
/// no foreign id to guard and nothing another account could probe.
pub(super) async fn overview(
    State(state): State<Arc<AppState>>,
    session: Session,
    Query(query): Query<StatsQuery>,
) -> Result<Json<OverviewStats>, AppError> {
    let days = query
        .days
        .unwrap_or(DEFAULT_STATS_DAYS)
        .clamp(1, MAX_STATS_DAYS);
    let range = window(days);

    // The shared x-axis, dense from the window's first day to its last. Built
    // once so a day nobody clicked is a zero in every line rather than a gap one
    // line has and another does not.
    let grid = day_grid(range);
    let days_iso: Vec<String> = grid
        .iter()
        .map(|ts| {
            DateTime::from_timestamp(*ts, 0)
                .map(|dt| dt.to_rfc3339())
                .unwrap_or_default()
        })
        .collect();

    let code_ids =
        repo::list_owned_code_ids(&state.db_pool, session.user_id, MAX_OVERVIEW_FETCH).await?;

    if code_ids.is_empty() {
        return Ok(Json(OverviewStats {
            days: days_iso,
            links: Vec::new(),
        }));
    }

    let mut groups = state
        .clicks
        .overview(&code_ids, range)
        .await
        .map_err(|_| AppError::Internal("failed to read analytics"))?;

    // Busiest first, and only as many lines as the chart can carry.
    groups.sort_by_key(|group| std::cmp::Reverse(group.total));
    groups.truncate(MAX_OVERVIEW_LINKS);

    let mut links = Vec::with_capacity(groups.len());
    for group in groups {
        let id =
            u64::try_from(group.code_id).map_err(|_| AppError::Internal("row id is negative"))?;
        let code = codec::shortcode(id)
            .ok_or(AppError::Internal("row id outside the shortcode domain"))?;

        // Align this line's sparse days onto the shared grid.
        let by_day: HashMap<i64, u64> = group
            .series
            .into_iter()
            .map(|point| (point.at.timestamp(), point.clicks))
            .collect();
        let clicks = grid
            .iter()
            .map(|ts| by_day.get(ts).copied().unwrap_or(0))
            .collect();

        links.push(OverviewLink {
            code,
            total: group.total,
            clicks,
        });
    }

    Ok(Json(OverviewStats {
        days: days_iso,
        links,
    }))
}

/// The window's day-start timestamps, oldest first — the axis every line shares.
///
/// Whole-number and saturating: `rem_euclid` floors to the day with a positive
/// divisor that cannot panic, and the loop is bounded by the 30-day clamp, so at
/// most 31 entries ever land here.
fn day_grid(range: DateRange) -> Vec<i64> {
    let day_start = |ts: i64| ts.saturating_sub(ts.rem_euclid(DAY_SECONDS));
    let start = day_start(range.from.timestamp());
    let end = day_start(range.to.timestamp());

    let mut grid = Vec::new();
    let mut at = start;
    while at <= end {
        grid.push(at);
        at = at.saturating_add(DAY_SECONDS);
    }
    grid
}
