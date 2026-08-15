//! Every call this front end makes.
//!
//! Paths are relative on purpose: trunk proxies them in development, and in
//! production Traefik routes `/v1` to the API on the same origin the page came
//! from. No base URL to configure, and no CORS.
//!
//! The session cookie rides along for free, for the same reason — same origin.
//! Nothing here touches the cookie, and nothing can: it is `HttpOnly`.

use gloo_net::http::{Request, Response};
use oxid_shared::{
    AccountResponse, ApiTokenSummary, ClickStats, CreateTokenRequest, CreatedToken,
    CredentialsRequest, EmailRequest, ImportRequest, ImportResponse, LinkPage, OverviewStats,
    ProblemDetails, ResetPasswordRequest, ShortenRequest, ShortenResponse, SignupResponse,
    TokenRequest,
};
use serde::{Serialize, de::DeserializeOwned};

/// Reads a response, turning a failure into the message a person should see.
///
/// The error body may not be JSON at all — axum's own 404 comes back empty.
/// Falling back to the status beats surfacing "EOF while parsing a value", which
/// tells the reader nothing about what went wrong.
async fn read<T: DeserializeOwned>(response: Response) -> Result<T, String> {
    let status = response.status();
    let status_text = response.status_text();

    if response.ok() {
        return response.json::<T>().await.map_err(|e| e.to_string());
    }

    Err(response.json::<ProblemDetails>().await.map_or_else(
        |_| format!("{status} {status_text}"),
        |problem| problem.message().to_owned(),
    ))
}

async fn post<B: Serialize, T: DeserializeOwned>(path: &str, body: &B) -> Result<T, String> {
    let response = Request::post(path)
        .json(body)
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;

    read(response).await
}

/// For the endpoints that answer 204. There is no body to deserialize, so the
/// status is the whole answer — and a failure still carries a problem document
/// worth showing.
async fn post_empty<B: Serialize>(path: &str, body: &B) -> Result<(), String> {
    let response = Request::post(path)
        .json(body)
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;

    status_only(response).await
}

async fn status_only(response: Response) -> Result<(), String> {
    if response.ok() {
        return Ok(());
    }

    let status = response.status();
    let status_text = response.status_text();

    Err(response.json::<ProblemDetails>().await.map_or_else(
        |_| format!("{status} {status_text}"),
        |problem| problem.message().to_owned(),
    ))
}

async fn get<T: DeserializeOwned>(path: &str) -> Result<T, String> {
    let response = Request::get(path).send().await.map_err(|e| e.to_string())?;

    read(response).await
}

pub async fn shorten(url: String) -> Result<ShortenResponse, String> {
    post("/v1/shorten", &ShortenRequest { url }).await
}

/// Creates an account and asks the server to mail a confirmation link.
///
/// Does **not** sign anyone in, and answers the same thing whether or not the
/// address was already registered — see `SignupResponse`. The front end must not
/// try to be more informative than this: reporting "that address is taken" would
/// hand back the enumeration oracle the server just gave up.
pub async fn signup(email: String, password: String) -> Result<SignupResponse, String> {
    post("/v1/signup", &CredentialsRequest { email, password }).await
}

/// Spends a confirmation link.
pub async fn verify_email(token: String) -> Result<(), String> {
    post_empty("/v1/verify-email", &TokenRequest { token }).await
}

/// Asks for another confirmation link. Always succeeds, even for an address with
/// no account.
pub async fn resend_verification(email: String) -> Result<(), String> {
    post_empty("/v1/resend-verification", &EmailRequest { email }).await
}

/// Starts a password reset. Always succeeds, for the same reason.
pub async fn forgot_password(email: String) -> Result<(), String> {
    post_empty("/v1/forgot-password", &EmailRequest { email }).await
}

/// Checks a reset link without spending it — what the reset screen calls on open.
pub async fn check_reset(token: &str) -> Result<(), String> {
    let response = Request::get(&format!(
        "/v1/reset-password?token={}",
        js_sys::encode_uri_component(token)
    ))
    .send()
    .await
    .map_err(|e| e.to_string())?;

    status_only(response).await
}

