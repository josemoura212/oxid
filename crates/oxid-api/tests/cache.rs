//! Runs against a real Redis. Set `OXID_TEST_REDIS` to point elsewhere; the
//! default matches the port the local compose publishes.
//!
//! Keys are namespaced per test instead of flushing the database, so the suite
//! can run in parallel against one instance.
#![allow(clippy::unwrap_used)]

use std::time::Duration;

use oxid::{
    cache::{Cache, Cached},
    configuration::CacheSettings,
};
use redis::AsyncCommands;

const DEFAULT_REDIS: &str = "redis://127.0.0.1:6381";

fn settings() -> CacheSettings {
    let url = std::env::var("OXID_TEST_REDIS").unwrap_or_else(|_| DEFAULT_REDIS.to_owned());
    let url = url.strip_prefix("redis://").unwrap_or(&url).to_owned();
    let (host, port) = url.rsplit_once(':').unwrap();

    CacheSettings {
        host: host.to_owned(),
        port: port.parse().unwrap(),
        negative_ttl_seconds: 60,
        connect_timeout_seconds: 2,
    }
}

async fn cache() -> Cache {
    let settings = settings();
    let conn = oxid::cache::connect(&settings).await.unwrap();
    Cache::new(conn, settings.negative_ttl_seconds)
}

/// Talks to Redis directly, to assert on what the cache actually wrote.
async fn raw() -> redis::aio::MultiplexedConnection {
    let settings = settings();
    redis::Client::open(settings.url())
        .unwrap()
        .get_multiplexed_async_connection()
        .await
        .unwrap()
}

async fn clear(code: &str) {
    let mut conn = raw().await;
    let _: () = conn.del(format!("u2:{code}")).await.unwrap();
}

#[tokio::test]
async fn stores_and_reads_back_a_url() {
    let code = "tst0001";
    clear(code).await;

    let cache = cache().await;
    assert_eq!(cache.get(code).await, None, "should start empty");

    cache.set_url(code, "https://example.com/a", false).await;

    assert_eq!(
        cache.get(code).await,
        Some(Cached::Url { long_url: "https://example.com/a".to_owned(), owned: false })
    );
}

/// `None` means "the cache has no opinion"; `Some(Missing)` means "I know there
/// is nothing". Collapsing the two would send every unknown code to Postgres.
#[tokio::test]
async fn absence_and_known_absence_are_different_answers() {
    let code = "tst0002";
    clear(code).await;

    let cache = cache().await;
    assert_eq!(cache.get(code).await, None);

    cache.set_missing(code).await;

    assert_eq!(cache.get(code).await, Some(Cached::Missing));
}

/// The asymmetry that makes caching everything safe: a shortcode is immutable,
/// so a positive entry can never go stale and needs no expiry.
#[tokio::test]
async fn positive_entries_never_expire_and_negative_ones_do() {
    let (positive, negative) = ("tst0003", "tst0004");
    clear(positive).await;
    clear(negative).await;

    let cache = cache().await;
    cache.set_url(positive, "https://example.com/b", false).await;
    cache.set_missing(negative).await;

    let mut conn = raw().await;
    let positive_ttl: i64 = conn.ttl(format!("u2:{positive}")).await.unwrap();
    let negative_ttl: i64 = conn.ttl(format!("u2:{negative}")).await.unwrap();

    assert_eq!(positive_ttl, -1, "positive entry must not expire");
    assert!(
        negative_ttl > 0 && negative_ttl <= 60,
        "negative entry must expire, got {negative_ttl}"
    );
}

/// The race this whole design exists for:
///
/// ```text
/// T1  GET /code   → miss in cache, miss in database
/// T2                POST creates that exact code, writes it to the cache
/// T1  writes "missing"
/// ```
///
/// With a plain `SET` the good value is gone and the code answers 404 until the
/// TTL runs out. `SET NX` makes the late write a no-op.
#[tokio::test]
async fn a_negative_write_never_overwrites_a_positive_one() {
    let code = "tst0005";
    clear(code).await;

    let cache = cache().await;

    cache.set_url(code, "https://example.com/winner", false).await;
    cache.set_missing(code).await;

    assert_eq!(
        cache.get(code).await,
        Some(Cached::Url { long_url: "https://example.com/winner".to_owned(), owned: false }),
        "the negative write clobbered the URL"
    );
}

/// Same race, now with the writes actually interleaved rather than sequenced.
#[tokio::test]
async fn the_positive_survives_concurrent_writes() {
    let code = "tst0006";
    clear(code).await;

    let cache = cache().await;
    let writer = cache.clone();

    let (positive, negative) = tokio::join!(
        async { writer.set_url(code, "https://example.com/winner", false).await },
        async {
            tokio::time::sleep(Duration::from_millis(5)).await;
            cache.set_missing(code).await;
        }
    );
    let ((), ()) = (positive, negative);

    assert_eq!(
        cache.get(code).await,
        Some(Cached::Url {
            long_url: "https://example.com/winner".to_owned(),
            owned: false
        })
    );
}

#[tokio::test]
async fn a_disabled_cache_stores_nothing_and_knows_nothing() {
    let code = "tst0007";
    clear(code).await;

    let cache = Cache::disabled();
    cache.set_url(code, "https://example.com/c", false).await;

    assert_eq!(cache.get(code).await, None);

    // And nothing reached Redis either.
    let mut conn = raw().await;
    let stored: Option<String> = conn.get(format!("u2:{code}")).await.unwrap();
    assert_eq!(stored, None);
}
