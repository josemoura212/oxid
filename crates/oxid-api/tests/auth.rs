//! The full account flow, over the real router with a real Redis behind the
//! session store. Postgres comes from `#[sqlx::test]`; Redis is shared, and safe
//! to share because session ids are random 128-bit values that never collide.
//!
//! `tests/routes.rs` covers everything that fails *before* a session is created —
//! validation, CORS, the anonymous 401s — with no Redis. This file covers what
//! only works once a session exists.
#![allow(clippy::unwrap_used)]

use std::{sync::Arc, time::Duration};

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode, header},
};
use http_body_util::BodyExt;
use oxid::{
    analytics::{ClickSink, ClickTx},
    auth::{
        password::{Decoy, Hasher},
        session::SessionStore,
    },
    cache::Cache,
    configuration::{CacheSettings, RateLimitSettings},
    routes,
    state::AppState,
};
use oxid_shared::{
    AccountResponse, ClickStats, CreatedToken, LinkPage, OverviewStats, ShortenResponse,
};
use serde_json::json;
use sqlx::PgPool;
use tower::ServiceExt;

const BASE_URL: &str = "https://oxid.test";
const DEFAULT_REDIS: &str = "redis://127.0.0.1:6381";

fn redis_settings() -> CacheSettings {
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

async fn app(pool: PgPool) -> Router {
    let settings = redis_settings();
    let conn = oxid::cache::connect(&settings).await.unwrap();

    let state = Arc::new(AppState {
        db_pool: pool,
        // Cache disabled: these tests are about sessions, and the cache has its
        // own suite. The session store gets the real Redis.
        cache: Cache::disabled(),
        sessions: SessionStore::new(conn, 3600),
        base_url: BASE_URL.to_owned(),
        clicks: ClickSink::disabled(),
        clicks_tx: ClickTx::disabled(),
        hasher: Hasher::new(4, Duration::from_secs(5), Decoy::generate().unwrap()),
        // False, so the cookie is not `Secure` — a test client speaks plain HTTP,
        // and a `Secure` cookie would be dropped, making every follow-up look
        // anonymous. The `Secure` attribute itself is asserted separately, where
        // the flag is forced on.
        secure_cookies: false,
        session_ttl_seconds: 3600,
    });

    routes::router(state, permissive_rate_limit()).unwrap()
}

const fn permissive_rate_limit() -> RateLimitSettings {
    RateLimitSettings {
        shorten_per_second: 1_000,
        shorten_burst: 10_000,
        login_per_second: 1_000,
        login_burst: 10_000,
        hash_concurrency: 4,
        hash_wait_ms: 5_000,
    }
}

fn post(path: &str, body: &serde_json::Value) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(path)
        .header(header::CONTENT_TYPE, "application/json")
        .header("x-forwarded-for", CLIENT_IP)
        .body(Body::from(body.to_string()))
        .unwrap()
}

fn post_with_cookie(path: &str, body: &serde_json::Value, cookie: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(path)
        .header(header::CONTENT_TYPE, "application/json")
        .header("x-forwarded-for", CLIENT_IP)
        .header(header::COOKIE, cookie)
        .body(Body::from(body.to_string()))
        .unwrap()
}

fn get_with_cookie(path: &str, cookie: &str) -> Request<Body> {
    Request::builder()
        .uri(path)
        .header(header::COOKIE, cookie)
        .body(Body::empty())
        .unwrap()
}

/// The raw `Set-Cookie` line, so a test can assert on its attributes.
fn set_cookie(response: &axum::response::Response) -> String {
    response
        .headers()
        .get(header::SET_COOKIE)
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned()
}

/// Just the `name=value` pair, ready to send back as a `Cookie` header.
fn cookie_pair(set_cookie: &str) -> String {
    set_cookie.split(';').next().unwrap().to_owned()
}

async fn body_json<T: serde::de::DeserializeOwned>(response: axum::response::Response) -> T {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}

