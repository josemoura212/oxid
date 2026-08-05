//! Runs against a real ClickHouse (the compose service on 8123). Each test uses
//! a `code_id` unique to this process, so reruns against the persistent table do
//! not accumulate.
#![allow(clippy::unwrap_used)]

use oxid::{
    analytics::{ClickEvent, ClickSink, DateRange, Summary},
    configuration::{AnalyticsBackend, AnalyticsSettings, ClickHouseSettings},
};
use sqlx::types::chrono::{DateTime, Utc};

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

/// A timestamp `days` before now, at a fixed hour.
///
/// Relative, not absolute, and that is the whole point. These tests used fixed
/// dates in July 2026, which worked until the calendar caught up with them: the
/// table carries `TTL created_at + INTERVAL 30 DAY`, so on 2026-08-05 the event
/// dated 2026-07-05 turned 31 days old and ClickHouse deleted it during a merge.
/// The test then read zero and reported it as a leak that had not happened.
///
/// It failed one test that day and would have taken the next one the day after,
/// in the order the dates were written — a slow fuse that reads as flakiness
/// because the first symptom is intermittent, appearing only once the TTL merge
/// has actually run on that part.
///
/// Anchoring to `now` removes the calendar from the test. The hour is fixed so a
/// test that groups by day still gets stable, predictable buckets.
fn days_ago(days: i64, hour: u32) -> DateTime<Utc> {
    // Seconds rather than a calendar subtraction: the arithmetic lint is denied
    // in tests too, and this keeps the whole thing on `timestamp`, which is what
    // the column stores anyway.
    let seconds = days.saturating_mul(86_400);
    let day = DateTime::from_timestamp(Utc::now().timestamp().saturating_sub(seconds), 0)
        .unwrap()
        .date_naive();

    day.and_hms_opt(hour, 0, 0).unwrap().and_utc()
}

/// The window every test reads through: comfortably inside the 30-day TTL, and
/// wide enough that nothing written by `days_ago` falls outside it.
fn window() -> DateRange {
    DateRange {
        from: days_ago(20, 0),
        to: days_ago(-1, 0),
    }
}

/// A code id unique to this test process, so a rerun against the same persistent
/// ClickHouse does not accumulate rows for a fixed id. `n` distinguishes tests
/// within a run; the process id distinguishes runs. CI starts with a clean
/// server, so it does not need this — local reruns do.
fn code_id(n: i64) -> i64 {
    // Saturating because the arithmetic lint is denied even in tests; overflow is
    // impossible here, but the checked form is what the lint accepts.
    9_000_000_000_i64
        .saturating_add(i64::from(std::process::id()).saturating_mul(10))
        .saturating_add(n)
}

