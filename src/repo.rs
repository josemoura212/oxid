use sqlx::PgPool;

/// Idempotente: a mesma URL longa sempre devolve o mesmo id.
///
/// São duas queries de propósito. Resolver com uma CTE
/// (`WITH inserido AS (INSERT ...) SELECT ... UNION ALL SELECT ...`) falha sob
/// concorrência: todas as partes de um comando compartilham o mesmo snapshot,
/// então o `DO NOTHING` engole o conflito e o `SELECT` não enxerga a linha do
/// vencedor — zero linhas, sem erro. Em comandos separados, `READ COMMITTED`
/// pega um snapshot novo e o fallback vê a linha commitada.
pub async fn insert_url(pool: &PgPool, long_url: &str) -> Result<i64, sqlx::Error> {
    let inserido = sqlx::query_scalar!(
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

    if let Some(id) = inserido {
        return Ok(id);
    }

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
