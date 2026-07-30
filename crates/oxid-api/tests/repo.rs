//! `#[sqlx::test]` creates a temporary database per test, runs the migrations
//! and drops it at the end. The path is relative to this crate — `migrations/`
//! lives at the workspace root.

use std::collections::HashSet;

use oxid::repo;
use oxid_shared::MAX_URL_LEN;
use sqlx::PgPool;
use tokio::task::JoinSet;

// The helpers propagate rather than unwrap: `allow-unwrap-in-tests` only covers
// functions the lint recognises as tests, and a plain `async fn` in this file is
// not one of them.

/// Shortens as nobody in particular, the way an anonymous request does.
async fn shorten_anon(pool: &PgPool, url: &str) -> Result<i64, sqlx::Error> {
    let url_id = repo::upsert_url(pool, url).await?;
    repo::upsert_code(pool, url_id, None).await
}

/// Deliberately not shaped like a PHC string. These tests never verify a
/// password, so the column only has to hold something — and anything starting
/// with `$argon2id$` trips secret scanners on every commit for no reason.
const STORED_HASH_PLACEHOLDER: &str = "not-a-real-hash";

async fn make_user(pool: &PgPool, email: &str) -> Result<Option<i64>, sqlx::Error> {
    repo::create_user(pool, email, STORED_HASH_PLACEHOLDER).await
}

#[sqlx::test(migrations = "../../migrations")]
async fn same_url_returns_the_same_id(pool: PgPool) {
    let url = "https://example.com/idempotency";

    let first = repo::upsert_url(&pool, url).await.unwrap();
    let second = repo::upsert_url(&pool, url).await.unwrap();

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
        tasks.spawn(async move { repo::upsert_url(&pool, url).await });
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
    let a = repo::upsert_url(&pool, "https://example.com/a")
        .await
        .unwrap();
    let b = repo::upsert_url(&pool, "https://example.com/b")
        .await
        .unwrap();

    assert_ne!(a, b);
}

#[sqlx::test(migrations = "../../migrations")]
async fn resolve_returns_the_shortened_url(pool: PgPool) {
    let url = "https://example.com/roundtrip";
    let code_id = shorten_anon(&pool, url).await.unwrap();

    let resolved = repo::resolve_code(&pool, code_id).await.unwrap().unwrap();
    assert_eq!(resolved.long_url, url);
    assert!(!resolved.owned, "an anonymous code must resolve as unowned");
}

#[sqlx::test(migrations = "../../migrations")]
async fn resolve_of_unknown_id_returns_none(pool: PgPool) {
    assert!(repo::resolve_code(&pool, 999_999).await.unwrap().is_none());
}

