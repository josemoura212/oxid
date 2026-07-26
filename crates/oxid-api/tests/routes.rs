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
use oxid::{routes, state::AppState};
use oxid_shared::{PROBLEM_JSON, ProblemDetails, ShortenResponse};
use serde_json::json;
use sqlx::PgPool;
use tower::ServiceExt;

const BASE_URL: &str = "https://oxid.test";

fn app(pool: PgPool) -> Router {
    routes::router(Arc::new(AppState {
        db_pool: pool,
        base_url: BASE_URL.to_owned(),
    }))
}

fn post_shorten(url: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri("/v1/shorten")
        .header(header::CONTENT_TYPE, "application/json")
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

/// The 2048-char CHECK lives in Postgres, so this arrives as a database error.
/// It must surface as 400 — the client sent something invalid, the server is fine.
#[sqlx::test(migrations = "../../migrations")]
async fn url_above_the_length_limit_is_a_400_not_a_500(pool: PgPool) {
    let app = app(pool);
    let too_long = format!("https://example.com/{}", "a".repeat(2100));

    let response = app.oneshot(post_shorten(&too_long)).await.unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[sqlx::test(migrations = "../../migrations")]
async fn malformed_json_is_rejected_with_a_problem_body(pool: PgPool) {
    let app = app(pool);
    let request = Request::builder()
        .method("POST")
        .uri("/v1/shorten")
        .header(header::CONTENT_TYPE, "application/json")
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