/// Spends the link and sets the password. Every session ends, including any this
/// browser held.
pub async fn reset_password(token: String, password: String) -> Result<(), String> {
    post_empty(
        "/v1/reset-password",
        &ResetPasswordRequest { token, password },
    )
    .await
}

pub async fn login(email: String, password: String) -> Result<AccountResponse, String> {
    post("/v1/login", &CredentialsRequest { email, password }).await
}

/// Answers 204 with no body, so there is nothing to deserialize — and no reason
/// to surface a failure either: the cookie is cleared by the response itself.
pub async fn logout() -> Result<(), String> {
    Request::post("/v1/logout")
        .send()
        .await
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// Signs out of every device by revoking all of the account's sessions.
///
/// Unlike `logout`, the server surfaces a failure here (500), so this reads the
/// status: telling someone reacting to a compromise that they are signed out
/// everywhere when a revoke failed would be the worst possible lie.
pub async fn logout_all() -> Result<(), String> {
    let response = Request::post("/v1/logout-all")
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() {
        Ok(())
    } else {
        Err(format!("{} {}", response.status(), response.status_text()))
    }
}

/// Who is signed in, or nobody.
///
/// Separate from `/v1/me` so first paint does not have to treat a 401 as an
/// error worth showing to someone who simply never logged in.
pub async fn session() -> Result<Option<i64>, String> {
    get("/v1/session").await
}

pub async fn me() -> Result<AccountResponse, String> {
    get("/v1/me").await
}

pub async fn owned_links(cursor: Option<&str>) -> Result<LinkPage, String> {
    match cursor {
        // The cursor is opaque and server-produced: `<rfc3339>|<code>`. An RFC
        // 3339 UTC timestamp ends in `+00:00`, and a raw `+` in a query string
        // decodes back to a space — so without encoding, `parse_cursor` on the
        // server sees a broken timestamp and every page after the first is a
        // 400. `encodeURIComponent` is the browser's own encoder; a hand-rolled
        // one would need updating the day the cursor format changes.
        Some(cursor) => {
            let encoded = String::from(js_sys::encode_uri_component(cursor));
            get(&format!("/v1/urls?cursor={encoded}")).await
        }
        None => get("/v1/urls").await,
    }
}

/// Brings the browser's saved links into the account.
///
/// One call for the whole list, not one per link: the write limit would throttle
/// a replay of twenty and fail one of fifty partway, which leaves an import that
/// half happened.
pub async fn import(urls: Vec<String>) -> Result<ImportResponse, String> {
    post("/v1/urls/import", &ImportRequest { urls }).await
}

/// Click analytics for one of the account's codes, over the last `days`.
pub async fn link_stats(code: &str, days: u32) -> Result<ClickStats, String> {
    get(&format!("/v1/urls/{code}/stats?days={days}")).await
}

/// Every one of the account's links on one axis, over the last `days`. The
/// aggregate counterpart to [`link_stats`].
pub async fn overview(days: u32) -> Result<OverviewStats, String> {
    get(&format!("/v1/urls/overview?days={days}")).await
}

/// The account's API tokens. Never carries a secret — the server only has digests.
pub async fn list_tokens() -> Result<Vec<ApiTokenSummary>, String> {
    get("/v1/tokens").await
}

/// Mints a token. The response is the one and only time the secret exists
/// outside the caller's hands, which is why it is returned rather than stored.
pub async fn create_token(name: String) -> Result<CreatedToken, String> {
    post("/v1/tokens", &CreateTokenRequest { name }).await
}

/// Revokes a token. Answers 204 with no body, so there is nothing to read — but
/// the status is checked, because telling someone a leaked credential is dead
/// when it is not is the worst possible lie here.
pub async fn revoke_token(id: i64) -> Result<(), String> {
    let response = Request::delete(&format!("/v1/tokens/{id}"))
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() {
        Ok(())
    } else {
        Err(format!("{} {}", response.status(), response.status_text()))
    }
}
