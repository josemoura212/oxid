//! The sink: where a batch of clicks goes, and where a summary comes from.
//!
//! An enum, not `Box<dyn Trait>`: two variants known at compile time, and a
//! trait object with `async fn` would still need `#[async_trait]` and an
//! allocation per call. `Disabled` is the load-test switch — the redirect path
//! must be measurable with analytics contributing nothing, the way
//! `Cache::disabled()` already works.

use clickhouse::{Client, Row};
use serde::Deserialize;
use sqlx::types::chrono::DateTime;

use super::{Breakdown, ClickEvent, DateRange, SCHEMA, SeriesGroup, Slice, Summary, TimePoint};
use crate::configuration::{AnalyticsBackend, AnalyticsSettings};

/// The totals row. Column order, not field names, has to match the SELECT — the
/// driver reads rows positionally (`RowBinary`).
#[derive(Row, Deserialize)]
struct Totals {
    total: u64,
    unique: u64,
}

/// One day's bucket, `at` as a Unix timestamp to avoid `DateTime` deserialization.
#[derive(Row, Deserialize)]
struct Bucket {
    at: u32,
    clicks: u64,
}

/// One grouped bucket for the overview: the same as [`Bucket`] but carrying the
/// code it belongs to, so a single query covers every one of an owner's links.
#[derive(Row, Deserialize)]
struct GroupBucket {
    code_id: i64,
    at: u32,
    clicks: u64,
}

/// One row of the breakdown, labelled with the dimension it came from — the
/// column that lets three rankings share one query.
#[derive(Row, Deserialize)]
struct Ranked {
    dimension: String,
    value: String,
    clicks: u64,
}

#[derive(Clone)]
pub enum ClickSink {
    Disabled,
    // Boxed so the enum is not sized by its heaviest variant: `Disabled` carries
    // nothing, and an unboxed `Client` would make every `ClickSink` — including
    // the disabled one — pay that width.
    ClickHouse(Box<Client>),
}

// `clickhouse::Client` is not `Debug`, and `AppState` derives it.
impl std::fmt::Debug for ClickSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let variant = match self {
            Self::Disabled => "Disabled",
            Self::ClickHouse(_) => "ClickHouse",
        };
        f.debug_tuple("ClickSink").field(&variant).finish()
    }
}

impl ClickSink {
    /// A sink that stores nothing and answers empty. The default, and what the
    /// load-test stages run with.
    pub const fn disabled() -> Self {
        Self::Disabled
    }

    /// Whether a backend is actually connected. The hot path checks this before
    /// building an event, so a disabled sink costs nothing beyond the branch —
    /// and it distinguishes a configured-but-unreachable backend, which
    /// [`connect`] quietly degraded, from one that is genuinely recording.
    pub const fn is_active(&self) -> bool {
        matches!(self, Self::ClickHouse(_))
    }

    /// Connects and ensures the table exists.
    ///
    /// Running the DDL on connect is deliberate: `CREATE TABLE IF NOT EXISTS` is
    /// idempotent, so the schema travels with the code instead of a separate
    /// migration step a fresh environment can forget.
    ///
    /// A failure here **degrades to `Disabled`**, it does not fail the boot. This
    /// is the opposite of the cache, on purpose: a cache that cannot connect
    /// means every read is slow, a problem worth stopping for; analytics touches
    /// no request path, so letting it take the whole API down — the redirect
    /// included — would be a side-channel outranking the product. The warning is
    /// loud enough to notice; the redirect keeps serving.
    pub async fn connect(settings: &AnalyticsSettings) -> Self {
        match settings.backend {
            AnalyticsBackend::Off => Self::Disabled,
            AnalyticsBackend::ClickHouse => {
                let ch = &settings.clickhouse;
                let client = Client::default()
                    .with_url(ch.url())
                    .with_user(&ch.user)
                    .with_password(ch.password())
                    .with_database(&ch.database);

                match client.query(SCHEMA).execute().await {
                    Ok(()) => Self::ClickHouse(Box::new(client)),
                    Err(err) => {
                        tracing::warn!(
                            %err,
                            "analytics backend unreachable; continuing with analytics disabled"
                        );
                        Self::Disabled
                    }
                }
            }
        }
    }