/// URLs with a backslash broke when the hash used `long_url::bytea`.
#[sqlx::test(migrations = "../../migrations")]
async fn url_with_backslash_is_accepted(pool: PgPool) {
    let url = r"https://example.com/path\query\101";

    let first = shorten_anon(&pool, url).await.unwrap();
    let second = shorten_anon(&pool, url).await.unwrap();

    assert_eq!(first, second);
    assert_eq!(
        repo::resolve_code(&pool, first)
            .await
            .unwrap()
            .unwrap()
            .long_url,
        url
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn url_above_the_limit_is_rejected(pool: PgPool) {
    let too_long = format!("https://example.com/{}", "a".repeat(MAX_URL_LEN));

    assert!(repo::upsert_url(&pool, &too_long).await.is_err());
}

/// Guards the CHECK itself, at the boundary, going straight to the repository
/// rather than through the handler — the handler now rejects earlier, and this
/// has to keep proving the database would refuse on its own.
#[sqlx::test(migrations = "../../migrations")]
async fn url_at_the_limit_is_accepted(pool: PgPool) {
    let prefix = "https://example.com/";
    let padding = "a".repeat(MAX_URL_LEN.saturating_sub(prefix.len()));
    let at_limit = format!("{prefix}{padding}");

    assert_eq!(at_limit.len(), MAX_URL_LEN);
    assert!(repo::upsert_url(&pool, &at_limit).await.is_ok());
}

// --- ownership ---

/// The stage's acceptance criterion.
///
/// Two accounts shortening one URL share the row that stores it — the dedupe
/// that makes the row count plausible survives — but get distinct codes, which
/// is what lets a click be attributed to one owner rather than to both.
#[sqlx::test(migrations = "../../migrations")]
async fn two_owners_share_the_url_row_but_not_the_code(pool: PgPool) {
    let url = "https://example.com/shared";
    let ana = make_user(&pool, "ana@example.com").await.unwrap().unwrap();
    let bruno = make_user(&pool, "bruno@example.com")
        .await
        .unwrap()
        .unwrap();

    let url_id = repo::upsert_url(&pool, url).await.unwrap();
    let ana_code = repo::upsert_code(&pool, url_id, Some(ana)).await.unwrap();
    let bruno_code = repo::upsert_code(&pool, url_id, Some(bruno)).await.unwrap();

    assert_ne!(ana_code, bruno_code, "owners must not share a code");

    // Both still resolve to the same destination, from one stored URL — and both
    // read as owned.
    let ana_resolved = repo::resolve_code(&pool, ana_code).await.unwrap().unwrap();
    let bruno_resolved = repo::resolve_code(&pool, bruno_code)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(ana_resolved.long_url, bruno_resolved.long_url);
    assert!(ana_resolved.owned && bruno_resolved.owned);

    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM urls")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(rows, 1, "the long URL must be stored once");
}

#[sqlx::test(migrations = "../../migrations")]
async fn the_same_owner_twice_gets_the_same_code(pool: PgPool) {
    let ana = make_user(&pool, "ana@example.com").await.unwrap().unwrap();
    let url_id = repo::upsert_url(&pool, "https://example.com/mine")
        .await
        .unwrap();

    let first = repo::upsert_code(&pool, url_id, Some(ana)).await.unwrap();
    let second = repo::upsert_code(&pool, url_id, Some(ana)).await.unwrap();

    assert_eq!(first, second);
}

/// Guards `UNIQUE NULLS NOT DISTINCT`.
///
/// A plain `UNIQUE (owner_id, url_id)` passes every other test in this file and
/// fails only this one: SQL treats `NULL` as never equal to `NULL`, so each
/// anonymous claim would insert a fresh row and hand out a new code. Nothing
/// errors — duplicates just accumulate, and the idempotence that works today
/// disappears without a symptom.
#[sqlx::test(migrations = "../../migrations")]
async fn anonymous_shortening_stays_idempotent(pool: PgPool) {
    let url = "https://example.com/anonymous";

    let first = shorten_anon(&pool, url).await.unwrap();
    let second = shorten_anon(&pool, url).await.unwrap();

    assert_eq!(first, second, "anonymous codes must not duplicate");
}

#[sqlx::test(migrations = "../../migrations")]
async fn concurrent_anonymous_claims_collapse_to_one_code(pool: PgPool) {
    let url = "https://example.com/anon-race";
    let url_id = repo::upsert_url(&pool, url).await.unwrap();
    let mut tasks = JoinSet::new();

    for _ in 0..16 {
        let pool = pool.clone();
        tasks.spawn(async move { repo::upsert_code(&pool, url_id, None).await });
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
        "anonymous race produced several codes: {ids:?}"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn an_owner_never_sees_another_owners_links(pool: PgPool) {
    let ana = make_user(&pool, "ana@example.com").await.unwrap().unwrap();
    let bruno = make_user(&pool, "bruno@example.com")
        .await
        .unwrap()
        .unwrap();

    let shared = repo::upsert_url(&pool, "https://example.com/shared")
        .await
        .unwrap();
    repo::upsert_code(&pool, shared, Some(ana)).await.unwrap();
    repo::upsert_code(&pool, shared, Some(bruno)).await.unwrap();

    let only_bruno = repo::upsert_url(&pool, "https://example.com/bruno")
        .await
        .unwrap();
    repo::upsert_code(&pool, only_bruno, Some(bruno))
        .await
        .unwrap();

    // The anonymous code for the same URL belongs to nobody's list.
    repo::upsert_code(&pool, shared, None).await.unwrap();

    let listed = repo::list_owned(&pool, ana, None, 50).await.unwrap();
    assert_eq!(listed.len(), 1);

    let listed = repo::list_owned(&pool, bruno, None, 50).await.unwrap();
    assert_eq!(listed.len(), 2);
}

#[sqlx::test(migrations = "../../migrations")]
async fn duplicate_email_is_refused(pool: PgPool) {
    make_user(&pool, "ana@example.com").await.unwrap().unwrap();

    let again = repo::create_user(&pool, "ana@example.com", STORED_HASH_PLACEHOLDER)
        .await
        .unwrap();

    assert!(again.is_none());
}

/// `citext` does the folding, so the application never has to remember to
/// lowercase — and one forgotten call cannot become a duplicate account.
#[sqlx::test(migrations = "../../migrations")]
async fn email_uniqueness_ignores_case(pool: PgPool) {
    make_user(&pool, "ana@example.com").await.unwrap().unwrap();

    let again = repo::create_user(&pool, "ANA@Example.COM", STORED_HASH_PLACEHOLDER)
        .await
        .unwrap();

    assert!(again.is_none());
    assert!(
        repo::find_credentials(&pool, "Ana@EXAMPLE.com")
            .await
            .unwrap()
            .is_some()
    );
}

/// Keyset pagination must return every row exactly once across pages.
#[sqlx::test(migrations = "../../migrations")]
async fn pagination_walks_every_row_once(pool: PgPool) {
    let ana = make_user(&pool, "ana@example.com").await.unwrap().unwrap();

    for n in 0..25 {
        let url_id = repo::upsert_url(&pool, &format!("https://example.com/{n}"))
            .await
            .unwrap();
        repo::upsert_code(&pool, url_id, Some(ana)).await.unwrap();
    }

    let mut seen = HashSet::new();
    let mut cursor = None;

    loop {
        let page = repo::list_owned(&pool, ana, cursor, 10).await.unwrap();
        if page.is_empty() {
            break;
        }

        for row in &page {
            assert!(
                seen.insert(row.code_id),
                "row {} came back twice",
                row.code_id
            );
        }

        let last = page.last().unwrap();
        cursor = Some((last.created_at, last.code_id));
    }

    assert_eq!(seen.len(), 25);
}

/// The overview's id source: an owner's own codes, newest first, capped.
#[sqlx::test(migrations = "../../migrations")]
async fn owned_code_ids_are_the_owners_newest_first(pool: PgPool) {
    let ana = make_user(&pool, "ana@example.com").await.unwrap().unwrap();
    let bruno = make_user(&pool, "bruno@example.com")
        .await
        .unwrap()
        .unwrap();

    let mut ana_codes = Vec::new();
    for n in 0..3 {
        let url_id = repo::upsert_url(&pool, &format!("https://example.com/{n}"))
            .await
            .unwrap();
        ana_codes.push(repo::upsert_code(&pool, url_id, Some(ana)).await.unwrap());
    }

    // One for bruno and one anonymous, on a shared URL — neither belongs in ana's
    // list.
    let shared = repo::upsert_url(&pool, "https://example.com/shared")
        .await
        .unwrap();
    repo::upsert_code(&pool, shared, Some(bruno)).await.unwrap();
    repo::upsert_code(&pool, shared, None).await.unwrap();

    let ids = repo::list_owned_code_ids(&pool, ana, 50).await.unwrap();

    // Only ana's three, and newest first — the reverse of insertion order.
    let mut expected = ana_codes.clone();
    expected.reverse();
    assert_eq!(ids, expected);

    // The limit caps the list to the newest.
    let capped = repo::list_owned_code_ids(&pool, ana, 2).await.unwrap();
    assert_eq!(capped.as_slice(), &expected[..2]);
}
