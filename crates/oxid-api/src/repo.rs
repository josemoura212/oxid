use sqlx::{
    PgPool,
    types::chrono::{DateTime, Utc},
};

/// Stores the long URL, returning the row that owns it.
///
/// Idempotent, and deliberately two queries. Doing it with a CTE
/// (`WITH inserted AS (INSERT ...) SELECT ... UNION ALL SELECT ...`) fails under
/// concurrency: every part of one command shares the same snapshot, so
/// `DO NOTHING` swallows the conflict and the `SELECT` cannot see the winner's
/// row — zero rows, no error. As separate commands, `READ COMMITTED` takes a
/// fresh snapshot and the fallback sees the committed row.
pub async fn upsert_url(pool: &PgPool, long_url: &str) -> Result<i64, sqlx::Error> {
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

/// Claims a shortcode for `(owner, url)`, returning the id the code is made
/// from. Same idempotence, same reason for two queries.
///
/// `owner_id` of `None` is the anonymous code, and the unique constraint is
/// declared `NULLS NOT DISTINCT` so two anonymous claims for one URL collapse
/// into a single row. A plain `UNIQUE` would treat each `NULL` as unique and
/// hand out a new code every time.
pub async fn upsert_code(
    pool: &PgPool,
    url_id: i64,
    owner_id: Option<i64>,
) -> Result<i64, sqlx::Error> {
    let inserted = sqlx::query_scalar!(
        r#"
        INSERT INTO short_codes (url_id, owner_id)
        VALUES ($1, $2)
        ON CONFLICT ON CONSTRAINT short_codes_owner_url_key DO NOTHING
        RETURNING id
        "#,
        url_id,
        owner_id
    )
    .fetch_optional(pool)
    .await?;

    if let Some(id) = inserted {
        return Ok(id);
    }

    // `IS NOT DISTINCT FROM` rather than `=`, because the anonymous owner is
    // NULL and `NULL = NULL` is NULL, not true — the fallback would find
    // nothing and the call would fail on a row that exists.
    sqlx::query_scalar!(
        r#"
        SELECT id
        FROM short_codes
        WHERE url_id = $1 AND owner_id IS NOT DISTINCT FROM $2
        "#,
        url_id,
        owner_id
    )
    .fetch_one(pool)
    .await
}

#[derive(Debug)]
pub struct Resolved {
    pub long_url: String,
    /// Whether the code has an owner. Decides 301 vs 302 and whether the redirect
    /// records a click — carried out of the same query so the redirect learns it
    /// without a second round trip.
    pub owned: bool,
}

/// Resolves a code id to its destination and ownership.
///
/// One join, both sides by primary key. The cache absorbs almost all of this
/// path, so the extra hop costs nothing at the volumes that matter.
pub async fn resolve_code(pool: &PgPool, code_id: i64) -> Result<Option<Resolved>, sqlx::Error> {
    sqlx::query_as!(
        Resolved,
        r#"
        SELECT u.long_url, (sc.owner_id IS NOT NULL) AS "owned!"
        FROM short_codes sc
        JOIN urls u ON u.id = sc.url_id
        WHERE sc.id = $1
        "#,
        code_id
    )
    .fetch_optional(pool)
    .await
}

/// Whether this code exists and belongs to this owner.
///
/// The dashboard uses it to refuse a code that is not the caller's — answering
/// the same 404 as a code that does not exist, so it cannot be used to probe
/// which codes other people own.
pub async fn owns_code(pool: &PgPool, code_id: i64, owner_id: i64) -> Result<bool, sqlx::Error> {
    let found = sqlx::query_scalar!(
        r#"
        SELECT 1 AS "one!"
        FROM short_codes
        WHERE id = $1 AND owner_id = $2
        "#,
        code_id,
        owner_id
    )
    .fetch_optional(pool)
    .await?;

    Ok(found.is_some())
}

pub struct Credentials {
    pub user_id: i64,
    pub password_hash: String,
}

/// Written by hand rather than derived: a derived `Debug` would put the stored
/// hash into any log line that formats this struct. The hash is not a password,
/// but it is the input to an offline attack, and logs travel further than
/// databases do.
impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Credentials")
            .field("user_id", &self.user_id)
            .finish_non_exhaustive()
    }
}

/// Fetches what a login needs, or nothing when the e-mail is unknown.
///
/// The caller must spend the same CPU either way — see [`crate::auth::password::Decoy`].
///
/// The `::citext` cast is load-bearing. The column is `citext`, but the driver
/// binds the parameter as `text`, and `citext = text` resolves through the base
/// type — a case-**sensitive** comparison. Without the cast, an account created as
/// `Ana@Example.com` cannot be found by typing `ana@example.com`, while the unique
/// index still refuses to let it register twice. The insert compares `citext` to
/// `citext` and behaves; only the lookup silently disagrees.
pub async fn find_credentials(
    pool: &PgPool,
    email: &str,
) -> Result<Option<Credentials>, sqlx::Error> {
    sqlx::query_as!(
        Credentials,
        r#"
        SELECT id AS user_id, password_hash
        FROM users
        WHERE email = $1::text::citext
        "#,
        email
    )
    .fetch_optional(pool)
    .await
}

/// Creates an account, or `None` when the e-mail is taken.
///
/// The conflict is resolved by the database rather than by a prior `SELECT`:
/// checking first and inserting second is a race two concurrent signups win
/// together.
pub async fn create_user(
    pool: &PgPool,
    email: &str,
    password_hash: &str,
) -> Result<Option<i64>, sqlx::Error> {
    sqlx::query_scalar!(
        r#"
        INSERT INTO users (email, password_hash)
        VALUES ($1, $2)
        ON CONFLICT (email) DO NOTHING
        RETURNING id
        "#,
        email,
        password_hash
    )
    .fetch_optional(pool)
    .await
}

