use sqlx::PgPool;

/// Idempotent — the same long URL always returns the same id.
///
/// Two queries on purpose. Doing it with a CTE
/// (`WITH inserted AS (INSERT ...) SELECT ... UNION ALL SELECT ...`) fails
/// under concurrency: every part of one command shares the same snapshot, so
/// `DO NOTHING` swallows the conflict and the `SELECT` cannot see the winner's
/// row — zero rows, no error. As separate commands, `READ COMMITTED` takes a
/// fresh snapshot and the fallback sees the committed row.
pub async fn insert_url(pool: &PgPool, long_url: &str) -> Result<i64, sqlx::Error> {
    let inserted = sqlx::query_scalar!(
        r#"
        INSERT INTO urls (long_url)
        VALUES ($1)
        ON CONFLICT (url_hash) DO NOTHING
        RETURNING id
        "#,
        long_url
    )
    .fetch_optional(pool)
    .await?;

    if let Some(id) = inserted {
        return Ok(id);
    }

    // `url_sha256` is the same function backing the generated column — the
    // stored hash and the looked-up hash cannot drift apart.
    sqlx::query_scalar!(
        r#"
        SELECT id
        FROM urls
        WHERE url_hash = url_sha256($1)
        "#,
        long_url
    )
    .fetch_one(pool)
    .await
}

pub async fn get_url(pool: &PgPool, id: i64) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar!(
        r#"
        SELECT long_url
        FROM urls
        WHERE id = $1
        "#,
        id
    )
    .fetch_optional(pool)
    .await
}
