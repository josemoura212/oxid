//! End-to-end tests over the real router: request in, response out, Postgres in
//! the middle. `#[sqlx::test]` gives each test its own temporary database.
//!
//! `allow-unwrap-in-tests` in clippy.toml only covers functions the linter can
//! see are tests. The helpers below are plain functions, so the allow has to be
//! stated here — a panic in a helper is still a failing test, which is the point.
#![allow(clippy::unwrap_used)]

use std::sync::Arc;

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode, header},
};
use http_body_util::BodyExt;
use oxid::{
    auth::{
        password::{Decoy, Hasher},
        session::SessionStore,
    },
    cache::Cache,
    configuration::RateLimitSettings,
    routes,
    state::AppState,
};
use oxid_shared::{MAX_URL_LEN, PROBLEM_JSON, ProblemDetails, ShortenResponse};
use serde_json::json;
use sqlx::PgPool;
use tower::ServiceExt;

const BASE_URL: &str = "https://oxid.test";

/// Room to spare, and no waiting. These tests are not about the hashing cap;
/// a tight one here would only give them a way to fail for the wrong reason.
const HASH_CONCURRENCY: usize = 8;
const HASH_WAIT_MS: u64 = 5_000;

/// Cache disabled on purpose: these tests assert routing and status codes, and a
/// shared Redis would make them order-dependent. Caching has its own suite.
///
/// The rate limit is set high enough to never trigger — these tests fire dozens
/// of requests in a loop from one address, which is exactly what it exists to
/// stop. Rate limiting has its own test.
fn app(pool: PgPool) -> Router {
    let state = Arc::new(AppState {
        db_pool: pool,
        cache: Cache::disabled(),
        // No Redis here, so nobody can be signed in — which is what these tests
        // want: they cover the anonymous surface.
        sessions: SessionStore::disabled(),
        base_url: BASE_URL.to_owned(),
        hasher: Hasher::new(
            HASH_CONCURRENCY,
            std::time::Duration::from_millis(HASH_WAIT_MS),
            Decoy::generate().unwrap(),
        ),
        secure_cookies: true,
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
        hash_concurrency: HASH_CONCURRENCY,
        hash_wait_ms: HASH_WAIT_MS,
    }
}

/// The rate limiter keys on the client IP, taken from X-Forwarded-For. `oneshot`
/// has no socket behind it, so the header is what stands in for one — the same
/// thing Traefik sets in front of the real service.
const CLIENT_IP: &str = "203.0.113.10";

fn post_shorten(url: &str) -> Request<Body> {
    post_shorten_from(url, CLIENT_IP)
}

fn post_shorten_from(url: &str, client_ip: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri("/v1/shorten")
        .header(header::CONTENT_TYPE, "application/json")
        .header("x-forwarded-for", client_ip)
        .body(Body::from(json!({ "url": url }).to_string()))
        .unwrap()
}

fn get_code(code: &str) -> Request<Body> {
    Request::builder()
        .uri(format!("/{code}"))
        .body(Body::empty())
        .unwrap()
}

async fn body_json<T: serde::de::DeserializeOwned>(response: axum::response::Response) -> T {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}

async fn shorten(app: &Router, url: &str) -> ShortenResponse {
    let response = app.clone().oneshot(post_shorten(url)).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    body_json(response).await
}

#[sqlx::test(migrations = "../../migrations")]
async fn shorten_returns_a_7_char_code(pool: PgPool) {
    let app = app(pool);
    let body = shorten(&app, "https://example.com/some/long/path").await;

    assert_eq!(body.code.len(), 7);
    assert_eq!(body.short_url, format!("{BASE_URL}/{}", body.code));
    assert_eq!(body.long_url, "https://example.com/some/long/path");
}

/// The catch-all `/{code}` must not shadow the literal routes.
#[sqlx::test(migrations = "../../migrations")]
async fn literal_routes_win_over_the_shortcode_parameter(pool: PgPool) {
    let app = app(pool);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}

#[sqlx::test(migrations = "../../migrations")]
async fn shortening_twice_returns_the_same_code(pool: PgPool) {
    let app = app(pool);
    let url = "https://example.com/idempotent";

    let first = shorten(&app, url).await;
    let second = shorten(&app, url).await;

    assert_eq!(first.code, second.code);
}

/// `https://a.com` and `https://a.com/` are the same resource. Normalizing before
/// hashing keeps them on one row instead of two shortcodes for one destination.
#[sqlx::test(migrations = "../../migrations")]
async fn urls_differing_only_by_normalization_share_a_code(pool: PgPool) {
    let app = app(pool);

    let bare = shorten(&app, "https://example.com").await;
    let slashed = shorten(&app, "https://example.com/").await;

    assert_eq!(bare.code, slashed.code);
}

#[sqlx::test(migrations = "../../migrations")]
async fn full_roundtrip_redirects_to_the_original_url(pool: PgPool) {
    let app = app(pool);
    let url = "https://example.com/final/destination";
    let body = shorten(&app, url).await;

    let response = app.oneshot(get_code(&body.code)).await.unwrap();

    assert_eq!(response.status(), StatusCode::MOVED_PERMANENTLY);
    assert_eq!(response.headers().get(header::LOCATION).unwrap(), url);
}

/// The scheme check is the difference between a shortener and an XSS vector
/// wearing a trusted domain.
#[sqlx::test(migrations = "../../migrations")]
async fn dangerous_schemes_are_rejected(pool: PgPool) {
    let app = app(pool);

    for url in [
        "javascript:alert(1)",
        "file:///etc/passwd",
        "data:text/html,<script>alert(1)</script>",
        "ftp://example.com/file",
    ] {
        let response = app.clone().oneshot(post_shorten(url)).await.unwrap();
        assert_eq!(
            response.status(),
            StatusCode::BAD_REQUEST,
            "should have rejected {url}"
        );
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn malformed_urls_are_rejected(pool: PgPool) {
    let app = app(pool);

    for url in ["", "not a url", "http://", "://example.com"] {
        let response = app.clone().oneshot(post_shorten(url)).await.unwrap();
        assert_eq!(
            response.status(),
            StatusCode::BAD_REQUEST,
            "should have rejected {url:?}"
        );
    }
}

/// Too long is the client's mistake, so it must be a 400 — never a 500 leaking
/// a database error.
#[sqlx::test(migrations = "../../migrations")]
async fn url_above_the_length_limit_is_a_400_not_a_500(pool: PgPool) {
    let app = app(pool);
    let too_long = format!("https://example.com/{}", "a".repeat(MAX_URL_LEN));

    let response = app.oneshot(post_shorten(&too_long)).await.unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

/// The boundary itself has to be accepted. Written against `MAX_URL_LEN` rather
/// than a literal, so raising the limit cannot leave a test asserting the old
/// one — which is how a limit ends up enforced in two places with two values.
#[sqlx::test(migrations = "../../migrations")]
async fn url_exactly_at_the_length_limit_is_accepted(pool: PgPool) {
    let app = app(pool);
    let prefix = "https://example.com/";
    let at_limit = format!(
        "{prefix}{}",
        "a".repeat(MAX_URL_LEN.saturating_sub(prefix.len()))
    );
    assert_eq!(at_limit.len(), MAX_URL_LEN);

    let response = app.oneshot(post_shorten(&at_limit)).await.unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}

#[sqlx::test(migrations = "../../migrations")]
async fn malformed_json_is_rejected_with_a_problem_body(pool: PgPool) {
    let app = app(pool);
    let request = Request::builder()
        .method("POST")
        .uri("/v1/shorten")
        .header(header::CONTENT_TYPE, "application/json")
        .header("x-forwarded-for", CLIENT_IP)
        .body(Body::from("{ this is not json"))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    // Without taking `JsonRejection` as a value, axum answers in plain text and
    // the client cannot parse the error at all.
    assert_eq!(
        response.headers().get(header::CONTENT_TYPE).unwrap(),
        PROBLEM_JSON
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn unknown_code_is_not_found(pool: PgPool) {
    let app = app(pool);
    let response = app.oneshot(get_code("aaaaaaa")).await.unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

/// A malformed code answers 404, not 400. Splitting the two would let anyone
/// discover the shortcode format from a handful of requests.
///
/// Characters outside the URI set are percent-encoded, which is what a real
/// client sends. `Path` decodes them before the handler sees them, so this
/// exercises the codec rather than the URI parser.
#[sqlx::test(migrations = "../../migrations")]
async fn malformed_code_is_not_found_too(pool: PgPool) {
    let app = app(pool);

    for code in [
        "---",
        "a.b.c",
        "ZZZZZZZZZZZZZZZZ",
        "a%20b",         // "a b"
        "%C3%A7%C3%A3o", // "ção"
    ] {
        let response = app.clone().oneshot(get_code(code)).await.unwrap();
        assert_eq!(
            response.status(),
            StatusCode::NOT_FOUND,
            "should have 404'd on {code:?}"
        );
    }
}

/// Writing is where abuse costs a row, so that is the path with a limit. The
/// redirect stays unlimited on purpose — see `RateLimitSettings`.
#[sqlx::test(migrations = "../../migrations")]
async fn shorten_is_rate_limited_and_the_redirect_is_not(pool: PgPool) {
    let state = Arc::new(AppState {
        db_pool: pool,
        cache: Cache::disabled(),
        // No Redis here, so nobody can be signed in — which is what these tests
        // want: they cover the anonymous surface.
        sessions: SessionStore::disabled(),
        base_url: BASE_URL.to_owned(),
        hasher: Hasher::new(
            HASH_CONCURRENCY,
            std::time::Duration::from_millis(HASH_WAIT_MS),
            Decoy::generate().unwrap(),
        ),
        secure_cookies: true,
        session_ttl_seconds: 3600,
    });
    let app = routes::router(
        state,
        RateLimitSettings {
            shorten_per_second: 1,
            shorten_burst: 2,
            // Left permissive: this test is about the write limit, and a tight
            // login limit here would only add a way for it to fail for the
            // wrong reason.
            login_per_second: 1_000,
            login_burst: 10_000,
            hash_concurrency: HASH_CONCURRENCY,
            hash_wait_ms: HASH_WAIT_MS,
        },
    )
    .unwrap();

    let mut statuses = Vec::new();
    for i in 0..10 {
        let request = post_shorten_from(&format!("https://example.com/burst/{i}"), "198.51.100.1");
        let response = app.clone().oneshot(request).await.unwrap();
        statuses.push(response.status());
    }

    let throttled = statuses
        .iter()
        .filter(|status| **status == StatusCode::TOO_MANY_REQUESTS)
        .count();

    assert!(throttled > 0, "burst of 10 writes was never throttled");

    // A different client is unaffected — the limit is per key, not global. This
    // is what `PeerIpKeyExtractor` would get wrong behind a proxy, where every
    // request carries the proxy's address.
    let other = post_shorten_from("https://example.com/other-client", "198.51.100.2");
    let response = app.clone().oneshot(other).await.unwrap();

    assert_eq!(
        response.status(),
        StatusCode::OK,
        "one client's burst throttled another"
    );

    // Reads are untouched by the limit.
    for _ in 0..10 {
        let response = app.clone().oneshot(get_code("aaaaaaa")).await.unwrap();
        assert_eq!(
            response.status(),
            StatusCode::NOT_FOUND,
            "the redirect must not be rate limited"
        );
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn errors_follow_rfc_9457(pool: PgPool) {
    let app = app(pool);
    let response = app
        .oneshot(post_shorten("javascript:alert(1)"))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        response.headers().get(header::CONTENT_TYPE).unwrap(),
        PROBLEM_JSON
    );

    let problem: ProblemDetails = body_json(response).await;

    assert_eq!(problem.kind, "https://oxid.uk/problems/invalid-url");
    assert_eq!(problem.title, "Invalid URL");
    assert_eq!(problem.status, 400);
    assert!(problem.detail.is_some());
}