pub async fn find_email(pool: &PgPool, user_id: i64) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar!(
        r#"
        SELECT email AS "email!: String"
        FROM users
        WHERE id = $1
        "#,
        user_id
    )
    .fetch_optional(pool)
    .await
}

#[derive(Debug)]
pub struct OwnedCode {
    pub code_id: i64,
    pub long_url: String,
    pub created_at: DateTime<Utc>,
}

/// One page of an owner's codes, newest first.
///
/// Keyset pagination, not `OFFSET`. `OFFSET n` makes the database walk and
/// discard `n` rows before returning anything, so page 500 costs 500 pages of
/// work — and rows inserted mid-pagination shift the window, silently skipping
/// or repeating entries. The cursor here is `(created_at, id)`, and the index
/// on `(owner_id, created_at DESC)` serves it directly.
///
/// `id` is in the cursor because `created_at` alone is not unique: two codes
/// created in the same instant would straddle the boundary, and one of them
/// would never appear on any page.
pub async fn list_owned(
    pool: &PgPool,
    owner_id: i64,
    cursor: Option<(DateTime<Utc>, i64)>,
    limit: i64,
) -> Result<Vec<OwnedCode>, sqlx::Error> {
    let (cursor_at, cursor_id) = match cursor {
        Some((at, id)) => (Some(at), Some(id)),
        None => (None, None),
    };

    sqlx::query_as!(
        OwnedCode,
        r#"
        SELECT sc.id AS code_id, u.long_url, sc.created_at
        FROM short_codes sc
        JOIN urls u ON u.id = sc.url_id
        WHERE sc.owner_id = $1
          AND ($2::timestamptz IS NULL
               OR (sc.created_at, sc.id) < ($2::timestamptz, $3::bigint))
        ORDER BY sc.created_at DESC, sc.id DESC
        LIMIT $4
        "#,
        owner_id,
        cursor_at,
        cursor_id,
        limit
    )
    .fetch_all(pool)
    .await
}

/// Just the ids of an owner's codes, newest first, for the overview.
///
/// The overview asks ClickHouse for the daily series of every one of these at
/// once, so the `limit` here bounds the `IN` list that query builds — not the
/// number of lines the chart ends up drawing, which the handler trims further by
/// click volume. Newest-first so a capped fetch keeps the links most likely to
/// still be in the 30-day analytics window.
pub async fn list_owned_code_ids(
    pool: &PgPool,
    owner_id: i64,
    limit: i64,
) -> Result<Vec<i64>, sqlx::Error> {
    sqlx::query_scalar!(
        r#"
        SELECT id
        FROM short_codes
        WHERE owner_id = $1
        ORDER BY created_at DESC, id DESC
        LIMIT $2
        "#,
        owner_id,
        limit
    )
    .fetch_all(pool)
    .await
}

// --- API tokens ---

/// One of an owner's tokens, as the list screen shows it. No digest: the caller
/// has no use for it and it has no business leaving this module.
#[derive(Debug)]
pub struct ApiToken {
    pub id: i64,
    pub name: String,
    pub created_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
}

pub async fn create_token(
    pool: &PgPool,
    user_id: i64,
    name: &str,
    token_hash: &str,
) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar!(
        r#"
        INSERT INTO api_tokens (user_id, name, token_hash)
        VALUES ($1, $2, $3)
        RETURNING id
        "#,
        user_id,
        name,
        token_hash
    )
    .fetch_one(pool)
    .await
}

/// Resolves a presented token to its owner, and records the use.
///
/// One statement rather than a SELECT followed by an UPDATE. Two would be two
/// round trips on every authenticated call, and would leave a window where a
/// token revoked in between still authenticates.
///
/// `last_used_at` is truncated to the hour. Not to save the write — Postgres
/// writes a new tuple either way — but because the column exists to answer "is
/// this unfamiliar token safe to revoke", and that question has never needed
/// better resolution than "some time today". A precise timestamp would invite
/// reading it as an audit log, which it is not.
pub async fn touch_token(pool: &PgPool, token_hash: &str) -> Result<Option<i64>, sqlx::Error> {
    sqlx::query_scalar!(
        r#"
        UPDATE api_tokens
        SET last_used_at = date_trunc('hour', now())
        WHERE token_hash = $1
        RETURNING user_id
        "#,
        token_hash
    )
    .fetch_optional(pool)
    .await
}

pub async fn list_tokens(pool: &PgPool, user_id: i64) -> Result<Vec<ApiToken>, sqlx::Error> {
    sqlx::query_as!(
        ApiToken,
        r#"
        SELECT id, name, created_at, last_used_at
        FROM api_tokens
        WHERE user_id = $1
        ORDER BY created_at DESC, id DESC
        "#,
        user_id
    )
    .fetch_all(pool)
    .await
}

/// Deletes one of the caller's tokens. `user_id` is in the WHERE clause, not
/// checked beforehand: revoking someone else's token has to be impossible by
/// construction rather than by remembering to guard.
pub async fn revoke_token(pool: &PgPool, id: i64, user_id: i64) -> Result<bool, sqlx::Error> {
    let deleted = sqlx::query!(
        r#"
        DELETE FROM api_tokens
        WHERE id = $1 AND user_id = $2
        "#,
        id,
        user_id
    )
    .execute(pool)
    .await?;

    Ok(deleted.rows_affected() > 0)
}