    /// Writes a batch. The worker accumulates events and calls this — never one
    /// event at a time, because ClickHouse turns small frequent inserts into a
    /// storm of parts it then has to merge.
    ///
    /// An empty batch is a no-op, not an empty insert: the worker can wake on a
    /// timer with nothing buffered, and opening an insert for zero rows is pure
    /// round trip.
    pub async fn record(&self, batch: &[ClickEvent]) -> Result<(), SinkError> {
        match self {
            Self::Disabled => Ok(()),
            Self::ClickHouse(client) => {
                if batch.is_empty() {
                    return Ok(());
                }

                let mut insert = client.insert::<ClickEvent>("click_events").await?;
                for event in batch {
                    insert.write(event).await?;
                }
                insert.end().await?;
                Ok(())
            }
        }
    }

    /// Reads the summary for one code over a range.
    ///
    /// Unlike [`record`], the read does not abstract across backends — the query
    /// is a ClickHouse dialect (`toStartOfDay`, `uniq`), which is exactly why the
    /// signature keeps `summary` separate from `record`.
    ///
    /// Timestamps cross as Unix seconds rather than serialized `DateTime`s: it
    /// sidesteps every ambiguity about how the driver formats a `DateTime` for a
    /// bound parameter, and `fromUnixTimestamp` / `toUnixTimestamp` keep both
    /// ends in the same unit.
    pub async fn summary(&self, code_id: i64, range: DateRange) -> Result<Summary, SinkError> {
        match self {
            Self::Disabled => Ok(Summary::empty()),
            Self::ClickHouse(client) => {
                let from = range.from.timestamp();
                let to = range.to.timestamp();

                let totals = client
                    .query(
                        "SELECT count() AS total, uniq(visitor_hash) AS unique \
                         FROM click_events \
                         WHERE code_id = ? \
                           AND created_at >= fromUnixTimestamp(?) \
                           AND created_at <  fromUnixTimestamp(?)",
                    )
                    .bind(code_id)
                    .bind(from)
                    .bind(to)
                    .fetch_one::<Totals>()
                    .await?;

                // Daily buckets, matching the dashboard's day axis. Hourly is a
                // later refinement gated on the selected range.
                let buckets = client
                    .query(
                        "SELECT toUnixTimestamp(toStartOfDay(created_at)) AS at, count() AS clicks \
                         FROM click_events \
                         WHERE code_id = ? \
                           AND created_at >= fromUnixTimestamp(?) \
                           AND created_at <  fromUnixTimestamp(?) \
                         GROUP BY at ORDER BY at",
                    )
                    .bind(code_id)
                    .bind(from)
                    .bind(to)
                    .fetch_all::<Bucket>()
                    .await?;

                let series = buckets
                    .into_iter()
                    .filter_map(|b| {
                        DateTime::from_timestamp(i64::from(b.at), 0).map(|at| TimePoint {
                            at,
                            clicks: b.clicks,
                        })
                    })
                    .collect();

                Ok(Summary {
                    total: totals.total,
                    unique: totals.unique,
                    series,
                })
            }
        }
    }