/// An enriched event, for the breakdown. `bot` is what decides whether it counts
/// as a person, which is the whole point of the column.
fn rich(
    code_id: i64,
    when: DateTime<Utc>,
    visitor: u64,
    country: &str,
    device: &str,
    referer: &str,
    bot: u8,
) -> ClickEvent {
    ClickEvent {
        country: country.to_owned(),
        device: device.to_owned(),
        referer_host: referer.to_owned(),
        is_bot: bot,
        ..event(code_id, when, visitor)
    }
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

/// Waits until a code has at least `expected` clicks visible, then returns what
/// the summary says.
///
/// This replaces the fixed sleeps this file used to carry. ClickHouse
/// acknowledges an insert before the rows are necessarily queryable, so the tests
/// guessed an interval and hoped — 500 ms, chosen because it worked on a laptop.
/// Under CI, where coverage instrumentation slows everything down, the guess was
/// wrong often enough to fail the build on code it was not testing.
///
/// Polling moves the determinism from the timing to the outcome: a fast machine
/// returns on the first read, a slow one takes a few more, and only a genuine
/// failure to record reaches the deadline. Returning the summary rather than
/// asserting inside keeps the real assertion — and its message — in the test that
/// owns it.
///
/// `>=` rather than `==` on purpose: a test asserting a code sees exactly its own
/// clicks must still observe a leak from another code, and waiting for equality
/// would spin until the deadline and then report a timeout instead of the bug.
async fn await_clicks(sink: &ClickSink, code_id: i64, range: DateRange, expected: u64) -> Summary {
    // Generous, because it is only ever reached when something is actually
    // broken — a healthy read returns on the first or second attempt. Checked,
    // because the arithmetic lint is denied in tests too, and `Instant` addition
    // can overflow in principle.
    let started = std::time::Instant::now();
    let budget = std::time::Duration::from_secs(20);

    loop {
        let summary = sink.summary(code_id, range).await.unwrap();

        if summary.total >= expected || started.elapsed() >= budget {
            return summary;
        }

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

#[tokio::test]
async fn disabled_records_nothing_and_summarizes_empty() {
    let sink = ClickSink::disabled();

    // A no-op, and an empty answer rather than an error.
    sink.record(&[event(1, days_ago(5, 12), 1)]).await.unwrap();

    let range = window();
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
        event(code_id, days_ago(7, 9), 111),
        event(code_id, days_ago(7, 15), 111),
        event(code_id, days_ago(6, 9), 222),
    ])
    .await
    .unwrap();

    let range = window();
    let summary = await_clicks(&sink, code_id, range, 3).await;

    assert_eq!(summary.total, 3, "three clicks recorded");
    assert_eq!(summary.unique, 2, "two distinct visitors");

    // Two daily buckets, in order, summing to the total.
    assert_eq!(summary.series.len(), 2);
    assert_eq!(summary.series[0].clicks, 2);
    assert_eq!(summary.series[1].clicks, 1);
    assert!(summary.series[0].at < summary.series[1].at);
}

#[tokio::test]
async fn overview_groups_every_codes_clicks_in_one_pass() {
    let sink = sink().await;
    let code_a = code_id(4);
    let code_b = code_id(5);

    // code_a: two clicks the same day. code_b: one, a day later.
    sink.record(&[
        event(code_a, days_ago(7, 9), 1),
        event(code_a, days_ago(7, 15), 2),
        event(code_b, days_ago(6, 9), 3),
    ])
    .await
    .unwrap();

    let range = window();
    // Both codes were written in one batch, so waiting on either is waiting on
    // both — and the overview reads the same rows the summary just saw.
    await_clicks(&sink, code_a, range, 2).await;

    let groups = sink.overview(&[code_a, code_b], range).await.unwrap();

    assert_eq!(groups.len(), 2, "one group per code");

    let a = groups.iter().find(|g| g.code_id == code_a).unwrap();
    let b = groups.iter().find(|g| g.code_id == code_b).unwrap();

    assert_eq!(a.total, 2);
    assert_eq!(a.series.len(), 1, "both clicks fall on one day");
    assert_eq!(a.series[0].clicks, 2);
    assert_eq!(b.total, 1);
}

/// An empty id list short-circuits: `IN []` would match nothing anyway, and the
/// sink returns before touching ClickHouse.
#[tokio::test]
async fn overview_of_no_codes_is_empty() {
    let sink = sink().await;

    let range = window();
    let groups = sink.overview(&[], range).await.unwrap();

    assert!(groups.is_empty());
}

#[tokio::test]
async fn a_code_never_sees_another_codes_clicks() {
    let sink = sink().await;

    let code_a = code_id(2);
    let code_b = code_id(3);

    sink.record(&[event(code_a, days_ago(8, 12), 1)])
        .await
        .unwrap();
    sink.record(&[event(code_b, days_ago(8, 12), 1)])
        .await
        .unwrap();

    let range = window();
    let summary = await_clicks(&sink, code_a, range, 1).await;

    assert_eq!(summary.total, 1, "must not count the other code's click");
}

// --- the batching worker ---
//
// The worker is the one piece that can lose clicks without anyone noticing: it
// buffers, and anything still buffered when the process ends is gone unless it
// flushes. These pin the two moments a batch gets written that are not "the batch
// filled up" — the timer, and shutdown.

/// A trickle of clicks must not sit unwritten. One event is far short of a full
/// batch, so only the flush interval can move it.
#[tokio::test]
async fn a_partial_batch_is_flushed_on_the_timer() {
    let code_id = code_id(6);
    let tx = oxid::analytics::spawn(sink().await);

    tx.emit(event(code_id, days_ago(5, 10), 7));

    // The sender is deliberately still alive, so nothing but the flush interval
    // can move this event. The poll below covers that interval — it is the thing
    // under test, not an assumption about how long the machine takes.
    let range = window();
    let summary = await_clicks(&sink().await, code_id, range, 1).await;

    assert_eq!(
        summary.total, 1,
        "the timer never flushed the partial batch"
    );
}

/// Dropping the last sender is how shutdown reaches the worker. What is buffered
/// at that moment has to be written, not discarded.
#[tokio::test]
async fn a_buffered_batch_is_flushed_when_the_senders_go_away() {
    let code_id = code_id(7);
    let tx = oxid::analytics::spawn(sink().await);

    tx.emit(event(code_id, days_ago(5, 10), 8));
    tx.emit(event(code_id, days_ago(5, 11), 9));

    // The channel is closed by this, which is what makes `recv` answer `None`.
    drop(tx);

    let range = window();
    let summary = await_clicks(&sink().await, code_id, range, 2).await;

    assert_eq!(summary.total, 2, "shutdown discarded the buffered clicks");
}

/// With no backend there is no worker and no channel, and emitting is a branch and
/// nothing more — the property that lets the load-test stages run the redirect with
/// analytics contributing zero.
#[tokio::test]
async fn a_disabled_sink_spawns_no_worker() {
    let tx = oxid::analytics::spawn(ClickSink::disabled());

    // Would panic or block if it were feeding a real channel with no reader.
    tx.emit(event(1, days_ago(5, 12), 1));
    tx.emit(event(1, days_ago(5, 12), 1));

    let summary = ClickSink::disabled().summary(1, window()).await.unwrap();

    assert_eq!(summary.total, 0);
}

// --- the ranked breakdown ---

/// Three rankings from one query, and the rule that makes them mean something:
/// bots are counted but never listed. For a shortener that is not a detail — a
/// link pasted into a group chat is fetched by the platform before anyone opens
/// it, so "top countries" would otherwise rank the crawlers' exit nodes.
#[tokio::test]
async fn the_breakdown_ranks_people_and_counts_bots_apart() {
    let sink = sink().await;
    let code_id = code_id(8);
    let when = days_ago(4, 10);

    sink.record(&[
        rich(code_id, when, 1, "BR", "mobile", "www.google.com", 0),
        rich(code_id, when, 2, "BR", "mobile", "www.google.com", 0),
        rich(code_id, when, 3, "PT", "desktop", "x.com", 0),
        // A crawler from a country and device that appear nowhere else, so if it
        // leaked into the lists it would be unmistakable.
        rich(code_id, when, 4, "NL", "bot", "", 1),
        rich(code_id, when, 5, "NL", "bot", "", 1),
    ])
    .await
    .unwrap();

    let range = window();
    // Five rows went in — three people and two crawlers. The breakdown reads the
    // same rows, so seeing them in the summary is seeing them at all.
    await_clicks(&sink, code_id, range, 5).await;

    let breakdown = sink.breakdown(code_id, range, 5).await.unwrap();

    assert_eq!(breakdown.bots, 2, "bots are counted");

    // Ranked by volume, busiest first.
    assert_eq!(breakdown.countries.len(), 2, "NL was a bot, not a visitor");
    assert_eq!(breakdown.countries[0].value, "BR");
    assert_eq!(breakdown.countries[0].clicks, 2);
    assert_eq!(breakdown.countries[1].value, "PT");

    assert_eq!(breakdown.devices[0].value, "mobile");
    assert_eq!(breakdown.devices[0].clicks, 2);
    assert!(
        breakdown.devices.iter().all(|d| d.value != "bot"),
        "a bot is not a device someone browses with"
    );

    // The empty referer of the bot rows must not become a referrer called "".
    assert_eq!(breakdown.referrers.len(), 2);
    assert_eq!(breakdown.referrers[0].value, "www.google.com");
}

/// `LIMIT n BY dimension` is what keeps one busy dimension from swallowing the
/// result. Without it, the six countries below would fill the whole answer and
/// the device list would come back empty.
#[tokio::test]
async fn the_limit_applies_per_dimension_not_overall() {
    let sink = sink().await;
    let code_id = code_id(9);
    let when = days_ago(3, 10);

    let countries = ["BR", "PT", "US", "DE", "FR", "JP"];
    let batch: Vec<_> = countries
        .iter()
        .enumerate()
        .map(|(index, country)| {
            let visitor = u64::try_from(index).unwrap_or(0);
            rich(code_id, when, visitor, country, "desktop", "", 0)
        })
        .collect();

    sink.record(&batch).await.unwrap();

    let range = window();
    await_clicks(&sink, code_id, range, 6).await;

    let breakdown = sink.breakdown(code_id, range, 3).await.unwrap();

    assert_eq!(breakdown.countries.len(), 3, "capped at the limit");
    assert_eq!(
        breakdown.devices.len(),
        1,
        "the device list survived a busier dimension"
    );
    assert_eq!(breakdown.devices[0].clicks, 6);
}

#[tokio::test]
async fn a_disabled_sink_returns_an_empty_breakdown() {
    let breakdown = ClickSink::disabled()
        .breakdown(1, window(), 5)
        .await
        .unwrap();

    assert_eq!(breakdown.bots, 0);
    assert!(breakdown.countries.is_empty());
}