/// The rate limiter on signup/login keys on the client IP via X-Forwarded-For.
/// `oneshot` has no socket, so without this header `SmartIpKeyExtractor` finds no
/// key and the layer answers 500 before the handler ever runs — the same header
/// Traefik sets in front of the real service.
const CLIENT_IP: &str = "203.0.113.42";

const EMAIL: &str = "ana@example.com";
const PASSWORD: &str = "a-long-enough-password";

async fn sign_up(app: &Router) -> String {
    let response = app
        .clone()
        .oneshot(post(
            "/v1/signup",
            &json!({ "email": EMAIL, "password": PASSWORD }),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    cookie_pair(&set_cookie(&response))
}

#[sqlx::test(migrations = "../../migrations")]
async fn signup_returns_the_account_and_a_session(pool: PgPool) {
    let app = app(pool).await;

    let response = app
        .clone()
        .oneshot(post(
            "/v1/signup",
            &json!({ "email": EMAIL, "password": PASSWORD }),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let raw = set_cookie(&response);
    assert!(raw.starts_with("oxid_session="));

    let account: AccountResponse = body_json(response).await;
    assert_eq!(account.email, EMAIL);
}

/// The cookie has to carry all three hardening attributes, and `Secure` is forced
/// on here rather than relying on the plain-HTTP default the other tests use.
#[sqlx::test(migrations = "../../migrations")]
async fn the_session_cookie_is_hardened(pool: PgPool) {
    let settings = redis_settings();
    let conn = oxid::cache::connect(&settings).await.unwrap();
    let state = Arc::new(AppState {
        db_pool: pool,
        cache: Cache::disabled(),
        sessions: SessionStore::new(conn, 3600),
        base_url: BASE_URL.to_owned(),
        clicks: ClickSink::disabled(),
        clicks_tx: ClickTx::disabled(),
        hasher: Hasher::new(4, Duration::from_secs(5), Decoy::generate().unwrap()),
        secure_cookies: true,
        session_ttl_seconds: 3600,
    });
    let app = routes::router(state, permissive_rate_limit()).unwrap();

    let response = app
        .oneshot(post(
            "/v1/signup",
            &json!({ "email": EMAIL, "password": PASSWORD }),
        ))
        .await
        .unwrap();

    let raw = set_cookie(&response).to_ascii_lowercase();
    assert!(raw.contains("httponly"), "cookie is not HttpOnly: {raw}");
    assert!(raw.contains("secure"), "cookie is not Secure: {raw}");
    assert!(
        raw.contains("samesite=lax"),
        "cookie is not SameSite=Lax: {raw}"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_taken_email_is_refused(pool: PgPool) {
    let app = app(pool).await;
    sign_up(&app).await;

    let response = app
        .oneshot(post(
            "/v1/signup",
            &json!({ "email": EMAIL, "password": "another-long-one" }),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CONFLICT);
}

#[sqlx::test(migrations = "../../migrations")]
async fn me_returns_the_account_when_signed_in(pool: PgPool) {
    let app = app(pool).await;
    let cookie = sign_up(&app).await;

    let response = app
        .oneshot(get_with_cookie("/v1/me", &cookie))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let account: AccountResponse = body_json(response).await;
    assert_eq!(account.email, EMAIL);
}

#[sqlx::test(migrations = "../../migrations")]
async fn login_with_the_right_password_works_and_a_wrong_one_does_not(pool: PgPool) {
    let app = app(pool).await;
    sign_up(&app).await;

    let ok = app
        .clone()
        .oneshot(post(
            "/v1/login",
            &json!({ "email": EMAIL, "password": PASSWORD }),
        ))
        .await
        .unwrap();
    assert_eq!(ok.status(), StatusCode::OK);

    let wrong = app
        .oneshot(post(
            "/v1/login",
            &json!({ "email": EMAIL, "password": "not-the-password" }),
        ))
        .await
        .unwrap();
    assert_eq!(wrong.status(), StatusCode::UNAUTHORIZED);
}

/// An unknown e-mail is a 401, same as a wrong password — the decoy makes the two
/// indistinguishable to the caller.
#[sqlx::test(migrations = "../../migrations")]
async fn login_for_an_unknown_email_is_also_401(pool: PgPool) {
    let app = app(pool).await;

    let response = app
        .oneshot(post(
            "/v1/login",
            &json!({ "email": "nobody@example.com", "password": PASSWORD }),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

/// Logout has to actually revoke: the old cookie must stop authenticating, which
/// is the whole reason sessions live in Redis rather than in a signed token.
#[sqlx::test(migrations = "../../migrations")]
async fn logout_revokes_the_session(pool: PgPool) {
    let app = app(pool).await;
    let cookie = sign_up(&app).await;

    // The cookie works before logout.
    let before = app
        .clone()
        .oneshot(get_with_cookie("/v1/me", &cookie))
        .await
        .unwrap();
    assert_eq!(before.status(), StatusCode::OK);

    let logout = app
        .clone()
        .oneshot(post_with_cookie("/v1/logout", &json!({}), &cookie))
        .await
        .unwrap();
    assert_eq!(logout.status(), StatusCode::NO_CONTENT);

    // And not after — same cookie, now revoked server-side.
    let after = app
        .oneshot(get_with_cookie("/v1/me", &cookie))
        .await
        .unwrap();
    assert_eq!(after.status(), StatusCode::UNAUTHORIZED);
}

/// "Sign out everywhere" kills every session, not just the one that called it.
///
/// Two logins for one account (two devices), then `logout-all` from one — both
/// cookies must stop working. A per-namespace store keeps this isolated on the
/// shared Redis, since the per-user index is keyed by a user id that repeats
/// across test databases.
#[sqlx::test(migrations = "../../migrations")]
async fn logout_all_revokes_every_device(pool: PgPool) {
    use oxid::auth::session::SessionStore;

    let settings = redis_settings();
    let conn = oxid::cache::connect(&settings).await.unwrap();
    // A namespace unique to this test, so its per-user index cannot collide with
    // another test database's user id 1.
    let ns = format!("test-logout-all-{}", std::process::id());
    let state = Arc::new(AppState {
        db_pool: pool,
        cache: Cache::disabled(),
        sessions: SessionStore::with_namespace(conn, 3600, &ns),
        base_url: BASE_URL.to_owned(),
        clicks: ClickSink::disabled(),
        clicks_tx: ClickTx::disabled(),
        hasher: Hasher::new(4, Duration::from_secs(5), Decoy::generate().unwrap()),
        secure_cookies: false,
        session_ttl_seconds: 3600,
    });
    let app = routes::router(state, permissive_rate_limit()).unwrap();

    // One account, two separate logins — two devices.
    let device_a = sign_up(&app).await;
    let device_b = {
        let response = app
            .clone()
            .oneshot(post(
                "/v1/login",
                &json!({ "email": EMAIL, "password": PASSWORD }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        cookie_pair(&set_cookie(&response))
    };

    // Sign out everywhere, from device A.
    let response = app
        .clone()
        .oneshot(post_with_cookie("/v1/logout-all", &json!({}), &device_a))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    // Neither device authenticates anymore.
    for (label, cookie) in [("A", &device_a), ("B", &device_b)] {
        let response = app
            .clone()
            .oneshot(get_with_cookie("/v1/me", cookie))
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::UNAUTHORIZED,
            "device {label} still authenticated after logout-all"
        );
    }
}

/// Shortening while signed in claims the code for the account, and it shows up in
/// the owner's list.
#[sqlx::test(migrations = "../../migrations")]
async fn a_signed_in_shorten_lands_in_the_owners_list(pool: PgPool) {
    let app = app(pool).await;
    let cookie = sign_up(&app).await;

    let shorten = app
        .clone()
        .oneshot(post_with_cookie(
            "/v1/shorten",
            &json!({ "url": "https://example.com/mine" }),
            &cookie,
        ))
        .await
        .unwrap();
    assert_eq!(shorten.status(), StatusCode::OK);

    let list = app
        .oneshot(get_with_cookie("/v1/urls", &cookie))
        .await
        .unwrap();
    assert_eq!(list.status(), StatusCode::OK);

    let page: LinkPage = body_json(list).await;
    assert_eq!(page.links.len(), 1);
    assert_eq!(page.links[0].long_url, "https://example.com/mine");
}

// --- analytics dashboards ---
//
// The sink is disabled in this file, so these assert the *shape* the dashboards
// depend on rather than click counts: the day axis, the density that makes the
// front's index-for-index alignment correct, and who is allowed to read what.
// `tests/analytics.rs` covers the counting against a real ClickHouse.

/// The window's axis is inclusive at both ends: seven days back plus today.
#[sqlx::test(migrations = "../../migrations")]
async fn the_overview_answers_a_dense_day_axis(pool: PgPool) {
    let app = app(pool).await;
    let cookie = sign_up(&app).await;

    let response = app
        .oneshot(get_with_cookie("/v1/urls/overview?days=7", &cookie))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let stats: OverviewStats = body_json(response).await;

    assert_eq!(stats.days.len(), 8, "seven days back, plus today");
    // No clicks recorded (the sink is disabled), so there is no line to draw.
    assert!(stats.links.is_empty());
}

/// `days` is clamped to the 30-day ClickHouse TTL, so a longer ask cannot produce
/// an axis reaching past data that no longer exists.
#[sqlx::test(migrations = "../../migrations")]
async fn the_overview_clamps_the_window_to_the_ttl(pool: PgPool) {
    let app = app(pool).await;
    let cookie = sign_up(&app).await;

    let response = app
        .oneshot(get_with_cookie("/v1/urls/overview?days=9000", &cookie))
        .await
        .unwrap();

    let stats: OverviewStats = body_json(response).await;
    assert_eq!(stats.days.len(), 31, "30 days back, plus today");
}

#[sqlx::test(migrations = "../../migrations")]
async fn the_overview_needs_a_session(pool: PgPool) {
    let app = app(pool).await;

    let response = app
        .oneshot(
            Request::builder()
                .uri("/v1/urls/overview")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

/// The per-link series is dense too. A sparse one would draw a single bar across
/// the whole window for a link clicked on one day.
#[sqlx::test(migrations = "../../migrations")]
async fn per_link_stats_are_dense_over_the_window(pool: PgPool) {
    let app = app(pool).await;
    let cookie = sign_up(&app).await;

    let shorten = app
        .clone()
        .oneshot(post_with_cookie(
            "/v1/shorten",
            &json!({ "url": "https://example.com/measured" }),
            &cookie,
        ))
        .await
        .unwrap();
    let created: ShortenResponse = body_json(shorten).await;

    let response = app
        .oneshot(get_with_cookie(
            &format!("/v1/urls/{}/stats?days=7", created.code),
            &cookie,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let stats: ClickStats = body_json(response).await;

    assert_eq!(stats.series.len(), 8, "one point per day, gaps filled");
    assert_eq!(stats.total, 0);
    assert!(
        stats.series.iter().all(|point| point.clicks == 0),
        "no clicks were recorded"
    );
}

/// A code that is not the caller's answers the same 404 as one that does not
/// exist, so the endpoint cannot be used to probe which codes others own.
#[sqlx::test(migrations = "../../migrations")]
async fn per_link_stats_refuse_someone_elses_code(pool: PgPool) {
    let app = app(pool).await;
    let owner = sign_up(&app).await;

    let shorten = app
        .clone()
        .oneshot(post_with_cookie(
            "/v1/shorten",
            &json!({ "url": "https://example.com/private" }),
            &owner,
        ))
        .await
        .unwrap();
    let created: ShortenResponse = body_json(shorten).await;

    // A second account, with no claim on that code.
    let intruder = app
        .clone()
        .oneshot(post(
            "/v1/signup",
            &json!({ "email": "bruno@example.com", "password": PASSWORD }),
        ))
        .await
        .unwrap();
    let intruder = cookie_pair(&set_cookie(&intruder));

    let response = app
        .clone()
        .oneshot(get_with_cookie(
            &format!("/v1/urls/{}/stats", created.code),
            &intruder,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    // And the same answer for a code nobody owns, which is what makes the two
    // indistinguishable.
    let unknown = app
        .oneshot(get_with_cookie("/v1/urls/zzzzzzz/stats", &intruder))
        .await
        .unwrap();
    assert_eq!(unknown.status(), StatusCode::NOT_FOUND);
}

/// The 301/302 split, which is the reason click analytics can exist at all.
///
/// A 301 is cached by the browser, so the second click never reaches the server
/// and cannot be counted. Only a code with an owner — the one with a dashboard to
/// feed — becomes a 302; anonymous codes stay 301 and cacheable, which is the path
/// the load tests measure. `tests/routes.rs` covers the anonymous side, and it has
/// no session to create an owned code with.
#[sqlx::test(migrations = "../../migrations")]
async fn an_owned_code_answers_302_so_its_clicks_keep_arriving(pool: PgPool) {
    let app = app(pool).await;
    let cookie = sign_up(&app).await;

    let owned = app
        .clone()
        .oneshot(post_with_cookie(
            "/v1/shorten",
            &json!({ "url": "https://example.com/owned" }),
            &cookie,
        ))
        .await
        .unwrap();
    let owned: ShortenResponse = body_json(owned).await;

    // The same URL claimed by nobody. A separate code, per the ownership split.
    let anonymous = app
        .clone()
        .oneshot(post(
            "/v1/shorten",
            &json!({ "url": "https://example.com/anonymous" }),
        ))
        .await
        .unwrap();
    let anonymous: ShortenResponse = body_json(anonymous).await;

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/{}", owned.code))
                .header(header::USER_AGENT, "curl/8.7")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        response.status(),
        StatusCode::FOUND,
        "an owned code answering 301 would be cached and stop being counted"
    );
    assert_eq!(
        response.headers().get(header::LOCATION).unwrap(),
        "https://example.com/owned"
    );

    let response = app
        .oneshot(
            Request::builder()
                .uri(format!("/{}", anonymous.code))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        response.status(),
        StatusCode::MOVED_PERMANENTLY,
        "an anonymous code has no dashboard, so it stays cacheable"
    );
    assert_eq!(
        response.headers().get(header::LOCATION).unwrap(),
        "https://example.com/anonymous"
    );
}

// --- API tokens ---

async fn mint_token(app: &Router, cookie: &str, name: &str) -> CreatedToken {
    let response = app
        .clone()
        .oneshot(post_with_cookie(
            "/v1/tokens",
            &json!({ "name": name }),
            cookie,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    body_json(response).await
}

fn get_with_token(path: &str, secret: &str) -> Request<Body> {
    Request::builder()
        .uri(path)
        .header(header::AUTHORIZATION, format!("Bearer {secret}"))
        .body(Body::empty())
        .unwrap()
}

fn post_with_token(path: &str, body: &serde_json::Value, secret: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(path)
        .header(header::CONTENT_TYPE, "application/json")
        .header("x-forwarded-for", CLIENT_IP)
        .header(header::AUTHORIZATION, format!("Bearer {secret}"))
        .body(Body::from(body.to_string()))
        .unwrap()
}

/// What the extension is for: a credential that is not the cookie, shortening
/// into the account it belongs to.
#[sqlx::test(migrations = "../../migrations")]
async fn a_token_authenticates_and_the_link_lands_in_the_account(pool: PgPool) {
    let app = app(pool).await;
    let cookie = sign_up(&app).await;
    let minted = mint_token(&app, &cookie, "laptop").await;

    let response = app
        .clone()
        .oneshot(post_with_token(
            "/v1/shorten",
            &json!({ "url": "https://example.com/from-the-extension" }),
            &minted.secret,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    // Listed through the *cookie*, proving the token wrote into the same account
    // rather than somewhere of its own.
    let page: LinkPage = body_json(
        app.oneshot(get_with_cookie("/v1/urls", &cookie))
            .await
            .unwrap(),
    )
    .await;

    assert_eq!(page.links.len(), 1);
    assert_eq!(
        page.links[0].long_url,
        "https://example.com/from-the-extension"
    );
}

/// The rule that keeps one stolen token from becoming a permanent foothold.
///
/// Found by hand after the first implementation shipped it broken: extending the
/// `Session` extractor to accept tokens silently gave tokens the run of the
/// credential endpoints, so a leaked token could mint replacements faster than
/// anyone could revoke them.
#[sqlx::test(migrations = "../../migrations")]
async fn a_token_cannot_manage_tokens(pool: PgPool) {
    let app = app(pool).await;
    let cookie = sign_up(&app).await;
    let minted = mint_token(&app, &cookie, "laptop").await;

    let mint_again = app
        .clone()
        .oneshot(post_with_token(
            "/v1/tokens",
            &json!({ "name": "escalation" }),
            &minted.secret,
        ))
        .await
        .unwrap();
    assert_eq!(
        mint_again.status(),
        StatusCode::UNAUTHORIZED,
        "a token that can mint tokens survives its own revocation"
    );

    let list = app
        .clone()
        .oneshot(get_with_token("/v1/tokens", &minted.secret))
        .await
        .unwrap();
    assert_eq!(list.status(), StatusCode::UNAUTHORIZED);

    let revoke = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/v1/tokens/{}", minted.token.id))
                .header(header::AUTHORIZATION, format!("Bearer {}", minted.secret))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(revoke.status(), StatusCode::UNAUTHORIZED);

    // And the cookie still manages them, so the restriction landed on the
    // credential rather than on the endpoints.
    let list = app
        .oneshot(get_with_cookie("/v1/tokens", &cookie))
        .await
        .unwrap();
    assert_eq!(list.status(), StatusCode::OK);
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_revoked_token_stops_working(pool: PgPool) {
    let app = app(pool).await;
    let cookie = sign_up(&app).await;
    let minted = mint_token(&app, &cookie, "laptop").await;

    let revoke = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/v1/tokens/{}", minted.token.id))
                .header(header::COOKIE, cookie.clone())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(revoke.status(), StatusCode::NO_CONTENT);

    let after = app
        .oneshot(get_with_token("/v1/urls", &minted.secret))
        .await
        .unwrap();
    assert_eq!(after.status(), StatusCode::UNAUTHORIZED);
}

/// Someone else's token id answers 404, the same as one that does not exist —
/// revocation is scoped in the WHERE clause, so probing ids learns nothing.
#[sqlx::test(migrations = "../../migrations")]
async fn a_token_cannot_be_revoked_by_another_account(pool: PgPool) {
    let app = app(pool).await;
    let owner = sign_up(&app).await;
    let minted = mint_token(&app, &owner, "laptop").await;

    let intruder = app
        .clone()
        .oneshot(post(
            "/v1/signup",
            &json!({ "email": "bruno@example.com", "password": PASSWORD }),
        ))
        .await
        .unwrap();
    let intruder = cookie_pair(&set_cookie(&intruder));

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/v1/tokens/{}", minted.token.id))
                .header(header::COOKIE, intruder)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    // Still the owner's, still working.
    let still = app
        .oneshot(get_with_token("/v1/urls", &minted.secret))
        .await
        .unwrap();
    assert_eq!(still.status(), StatusCode::OK);
}

/// The secret exists in exactly one response. A list that leaked it would make
/// storing only a digest pointless.
#[sqlx::test(migrations = "../../migrations")]
async fn the_secret_is_never_returned_again(pool: PgPool) {
    let app = app(pool).await;
    let cookie = sign_up(&app).await;
    let minted = mint_token(&app, &cookie, "laptop").await;

    let response = app
        .oneshot(get_with_cookie("/v1/tokens", &cookie))
        .await
        .unwrap();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let raw = String::from_utf8_lossy(&body);

    assert!(
        !raw.contains(&minted.secret),
        "the list handed back the secret"
    );
}
