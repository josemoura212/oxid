//! Runs against a real ClickHouse (the compose service on 8123). Each test uses
//! a `code_id` unique to this process, so reruns against the persistent table do
//! not accumulate.
#![allow(clippy::unwrap_used)]

use oxid::{
    analytics::{ClickEvent, ClickSink, DateRange},
    configuration::{AnalyticsBackend, AnalyticsSettings, ClickHouseSettings},
};
use sqlx::types::chrono::{DateTime, TimeZone, Utc};

const DEFAULT_HTTP: &str = "127.0.0.1:8123";

fn settings() -> AnalyticsSettings {
    let hostport =
        std::env::var("OXID_TEST_CLICKHOUSE").unwrap_or_else(|_| DEFAULT_HTTP.to_owned());
    let (host, port) = hostport.rsplit_once(':').unwrap();

    AnalyticsSettings {
        backend: AnalyticsBackend::ClickHouse,
        clickhouse: ClickHouseSettings {
            host: host.to_owned(),
            port: port.parse().unwrap(),
            user: "oxid".to_owned(),
            password: "oxid".into(),
            database: "oxid".to_owned(),
        },
    }
}

/// Connects for real. `connect` degrades to `Disabled` when ClickHouse is
/// unreachable rather than erroring, so this asserts the sink is actually active
/// — otherwise a missing ClickHouse would turn these tests into silent no-ops
/// that pass against zero rows.
async fn sink() -> ClickSink {
    let sink = ClickSink::connect(&settings()).await;
    assert!(
        sink.is_active(),
        "ClickHouse is not reachable; these tests need the compose service on 8123"
    );
    sink
}

fn at(y: i32, m: u32, d: u32, h: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(y, m, d, h, 0, 0).unwrap()
}

/// A code id unique to this test process, so a rerun against the same persistent
/// ClickHouse does not accumulate rows for a fixed id. `n` distinguishes tests
/// within a run; the process id distinguishes runs. CI starts with a clean
/// server, so it does not need this — local reruns do.
fn code_id(n: i64) -> i64 {
    9_000_000_000 + i64::from(std::process::id()) * 10 + n
}

fn event(code_id: i64, when: DateTime<Utc>, visitor: u64) -> ClickEvent {
    ClickEvent {
        created_at: when,
        code_id,
        country: "BR".to_owned(),
        device: "desktop".to_owned(),
        os: "linux".to_owned(),
        browser: "firefox".to_owned(),
        referer_host: "example.com".to_owned(),
        lang: "pt".to_owned(),
        is_bot: 0,
        visitor_hash: visitor,
    }
}

#[tokio::test]
async fn disabled_records_nothing_and_summarizes_empty() {
    let sink = ClickSink::disabled();

    // A no-op, and an empty answer rather than an error.
    sink.record(&[event(1, at(2026, 7, 1, 12), 1)])
        .await
        .unwrap();

    let range = DateRange {
        from: at(2026, 7, 1, 0),
        to: at(2026, 8, 1, 0),
    };
    let summary = sink.summary(1, range).await.unwrap();

    assert_eq!(summary.total, 0);
    assert_eq!(summary.unique, 0);
    assert!(summary.series.is_empty());
}

#[tokio::test]
async fn recorded_clicks_come_back_in_the_summary() {
    let sink = sink().await;
    let code_id = code_id(1);

    // Three clicks across two days; two share a visitor, so unique is 2.
    sink.record(&[
        event(code_id, at(2026, 7, 10, 9), 111),
        event(code_id, at(2026, 7, 10, 15), 111),
        event(code_id, at(2026, 7, 11, 9), 222),
    ])
    .await
    .unwrap();

    // ClickHouse acknowledges the insert before the part is fully merged; a
    // brief settle keeps the read from racing the write.
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    let range = DateRange {
        from: at(2026, 7, 1, 0),
        to: at(2026, 8, 1, 0),
    };
    let summary = sink.summary(code_id, range).await.unwrap();

    assert_eq!(summary.total, 3, "three clicks recorded");
    assert_eq!(summary.unique, 2, "two distinct visitors");

    // Two daily buckets, in order, summing to the total.
    assert_eq!(summary.series.len(), 2);
    assert_eq!(summary.series[0].clicks, 2);
    assert_eq!(summary.series[1].clicks, 1);
    assert!(summary.series[0].at < summary.series[1].at);
}

#[tokio::test]
async fn a_code_never_sees_another_codes_clicks() {
    let sink = sink().await;

    let code_a = code_id(2);
    let code_b = code_id(3);

    sink.record(&[event(code_a, at(2026, 7, 5, 12), 1)])
        .await
        .unwrap();
    sink.record(&[event(code_b, at(2026, 7, 5, 12), 1)])
        .await
        .unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    let range = DateRange {
        from: at(2026, 7, 1, 0),
        to: at(2026, 8, 1, 0),
    };
    let summary = sink.summary(code_a, range).await.unwrap();

    assert_eq!(summary.total, 1, "must not count the other code's click");
}
