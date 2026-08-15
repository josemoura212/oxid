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
        onetime::OneTimeTokens,
        password::{Decoy, Hasher},
        session::SessionStore,
    },
    cache::Cache,
    configuration::{CacheSettings, RateLimitSettings},
    email::Mailer,
    routes,
    state::AppState,
};
use oxid_shared::{
    AccountResponse, ClickStats, CreatedToken, LinkPage, OverviewStats, ShortenResponse,
    SignupResponse,
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

/// A Redis namespace unique to this test's database.
///
/// `#[sqlx::test]` hands every test its own Postgres database, so **every test
/// has a user id 1**. The session index and the one-time token pointers are both
/// keyed by that id, and the Redis behind them is shared across the whole run —
/// so without this, one test's `revoke_all(1)` signs out another test's user, and
/// one test's confirmation token overwrites another's. Both happened.
///
/// `current_database()` is the one name that is already unique per test and
/// needs no coordination.
async fn namespace(pool: &PgPool) -> String {
    sqlx::query_scalar!("SELECT current_database()")
        .fetch_one(pool)
        .await
        .unwrap()
        .unwrap()
}

async fn app(pool: &PgPool) -> Router {
    routes::router(state(pool).await, permissive_rate_limit()).unwrap()
}

/// Extracted so `the_suite_never_mails_anyone` can assert on the state the suite
/// really builds, rather than on one it constructs for the assertion.
async fn state(pool: &PgPool) -> Arc<AppState> {
    let settings = redis_settings();
    let conn = oxid::cache::connect(&settings).await.unwrap();
    let ns = namespace(pool).await;

    Arc::new(AppState {
        db_pool: pool.clone(),
        // Cache disabled: these tests are about sessions, and the cache has its
        // own suite. The session store gets the real Redis.
        cache: Cache::disabled(),
        sessions: SessionStore::with_namespace(conn.clone(), 3600, &ns),
        tokens: OneTimeTokens::with_namespace(conn, &ns),
        // Disabled everywhere in the suite, and asserted as such below. A test
        // run that mails for real spends provider quota to deliver links nobody
        // reads -- to addresses that do not exist.
        mailer: Mailer::disabled(),
        site_url: BASE_URL.to_owned(),
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
    })
}

const fn permissive_rate_limit() -> RateLimitSettings {
    RateLimitSettings {
        shorten_per_second: 1_000,
        shorten_burst: 10_000,
        login_per_second: 1_000,
        login_burst: 10_000,
        email_per_second: 1_000,
        email_burst: 10_000,
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

/// Signs up, confirms the address, and signs in — the three steps it now takes
/// to hold a session.
///
/// The confirmation is applied straight to the row rather than by spending the
/// token from the e-mail. That is deliberate: these tests are about links,
/// sessions and analytics, and routing each of them through the mail flow would
/// make every one of them fail when that flow breaks. The flow has its own tests,
/// where the token really is read and spent.
async fn sign_up(app: &Router, pool: &PgPool) -> String {
    let response = app
        .clone()
        .oneshot(post(
            "/v1/signup",
            &json!({ "email": EMAIL, "password": PASSWORD }),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    confirm(pool, EMAIL).await;
    sign_in(app, EMAIL).await
}

/// Marks an address confirmed without going through the link.
async fn confirm(pool: &PgPool, email: &str) {
    sqlx::query!(
        "UPDATE users SET email_verified_at = now() WHERE email = $1::text::citext",
        email
    )
    .execute(pool)
    .await
    .unwrap();
}

/// A second account, signed in. Two tests need one to prove an owner's data is
/// not reachable from somewhere else.
async fn sign_up_other(app: &Router, pool: &PgPool, email: &str) -> String {
    let response = app
        .clone()
        .oneshot(post(
            "/v1/signup",
            &json!({ "email": email, "password": PASSWORD }),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    confirm(pool, email).await;
    sign_in(app, email).await
}

async fn sign_in(app: &Router, email: &str) -> String {
    let response = app
        .clone()
        .oneshot(post(
            "/v1/login",
            &json!({ "email": email, "password": PASSWORD }),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK, "sign-in must succeed");
    cookie_pair(&set_cookie(&response))
}

/// Signing up no longer signs you in. The address is unconfirmed, and confirming
/// it is what the link in the message is for.
#[sqlx::test(migrations = "../../migrations")]
async fn signup_creates_an_unconfirmed_account_and_no_session(pool: PgPool) {
    let app = app(&pool).await;

    let response = app
        .clone()
        .oneshot(post(
            "/v1/signup",
            &json!({ "email": EMAIL, "password": PASSWORD }),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        response.headers().get(header::SET_COOKIE).is_none(),
        "signup must not hand out a session"
    );

    let body: SignupResponse = body_json(response).await;
    assert_eq!(body.email, EMAIL);

    let verified: Option<Option<sqlx::types::chrono::DateTime<sqlx::types::chrono::Utc>>> =
        sqlx::query_scalar!(
            "SELECT email_verified_at FROM users WHERE email = $1::text::citext",
            EMAIL
        )
        .fetch_optional(&pool)
        .await
        .unwrap();

    assert_eq!(
        verified,
        Some(None),
        "the account exists and is not confirmed"
    );
}

/// The password is right; the address was never confirmed. **401, the same as a
/// wrong password** — telling them apart is the enumeration oracle this flow
/// exists to close, and `signup_then_login_cannot_tell_a_free_address_from_a_taken_one`
/// is the test that keeps it shut.
#[sqlx::test(migrations = "../../migrations")]
async fn an_unconfirmed_account_cannot_sign_in(pool: PgPool) {
    let app = app(&pool).await;

    let signup = app
        .clone()
        .oneshot(post(
            "/v1/signup",
            &json!({ "email": EMAIL, "password": PASSWORD }),
        ))
        .await
        .unwrap();
    assert_eq!(signup.status(), StatusCode::OK);

    let response = app
        .oneshot(post(
            "/v1/login",
            &json!({ "email": EMAIL, "password": PASSWORD }),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

/// The wrong password on an unconfirmed account must still answer 401.
///
/// Answering 403 here would say "this address has an account" to anyone who
/// guessed it — exactly the enumeration the rest of this flow closes. The
/// confirmation check has to sit *after* the password check for that reason, and
/// this is the test that keeps it there.
#[sqlx::test(migrations = "../../migrations")]
async fn a_wrong_password_never_reveals_that_the_account_is_unconfirmed(pool: PgPool) {
    let app = app(&pool).await;

    let signup = app
        .clone()
        .oneshot(post(
            "/v1/signup",
            &json!({ "email": EMAIL, "password": PASSWORD }),
        ))
        .await
        .unwrap();
    assert_eq!(signup.status(), StatusCode::OK);

    let response = app
        .oneshot(post(
            "/v1/login",
            &json!({ "email": EMAIL, "password": "a-completely-wrong-one" }),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

/// The cookie has to carry all three hardening attributes, and `Secure` is forced
/// on here rather than relying on the plain-HTTP default the other tests use.
#[sqlx::test(migrations = "../../migrations")]
async fn the_session_cookie_is_hardened(pool: PgPool) {
    let settings = redis_settings();
    let conn = oxid::cache::connect(&settings).await.unwrap();
    let ns = namespace(&pool).await;
    let state = Arc::new(AppState {
        db_pool: pool.clone(),
        cache: Cache::disabled(),
        sessions: SessionStore::with_namespace(conn.clone(), 3600, &ns),
        tokens: OneTimeTokens::with_namespace(conn, &ns),
        // Disabled everywhere in the suite, and asserted as such below. A test
        // run that mails for real spends provider quota to deliver links nobody
        // reads -- to addresses that do not exist.
        mailer: Mailer::disabled(),
        site_url: BASE_URL.to_owned(),
        base_url: BASE_URL.to_owned(),
        clicks: ClickSink::disabled(),
        clicks_tx: ClickTx::disabled(),
        hasher: Hasher::new(4, Duration::from_secs(5), Decoy::generate().unwrap()),
        secure_cookies: true,
        session_ttl_seconds: 3600,
    });
    let app = routes::router(state, permissive_rate_limit()).unwrap();

    // Through the real flow, because the cookie is only issued at sign-in now.
    app.clone()
        .oneshot(post(
            "/v1/signup",
            &json!({ "email": EMAIL, "password": PASSWORD }),
        ))
        .await
        .unwrap();
    confirm(&pool, EMAIL).await;

    let response = app
        .oneshot(post(
            "/v1/login",
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

/// A registered address answers exactly what a new one answers.
///
/// This is the enumeration fix, and it is the reason the 409 is gone. The old
/// behaviour handed anyone with a list of e-mails a free membership check. The
/// owner of the address still finds out — by e-mail, which is the one channel
/// the person asking does not control.
#[sqlx::test(migrations = "../../migrations")]
async fn a_registered_address_answers_the_same_as_a_new_one(pool: PgPool) {
    let app = app(&pool).await;

    let first = app
        .clone()
        .oneshot(post(
            "/v1/signup",
            &json!({ "email": EMAIL, "password": PASSWORD }),
        ))
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    let first_body: SignupResponse = body_json(first).await;

    let second = app
        .clone()
        .oneshot(post(
            "/v1/signup",
            &json!({ "email": EMAIL, "password": "another-long-one" }),
        ))
        .await
        .unwrap();
    assert_eq!(second.status(), StatusCode::OK, "same status, always");
    let second_body: SignupResponse = body_json(second).await;

    assert_eq!(
        first_body.email, second_body.email,
        "same body, so the response cannot be told apart"
    );

    // And nothing was created the second time: the password did not change, so
    // the original one still works.
    confirm(&pool, EMAIL).await;
    sign_in(&app, EMAIL).await;
}

#[sqlx::test(migrations = "../../migrations")]
async fn me_returns_the_account_when_signed_in(pool: PgPool) {
    let app = app(&pool).await;
    let cookie = sign_up(&app, &pool).await;

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
    let app = app(&pool).await;
    sign_up(&app, &pool).await;

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
    let app = app(&pool).await;

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
    let app = app(&pool).await;
    let cookie = sign_up(&app, &pool).await;

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
        db_pool: pool.clone(),
        cache: Cache::disabled(),
        sessions: SessionStore::with_namespace(conn.clone(), 3600, &ns),
        tokens: OneTimeTokens::with_namespace(conn, &ns),
        mailer: Mailer::disabled(),
        site_url: BASE_URL.to_owned(),
        base_url: BASE_URL.to_owned(),
        clicks: ClickSink::disabled(),
        clicks_tx: ClickTx::disabled(),
        hasher: Hasher::new(4, Duration::from_secs(5), Decoy::generate().unwrap()),
        secure_cookies: false,
        session_ttl_seconds: 3600,
    });
    let app = routes::router(state, permissive_rate_limit()).unwrap();

    // One account, two separate logins — two devices.
    let device_a = sign_up(&app, &pool).await;
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
    let app = app(&pool).await;
    let cookie = sign_up(&app, &pool).await;

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
    let app = app(&pool).await;
    let cookie = sign_up(&app, &pool).await;

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

/// With a link owned, the overview takes its other path — the one that asks
/// ClickHouse for totals and a breakdown rather than short-circuiting on an
/// empty id list.
///
/// The sink is disabled here, so every number is zero. That is the point: the
/// screen has to answer a whole payload before any click exists, because that is
/// what a new account sees. It shipped without these fields at all, and an
/// account with links and no clicks is exactly when their absence showed.
#[sqlx::test(migrations = "../../migrations")]
async fn the_overview_answers_totals_for_an_account_with_links(pool: PgPool) {
    let app = app(&pool).await;
    let cookie = sign_up(&app, &pool).await;

    let shorten = app
        .clone()
        .oneshot(post_with_cookie(
            "/v1/shorten",
            &json!({ "url": "https://example.com/counted" }),
            &cookie,
        ))
        .await
        .unwrap();
    assert_eq!(shorten.status(), StatusCode::OK);

    let response = app
        .oneshot(get_with_cookie("/v1/urls/overview?days=7", &cookie))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let stats: OverviewStats = body_json(response).await;

    assert_eq!(stats.days.len(), 8, "seven days back, plus today");
    assert_eq!(stats.total, 0, "the link exists, nobody has clicked it");
    assert_eq!(stats.unique, 0);
    assert_eq!(stats.breakdown.bots, 0);
    assert!(stats.breakdown.countries.is_empty());
    assert!(stats.breakdown.devices.is_empty());
    assert!(stats.breakdown.referrers.is_empty());
}

/// `days` is clamped to the 30-day ClickHouse TTL, so a longer ask cannot produce
/// an axis reaching past data that no longer exists.
#[sqlx::test(migrations = "../../migrations")]
async fn the_overview_clamps_the_window_to_the_ttl(pool: PgPool) {
    let app = app(&pool).await;
    let cookie = sign_up(&app, &pool).await;

    let response = app
        .oneshot(get_with_cookie("/v1/urls/overview?days=9000", &cookie))
        .await
        .unwrap();

    let stats: OverviewStats = body_json(response).await;
    assert_eq!(stats.days.len(), 31, "30 days back, plus today");
}

#[sqlx::test(migrations = "../../migrations")]
async fn the_overview_needs_a_session(pool: PgPool) {
    let app = app(&pool).await;

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
    let app = app(&pool).await;
    let cookie = sign_up(&app, &pool).await;

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
    let app = app(&pool).await;
    let owner = sign_up(&app, &pool).await;

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
    let intruder = sign_up_other(&app, &pool, "bruno@example.com").await;

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
    let app = app(&pool).await;
    let cookie = sign_up(&app, &pool).await;

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
    let app = app(&pool).await;
    let cookie = sign_up(&app, &pool).await;
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
    let app = app(&pool).await;
    let cookie = sign_up(&app, &pool).await;
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
    let app = app(&pool).await;
    let cookie = sign_up(&app, &pool).await;
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
    let app = app(&pool).await;
    let owner = sign_up(&app, &pool).await;
    let minted = mint_token(&app, &owner, "laptop").await;

    let intruder = sign_up_other(&app, &pool, "bruno@example.com").await;

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
    let app = app(&pool).await;
    let cookie = sign_up(&app, &pool).await;
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

// --- the e-mail flows ---
//
// These are the only tests that spend a real token. Everything above confirms
// accounts by writing the column directly, so a break in this flow shows up
// here and does not take the whole suite with it.

/// Reads the confirmation token out of Redis.
///
/// The message is never sent — the mailer is disabled everywhere in this suite —
/// so the store is where the token has to come from. It stands in for the person
/// reading their inbox, and the assertion that matters is what happens after.
async fn issued_token(pool: &PgPool, purpose: &str, email: &str) -> String {
    let mut conn = oxid::cache::connect(&redis_settings()).await.unwrap();
    let ns = namespace(pool).await;
    let user_id = user_id_of(pool, email).await;

    let token: Option<String> =
        redis::AsyncCommands::get(&mut conn, format!("{ns}:otu:{purpose}:{user_id}"))
            .await
            .unwrap();

    assert!(token.is_some(), "no {purpose} token was issued for {email}");
    token.unwrap_or_default()
}

async fn user_id_of(pool: &PgPool, email: &str) -> i64 {
    sqlx::query_scalar!("SELECT id FROM users WHERE email = $1::text::citext", email)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// The whole path a new account walks: sign up, cannot sign in, spend the link,
/// sign in.
#[sqlx::test(migrations = "../../migrations")]
async fn confirming_the_address_is_what_unlocks_sign_in(pool: PgPool) {
    let app = app(&pool).await;

    app.clone()
        .oneshot(post(
            "/v1/signup",
            &json!({ "email": EMAIL, "password": PASSWORD }),
        ))
        .await
        .unwrap();

    let blocked = app
        .clone()
        .oneshot(post(
            "/v1/login",
            &json!({ "email": EMAIL, "password": PASSWORD }),
        ))
        .await
        .unwrap();
    assert_eq!(blocked.status(), StatusCode::UNAUTHORIZED);

    let token = issued_token(&pool, "v", EMAIL).await;

    let verify = app
        .clone()
        .oneshot(post("/v1/verify-email", &json!({ "token": token })))
        .await
        .unwrap();
    assert_eq!(verify.status(), StatusCode::NO_CONTENT);

    let signed_in = app
        .clone()
        .oneshot(post(
            "/v1/login",
            &json!({ "email": EMAIL, "password": PASSWORD }),
        ))
        .await
        .unwrap();
    assert_eq!(signed_in.status(), StatusCode::OK);

    // And the link is spent: a second click cannot be replayed.
    let again = app
        .oneshot(post("/v1/verify-email", &json!({ "token": token })))
        .await
        .unwrap();
    assert_eq!(again.status(), StatusCode::BAD_REQUEST);
}

/// An unknown address answers exactly what a known one does.
///
/// This endpoint needs no password at all, so leaking membership here would be
/// cheaper for an attacker than the signup ever was.
#[sqlx::test(migrations = "../../migrations")]
async fn resending_says_nothing_about_whether_the_address_exists(pool: PgPool) {
    let app = app(&pool).await;

    app.clone()
        .oneshot(post(
            "/v1/signup",
            &json!({ "email": EMAIL, "password": PASSWORD }),
        ))
        .await
        .unwrap();

    let known = app
        .clone()
        .oneshot(post("/v1/resend-verification", &json!({ "email": EMAIL })))
        .await
        .unwrap();
    let unknown = app
        .oneshot(post(
            "/v1/resend-verification",
            &json!({ "email": "nobody@example.com" }),
        ))
        .await
        .unwrap();

    assert_eq!(known.status(), StatusCode::NO_CONTENT);
    assert_eq!(unknown.status(), known.status());
}

/// Asking twice must leave one working link, not two.
///
/// Every extra live token widens the window an attacker has, and a link that was
/// forwarded or logged an hour ago should stop working the moment a new one is
/// requested.
#[sqlx::test(migrations = "../../migrations")]
async fn a_new_link_retires_the_previous_one(pool: PgPool) {
    let app = app(&pool).await;

    app.clone()
        .oneshot(post(
            "/v1/signup",
            &json!({ "email": EMAIL, "password": PASSWORD }),
        ))
        .await
        .unwrap();

    let first = issued_token(&pool, "v", EMAIL).await;

    app.clone()
        .oneshot(post("/v1/resend-verification", &json!({ "email": EMAIL })))
        .await
        .unwrap();

    let second = issued_token(&pool, "v", EMAIL).await;
    assert_ne!(first, second, "a fresh token is issued");

    let stale = app
        .clone()
        .oneshot(post("/v1/verify-email", &json!({ "token": first })))
        .await
        .unwrap();
    assert_eq!(
        stale.status(),
        StatusCode::BAD_REQUEST,
        "the previous link must be dead"
    );

    let fresh = app
        .oneshot(post("/v1/verify-email", &json!({ "token": second })))
        .await
        .unwrap();
    assert_eq!(fresh.status(), StatusCode::NO_CONTENT);
}

/// The reset, end to end: the link survives being checked, dies on save, and
/// takes every existing session with it.
#[sqlx::test(migrations = "../../migrations")]
async fn a_reset_sets_the_password_and_ends_every_session(pool: PgPool) {
    const NEW_PASSWORD: &str = "an-even-longer-password";

    let app = app(&pool).await;
    let cookie = sign_up(&app, &pool).await;

    // The session works before the reset.
    let before = app
        .clone()
        .oneshot(get_with_cookie("/v1/me", &cookie))
        .await
        .unwrap();
    assert_eq!(before.status(), StatusCode::OK);

    app.clone()
        .oneshot(post("/v1/forgot-password", &json!({ "email": EMAIL })))
        .await
        .unwrap();

    let token = issued_token(&pool, "r", EMAIL).await;

    // Checked on open. Twice, because a refresh or a second tab must not kill it.
    for _ in 0..2 {
        let check = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/v1/reset-password?token={token}"))
                    .header("x-forwarded-for", CLIENT_IP)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            check.status(),
            StatusCode::NO_CONTENT,
            "checking must not spend the link"
        );
    }

    let saved = app
        .clone()
        .oneshot(post(
            "/v1/reset-password",
            &json!({ "token": token, "password": NEW_PASSWORD }),
        ))
        .await
        .unwrap();
    assert_eq!(saved.status(), StatusCode::NO_CONTENT);

    // Spent: the same link cannot be used again.
    let replay = app
        .clone()
        .oneshot(post(
            "/v1/reset-password",
            &json!({ "token": token, "password": "yet-another-password" }),
        ))
        .await
        .unwrap();
    assert_eq!(replay.status(), StatusCode::BAD_REQUEST);

    // Every session is gone — the whole point of a reset.
    let after = app
        .clone()
        .oneshot(get_with_cookie("/v1/me", &cookie))
        .await
        .unwrap();
    assert_eq!(
        after.status(),
        StatusCode::UNAUTHORIZED,
        "the old session must not survive a reset"
    );

    // The old password is dead and the new one works.
    let old = app
        .clone()
        .oneshot(post(
            "/v1/login",
            &json!({ "email": EMAIL, "password": PASSWORD }),
        ))
        .await
        .unwrap();
    assert_eq!(old.status(), StatusCode::UNAUTHORIZED);

    let new = app
        .oneshot(post(
            "/v1/login",
            &json!({ "email": EMAIL, "password": NEW_PASSWORD }),
        ))
        .await
        .unwrap();
    assert_eq!(new.status(), StatusCode::OK);
}

/// Completing a reset confirms the address on the way through.
///
/// Reading a message sent there is the same proof the confirmation link asks
/// for. Leaving the account unconfirmed would lock someone out of an account
/// they just demonstrated control of.
#[sqlx::test(migrations = "../../migrations")]
async fn a_reset_also_confirms_an_unconfirmed_address(pool: PgPool) {
    const NEW_PASSWORD: &str = "an-even-longer-password";

    let app = app(&pool).await;

    app.clone()
        .oneshot(post(
            "/v1/signup",
            &json!({ "email": EMAIL, "password": PASSWORD }),
        ))
        .await
        .unwrap();

    app.clone()
        .oneshot(post("/v1/forgot-password", &json!({ "email": EMAIL })))
        .await
        .unwrap();

    let token = issued_token(&pool, "r", EMAIL).await;

    let saved = app
        .clone()
        .oneshot(post(
            "/v1/reset-password",
            &json!({ "token": token, "password": NEW_PASSWORD }),
        ))
        .await
        .unwrap();
    assert_eq!(saved.status(), StatusCode::NO_CONTENT);

    let signed_in = app
        .oneshot(post(
            "/v1/login",
            &json!({ "email": EMAIL, "password": NEW_PASSWORD }),
        ))
        .await
        .unwrap();
    assert_eq!(
        signed_in.status(),
        StatusCode::OK,
        "the reset proved the address, so sign-in must work"
    );
}

/// A confirmation token must not be spendable as a reset, or the weaker link
/// would grant what the stronger one guards.
#[sqlx::test(migrations = "../../migrations")]
async fn a_confirmation_token_cannot_be_spent_as_a_reset(pool: PgPool) {
    let app = app(&pool).await;

    app.clone()
        .oneshot(post(
            "/v1/signup",
            &json!({ "email": EMAIL, "password": PASSWORD }),
        ))
        .await
        .unwrap();

    let token = issued_token(&pool, "v", EMAIL).await;

    let response = app
        .oneshot(post(
            "/v1/reset-password",
            &json!({ "token": token, "password": "an-even-longer-password" }),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

/// Forgetting a password for an address with no account answers the same as one
/// with an account. The temptation to be helpful here is exactly the oracle.
#[sqlx::test(migrations = "../../migrations")]
async fn forgot_password_says_nothing_about_whether_the_address_exists(pool: PgPool) {
    let app = app(&pool).await;
    let _ = sign_up(&app, &pool).await;

    let known = app
        .clone()
        .oneshot(post("/v1/forgot-password", &json!({ "email": EMAIL })))
        .await
        .unwrap();
    let unknown = app
        .oneshot(post(
            "/v1/forgot-password",
            &json!({ "email": "nobody@example.com" }),
        ))
        .await
        .unwrap();

    assert_eq!(known.status(), StatusCode::NO_CONTENT);
    assert_eq!(unknown.status(), known.status());
}

/// The suite must never mail anyone.
///
/// Asserts on the state `app()` actually builds. The previous version constructed
/// a `Mailer::disabled()` on the spot and checked it was disabled — which passes
/// no matter what `app()` does, including if someone wires a real Resend client
/// into it.
#[sqlx::test(migrations = "../../migrations")]
async fn the_suite_never_mails_anyone(pool: PgPool) {
    let state = state(&pool).await;

    assert!(
        !state.mailer.is_active(),
        "the test AppState built a mailer that sends"
    );
}

/// A confirmed address gets no confirmation link.
///
/// Without this, inverting the `email_verified_at.is_none()` guard passes the
/// whole suite — and anyone holding an address could fire a confirmation e-mail
/// at a confirmed account at will.
#[sqlx::test(migrations = "../../migrations")]
async fn a_confirmed_address_is_not_sent_another_link(pool: PgPool) {
    let app = app(&pool).await;
    let _ = sign_up(&app, &pool).await;

    // The token from the signup is still around — `sign_up` confirms the row
    // directly rather than spending it. What must not happen is a *new* one.
    let before = issued_token(&pool, "v", EMAIL).await;

    let response = app
        .oneshot(post("/v1/resend-verification", &json!({ "email": EMAIL })))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    let after = issued_token(&pool, "v", EMAIL).await;

    assert_eq!(
        before, after,
        "a confirmed account must not be issued another confirmation token"
    );
}

/// The other direction of the purpose split. One test proves the keyspace is
/// separate only if both directions are refused.
#[sqlx::test(migrations = "../../migrations")]
async fn a_reset_token_cannot_be_spent_as_a_confirmation(pool: PgPool) {
    let app = app(&pool).await;

    app.clone()
        .oneshot(post(
            "/v1/signup",
            &json!({ "email": EMAIL, "password": PASSWORD }),
        ))
        .await
        .unwrap();
    app.clone()
        .oneshot(post("/v1/forgot-password", &json!({ "email": EMAIL })))
        .await
        .unwrap();

    let token = issued_token(&pool, "r", EMAIL).await;

    let response = app
        .oneshot(post("/v1/verify-email", &json!({ "token": token })))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

/// Checking a link that is not a link answers 400, not 500 or 204.
#[sqlx::test(migrations = "../../migrations")]
async fn checking_a_bogus_reset_link_is_refused(pool: PgPool) {
    let app = app(&pool).await;

    let response = app
        .oneshot(
            Request::builder()
                .uri("/v1/reset-password?token=not-a-real-token")
                .header("x-forwarded-for", CLIENT_IP)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

/// A reset kills API tokens as well as sessions.
///
/// A stolen session can mint a personal access token, and that token has no
/// expiry. Revoking only the sessions would leave the intruder a credential that
/// still writes into the account.
#[sqlx::test(migrations = "../../migrations")]
async fn a_reset_revokes_api_tokens_too(pool: PgPool) {
    const NEW_PASSWORD: &str = "an-even-longer-password";

    let app = app(&pool).await;
    let cookie = sign_up(&app, &pool).await;
    let minted = mint_token(&app, &cookie, "stolen").await;

    // Asserted against a route that *requires* a credential. `/v1/shorten` accepts
    // anonymous callers by design, so a revoked token there answers 200 as an
    // anonymous shorten — which proves nothing about the token.
    let before = app
        .clone()
        .oneshot(get_with_token("/v1/urls", &minted.secret))
        .await
        .unwrap();
    assert_eq!(before.status(), StatusCode::OK);

    app.clone()
        .oneshot(post("/v1/forgot-password", &json!({ "email": EMAIL })))
        .await
        .unwrap();
    let token = issued_token(&pool, "r", EMAIL).await;

    let saved = app
        .clone()
        .oneshot(post(
            "/v1/reset-password",
            &json!({ "token": token, "password": NEW_PASSWORD }),
        ))
        .await
        .unwrap();
    assert_eq!(saved.status(), StatusCode::NO_CONTENT);

    let after = app
        .oneshot(get_with_token("/v1/urls", &minted.secret))
        .await
        .unwrap();
    assert_eq!(
        after.status(),
        StatusCode::UNAUTHORIZED,
        "an API token must not survive a password reset"
    );
}

/// **The oracle this flow exists to close, rebuilt out of two status codes.**
///
/// Signing up with a random password and then signing in with the same one used
/// to answer differently depending on whether the address was free:
///
/// - free  → the signup created it, the password matches, 403 "confirm your email"
/// - taken → the signup did nothing, the password does not match, 401
///
/// Two requests per address, deterministic, no timing involved. The fix is that
/// an unconfirmed account answers 401 like any other failed sign-in.
#[sqlx::test(migrations = "../../migrations")]
async fn signup_then_login_cannot_tell_a_free_address_from_a_taken_one(pool: PgPool) {
    const PROBE: &str = "a-probe-password-nobody-uses";

    let app = app(&pool).await;

    // A taken address, with a password the prober does not know.
    app.clone()
        .oneshot(post(
            "/v1/signup",
            &json!({ "email": EMAIL, "password": PASSWORD }),
        ))
        .await
        .unwrap();

    let mut statuses = Vec::new();
    for address in [EMAIL, "nobody-has-this@example.com"] {
        app.clone()
            .oneshot(post(
                "/v1/signup",
                &json!({ "email": address, "password": PROBE }),
            ))
            .await
            .unwrap();

        let login = app
            .clone()
            .oneshot(post(
                "/v1/login",
                &json!({ "email": address, "password": PROBE }),
            ))
            .await
            .unwrap();

        statuses.push(login.status());
    }

    assert_eq!(
        statuses[0], statuses[1],
        "a taken address and a free one must answer the same, or signup + login is an enumerator"
    );
}
