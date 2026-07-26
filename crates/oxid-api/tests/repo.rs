//! `#[sqlx::test]` creates a temporary database per test, runs the migrations
//! and drops it at the end. The path is relative to this crate — `migrations/`
//! lives at the workspace root.

use std::collections::HashSet;

use oxid::repo;
use sqlx::PgPool;
use tokio::task::JoinSet;

#[sqlx::test(migrations = "../../migrations")]
async fn same_url_returns_the_same_id(pool: PgPool) {
    let url = "https://example.com/idempotency";

    let first = repo::insert_url(&pool, url).await.unwrap();
    let second = repo::insert_url(&pool, url).await.unwrap();

    assert_eq!(first, second);
}

/// The test that closes the stage. A sequential one would pass even with
/// `SELECT`-then-`INSERT` or with the CTE version — only the race tells them apart.
#[sqlx::test(migrations = "../../migrations")]
async fn concurrent_inserts_return_the_same_id(pool: PgPool) {
    let url = "https://example.com/race";
    let mut tasks = JoinSet::new();

    for _ in 0..16 {
        let pool = pool.clone();
        tasks.spawn(async move { repo::insert_url(&pool, url).await });
    }

    let ids: HashSet<i64> = tasks
        .join_all()
        .await
        .into_iter()
        .map(|result| result.unwrap())
        .collect();

    assert_eq!(
        ids.len(),
        1,
        "the same URL produced more than one id: {ids:?}"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn different_urls_get_different_ids(pool: PgPool) {
    let a = repo::insert_url(&pool, "https://example.com/a")
        .await
        .unwrap();
    let b = repo::insert_url(&pool, "https://example.com/b")
        .await
        .unwrap();

    assert_ne!(a, b);
}

#[sqlx::test(migrations = "../../migrations")]
async fn get_url_returns_the_inserted_url(pool: PgPool) {
    let url = "https://example.com/roundtrip";
    let id = repo::insert_url(&pool, url).await.unwrap();

    assert_eq!(
        repo::get_url(&pool, id).await.unwrap(),
        Some(url.to_owned())
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn get_url_of_unknown_id_returns_none(pool: PgPool) {
    assert_eq!(repo::get_url(&pool, 999_999).await.unwrap(), None);
}

/// URLs with a backslash broke when the hash used `long_url::bytea`.
#[sqlx::test(migrations = "../../migrations")]
async fn url_with_backslash_is_accepted(pool: PgPool) {
    let url = r"https://example.com/path\query\101";

    let first = repo::insert_url(&pool, url).await.unwrap();
    let second = repo::insert_url(&pool, url).await.unwrap();

    assert_eq!(first, second);
    assert_eq!(
        repo::get_url(&pool, first).await.unwrap(),
        Some(url.to_owned())
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn url_above_the_limit_is_rejected(pool: PgPool) {
    let too_long = format!("https://example.com/{}", "a".repeat(2100));

    assert!(repo::insert_url(&pool, &too_long).await.is_err());
}

#[sqlx::test(migrations = "../../migrations")]
async fn url_at_the_limit_is_accepted(pool: PgPool) {
    let prefix = "https://example.com/";
    let padding = "a".repeat(2048_usize.saturating_sub(prefix.len()));
    let at_limit = format!("{prefix}{padding}");

    assert_eq!(at_limit.len(), 2048);
    assert!(repo::insert_url(&pool, &at_limit).await.is_ok());
}
