use sqlx::PgPool;

/// EN: Idempotent — the same long URL always returns the same id.
///
/// EN: Two queries on purpose. Doing it with a CTE
/// EN: (`WITH inserted AS (INSERT ...) SELECT ... UNION ALL SELECT ...`) fails
/// EN: under concurrency: every part of one command shares the same snapshot, so
/// EN: `DO NOTHING` swallows the conflict and the `SELECT` cannot see the winner's
/// EN: row — zero rows, no error. As separate commands, `READ COMMITTED` takes a
/// EN: fresh snapshot and the fallback sees the committed row.
/// PT: Idempotente — a mesma URL longa sempre devolve o mesmo id.
///
/// PT: São duas queries de propósito. Resolver com uma CTE
/// PT: (`WITH inserted AS (INSERT ...) SELECT ... UNION ALL SELECT ...`) falha sob
/// PT: concorrência: todas as partes de um comando compartilham o mesmo snapshot,
/// PT: então o `DO NOTHING` engole o conflito e o `SELECT` não enxerga a linha do
/// PT: vencedor — zero linhas, sem erro. Em comandos separados, `READ COMMITTED`
/// PT: pega um snapshot novo e o fallback vê a linha commitada.
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

    // EN: `url_sha256` is the same function backing the generated column — the
    // EN: stored hash and the looked-up hash cannot drift apart.
    // PT: `url_sha256` é a mesma função da coluna gerada — o hash gravado e o
    // PT: hash procurado não têm como divergir.
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
