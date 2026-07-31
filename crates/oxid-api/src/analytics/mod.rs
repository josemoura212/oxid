//! Click analytics, on ClickHouse.
//!
//! A click event is decoupled from the Postgres side on purpose: it lands here,
//! not in a table with a foreign key to `short_codes`. That is what makes the
//! write cheap enough to sit behind the redirect, and it is also why deleting an
//! account does **not** cascade to its clicks — that becomes an explicit
//! `ALTER TABLE ... DELETE`, recorded in the roadmap rather than assumed.
//!
//! ClickHouse rather than Postgres because the workload is columnar: count and
//! `uniq()` over a time range, grouped by a small set of dimensions. The 30-day
//! TTL keeps it a rolling window instead of the 365-billion-row archive the rest
//! of the system is sized for.
//!
//! This module is the inert foundation — types, the sink enum, and the schema.
//! Nothing writes yet; the batching worker and the hot-path emit come next.

mod enrich;
mod sink;
mod worker;

pub use enrich::{Agent, agent, country, lang, referer_host};
pub use sink::{ClickSink, SinkError};
pub use worker::{ClickTx, spawn};

use clickhouse::Row;
use serde::{Deserialize, Serialize};
use sqlx::types::chrono::{DateTime, Utc};

/// One redirect that was counted.
///
/// The field order and types mirror the ClickHouse columns exactly — the
/// `clickhouse` crate maps a `Row` positionally, so a reordering here is a
/// silent corruption, not a compile error. `LowCardinality(String)` on the
/// server is a plain `String` on this side; `UInt8`/`UInt64` are `u8`/`u64`.
#[derive(Debug, Clone, Row, Serialize, Deserialize)]
pub struct ClickEvent {
    /// Serialized as ClickHouse `DateTime` (second precision), which is all a
    /// click needs — sub-second ordering of two clicks buys nothing here.
    #[serde(with = "clickhouse::serde::chrono::datetime")]
    pub created_at: DateTime<Utc>,

    /// `short_codes.id`, recovered from the public code by the bijection — not
    /// `url_id`. This is the line that makes a click belong to one owner's code
    /// rather than to a URL shared across owners.
    pub code_id: i64,

    // Enrichment. All present in the schema from the start so a partitioned
    // table never needs an `ALTER`; filled in by a later slice, empty until then.
    pub country: String,
    pub device: String,
    pub os: String,
    pub browser: String,
    pub referer_host: String,
    pub lang: String,

    /// `UInt8`, because ClickHouse has no boolean.
    pub is_bot: u8,

    /// `hash(ip + user-agent + daily salt)`, folded to 64 bits. The daily salt
    /// is what stops the same visitor being re-identified across days; 64 bits
    /// is enough for `uniq()`, which is approximate by design.
    pub visitor_hash: u64,
}

/// Half-open time window `[from, to)` for a summary query.
#[derive(Debug, Clone, Copy)]
pub struct DateRange {
    pub from: DateTime<Utc>,
    pub to: DateTime<Utc>,
}

/// One point on the clicks-over-time line — the shape both dashboard screens
/// draw, one series per code on the overview and one code on the detail view.
#[derive(Debug, Clone, Serialize)]
pub struct TimePoint {
    pub at: DateTime<Utc>,
    pub clicks: u64,
}

/// What a dashboard screen needs for a single code over a range.
#[derive(Debug, Clone, Serialize)]
pub struct Summary {
    pub total: u64,
    /// Distinct `visitor_hash` values — `uniq()` on the ClickHouse side.
    pub unique: u64,
    pub series: Vec<TimePoint>,
}

impl Summary {
    /// What the disabled sink returns: a real, empty answer rather than an error,
    /// so a dashboard with analytics off renders zeros instead of a failure.
    pub const fn empty() -> Self {
        Self {
            total: 0,
            unique: 0,
            series: Vec::new(),
        }
    }
}

/// One code's line for the overview.
///
/// Carries its id, the window total that ranks it, and its sparse daily series.
/// Sparse on purpose — the handler densifies against a shared day axis, which is
/// where the alignment belongs, not in the query.
#[derive(Debug, Clone)]
pub struct SeriesGroup {
    pub code_id: i64,
    pub total: u64,
    pub series: Vec<TimePoint>,
}

/// The table, created idempotently on connect.
///
/// `ORDER BY (code_id, created_at)` is the one decision that governs read speed:
/// every dashboard query filters by code and time, so sorting the data that way
/// is what lets a query read contiguous blocks. `PARTITION BY toYYYYMM` is
/// monthly and no finer — over-partitioning multiplies the parts ClickHouse has
/// to merge. `TTL ... 30 DAY` makes retention a property of the table instead of
/// a cron job.
const SCHEMA: &str = "\
CREATE TABLE IF NOT EXISTS click_events (
    created_at   DateTime,
    code_id      Int64,
    country      LowCardinality(String) DEFAULT '',
    device       LowCardinality(String) DEFAULT '',
    os           LowCardinality(String) DEFAULT '',
    browser      LowCardinality(String) DEFAULT '',
    referer_host String                 DEFAULT '',
    lang         LowCardinality(String) DEFAULT '',
    is_bot       UInt8                  DEFAULT 0,
    visitor_hash UInt64                 DEFAULT 0
)
ENGINE = MergeTree
PARTITION BY toYYYYMM(created_at)
ORDER BY (code_id, created_at)
TTL created_at + INTERVAL 30 DAY
SETTINGS index_granularity = 8192";
