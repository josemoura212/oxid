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

use super::{ClickEvent, DateRange, SCHEMA, Summary, TimePoint};
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

    /// Connects and ensures the table exists.
    ///
    /// Running the DDL on every boot is deliberate: `CREATE TABLE IF NOT EXISTS`
    /// is idempotent, so the schema travels with the code instead of a separate
    /// migration step that a fresh environment can forget. Failing here fails the
    /// boot — a misconfigured analytics backend should be loud, not a surprise on
    /// the first click.
    pub async fn connect(settings: &AnalyticsSettings) -> Result<Self, SinkError> {
        match settings.backend {
            AnalyticsBackend::Off => Ok(Self::Disabled),
            AnalyticsBackend::ClickHouse => {
                let ch = &settings.clickhouse;
                let client = Client::default()
                    .with_url(ch.url())
                    .with_user(&ch.user)
                    .with_password(ch.password())
                    .with_database(&ch.database);

                client.query(SCHEMA).execute().await?;

                Ok(Self::ClickHouse(Box::new(client)))
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
}

#[derive(Debug, thiserror::Error)]
pub enum SinkError {
    #[error(transparent)]
    ClickHouse(#[from] clickhouse::error::Error),
}
