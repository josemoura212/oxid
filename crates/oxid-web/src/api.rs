//! The one call this front end makes.

use gloo_net::http::Request;
use oxid_shared::{ProblemDetails, ShortenRequest, ShortenResponse};

/// Relative path on purpose: trunk proxies it in development, and in production
/// Traefik routes `/v1` to the API on the same origin the page came from. No
/// base URL to configure, and no CORS.
pub async fn shorten(url: String) -> Result<ShortenResponse, String> {
    let response = Request::post("/v1/shorten")
        .json(&ShortenRequest { url })
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;

    let status = response.status();
    let status_text = response.status_text();

    if response.ok() {
        return response
            .json::<ShortenResponse>()
            .await
            .map_err(|e| e.to_string());
    }

    // The error body may not be JSON at all — axum's own 404 comes back empty.
    // Falling back to the status beats leaking "EOF while parsing a value",
    // which tells the reader nothing about what went wrong.
    let error = response.json::<ProblemDetails>().await.map_or_else(
        |_| format!("{status} {status_text}"),
        |problem| problem.message().to_owned(),
    );

    Err(error)
}