    /// The ranked dimensions for one code: countries, devices, referrers.
    ///
    /// One round trip for all three, not three. The `UNION ALL` labels each row
    /// with the dimension it came from, and `LIMIT n BY dimension` — a ClickHouse
    /// extension, not standard SQL — keeps the top `n` *within each group* rather
    /// than the top `n` overall. Without it, one popular country would fill the
    /// whole result and the device list would come back empty.
    ///
    /// Bots are excluded from the lists and counted on their own. Empty values are
    /// dropped too: a click with no `Referer` is the normal case, not a referrer
    /// called "".
    pub async fn breakdown(
        &self,
        code_id: i64,
        range: DateRange,
        limit: u8,
    ) -> Result<Breakdown, SinkError> {
        match self {
            Self::Disabled => Ok(Breakdown::default()),
            Self::ClickHouse(client) => {
                let from = range.from.timestamp();
                let to = range.to.timestamp();

                let bots = client
                    .query(
                        "SELECT count() FROM click_events \
                         WHERE code_id = ? \
                           AND created_at >= fromUnixTimestamp(?) \
                           AND created_at <  fromUnixTimestamp(?) \
                           AND is_bot = 1",
                    )
                    .bind(code_id)
                    .bind(from)
                    .bind(to)
                    .fetch_one::<u64>()
                    .await?;

                let rows = client
                    .query(
                        "SELECT dimension, value, clicks FROM ( \
                             SELECT 'country' AS dimension, country AS value, count() AS clicks \
                             FROM click_events \
                             WHERE code_id = ? AND created_at >= fromUnixTimestamp(?) \
                               AND created_at < fromUnixTimestamp(?) AND is_bot = 0 AND country != '' \
                             GROUP BY value \
                             UNION ALL \
                             SELECT 'device' AS dimension, device AS value, count() AS clicks \
                             FROM click_events \
                             WHERE code_id = ? AND created_at >= fromUnixTimestamp(?) \
                               AND created_at < fromUnixTimestamp(?) AND is_bot = 0 AND device != '' \
                             GROUP BY value \
                             UNION ALL \
                             SELECT 'referer' AS dimension, referer_host AS value, count() AS clicks \
                             FROM click_events \
                             WHERE code_id = ? AND created_at >= fromUnixTimestamp(?) \
                               AND created_at < fromUnixTimestamp(?) AND is_bot = 0 AND referer_host != '' \
                             GROUP BY value \
                         ) ORDER BY dimension, clicks DESC, value LIMIT ? BY dimension",
                    )
                    .bind(code_id)
                    .bind(from)
                    .bind(to)
                    .bind(code_id)
                    .bind(from)
                    .bind(to)
                    .bind(code_id)
                    .bind(from)
                    .bind(to)
                    .bind(limit)
                    .fetch_all::<Ranked>()
                    .await?;

                let mut breakdown = Breakdown {
                    bots,
                    ..Breakdown::default()
                };

                for row in rows {
                    let slice = Slice {
                        value: row.value,
                        clicks: row.clicks,
                    };

                    match row.dimension.as_str() {
                        "country" => breakdown.countries.push(slice),
                        "device" => breakdown.devices.push(slice),
                        "referer" => breakdown.referrers.push(slice),
                        // The query is the only producer of this column, so an
                        // unknown label means the SELECT and this match drifted
                        // apart. Dropping the row keeps the dashboard honest
                        // rather than filing it under the wrong heading.
                        _ => {}
                    }
                }

                Ok(breakdown)
            }
        }
    }

    /// Daily buckets for many codes at once, one query rather than one per link.
    ///
    /// The rows come back sorted by `(code_id, at)`, so folding them into groups
    /// is a single pass with no map. Each group's sparse series and its window
    /// total go up to the handler, which aligns them to a shared day axis — the
    /// query stays a plain group-by and the presentation concern stays out of it.
    ///
    /// `IN ?` binds the slice as a ClickHouse array. An empty slice never reaches
    /// the query: it would render `IN []` and match nothing anyway, so the caller
    /// is short-circuited to an empty result.
    pub async fn overview(
        &self,
        code_ids: &[i64],
        range: DateRange,
    ) -> Result<Vec<SeriesGroup>, SinkError> {
        match self {
            Self::Disabled => Ok(Vec::new()),
            Self::ClickHouse(_) if code_ids.is_empty() => Ok(Vec::new()),
            Self::ClickHouse(client) => {
                let from = range.from.timestamp();
                let to = range.to.timestamp();

                let rows = client
                    .query(
                        "SELECT code_id, toUnixTimestamp(toStartOfDay(created_at)) AS at, count() AS clicks \
                         FROM click_events \
                         WHERE code_id IN ? \
                           AND created_at >= fromUnixTimestamp(?) \
                           AND created_at <  fromUnixTimestamp(?) \
                         GROUP BY code_id, at ORDER BY code_id, at",
                    )
                    .bind(code_ids)
                    .bind(from)
                    .bind(to)
                    .fetch_all::<GroupBucket>()
                    .await?;

                let mut groups: Vec<SeriesGroup> = Vec::new();
                for row in rows {
                    let Some(at) = DateTime::from_timestamp(i64::from(row.at), 0) else {
                        continue;
                    };
                    let point = TimePoint {
                        at,
                        clicks: row.clicks,
                    };

                    match groups.last_mut() {
                        Some(group) if group.code_id == row.code_id => {
                            group.total = group.total.saturating_add(row.clicks);
                            group.series.push(point);
                        }
                        _ => groups.push(SeriesGroup {
                            code_id: row.code_id,
                            total: row.clicks,
                            series: vec![point],
                        }),
                    }
                }

                Ok(groups)
            }
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SinkError {
    #[error(transparent)]
    ClickHouse(#[from] clickhouse::error::Error),
}
