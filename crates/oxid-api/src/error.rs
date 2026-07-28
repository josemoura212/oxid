use axum::{
    Json,
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use oxid_shared::{MAX_URL_LEN, PROBLEM_JSON, ProblemDetails};
use thiserror::Error;

/// Postgres `check_violation`. The only CHECK on `urls` is the length limit on
/// `long_url`, so hitting it means the client sent something too long — a 400,
/// not a 500.
///
/// The route rejects oversized URLs before they reach Postgres, so this path is
/// now the backstop rather than the usual one: it still fires for anything that
/// writes to the table without going through the handler.
const CHECK_VIOLATION: &str = "23514";

#[derive(Debug, Error)]
pub enum AppError {
    #[error("{0}")]
    InvalidUrl(&'static str),

    #[error("url exceeds the maximum length of {MAX_URL_LEN} characters")]
    UrlTooLong,

    #[error("malformed request body: {0}")]
    InvalidBody(String),

    #[error("shortcode not found")]
    NotFound,

    #[error(transparent)]
    Database(#[from] sqlx::Error),

    #[error("{0}")]
    Internal(&'static str),
}

impl AppError {
    fn status(&self) -> StatusCode {
        match self {
            Self::InvalidUrl(_) | Self::InvalidBody(_) | Self::UrlTooLong => {
                StatusCode::BAD_REQUEST
            }
            Self::NotFound => StatusCode::NOT_FOUND,
            Self::Database(err) if is_check_violation(err) => StatusCode::BAD_REQUEST,
            Self::Database(_) | Self::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    /// Absolute URI, not relative. A relative reference resolves against the
    /// request URL, so the same failure would carry a different identifier per
    /// environment and clients matching on `type` would break when the host
    /// changes. These are stable identifiers, not links that must resolve.
    fn kind(&self) -> &'static str {
        match self {
            Self::InvalidUrl(_) => "https://oxid.uk/problems/invalid-url",
            Self::InvalidBody(_) => "https://oxid.uk/problems/invalid-body",
            Self::NotFound => "https://oxid.uk/problems/not-found",
            // Same identifier whether the length was caught in the handler or by
            // the database: from the client's side it is one failure, and the
            // `type` is what clients match on.
            Self::UrlTooLong => "https://oxid.uk/problems/url-too-long",
            Self::Database(err) if is_check_violation(err) => {
                "https://oxid.uk/problems/url-too-long"
            }
            Self::Database(_) | Self::Internal(_) => "https://oxid.uk/problems/internal",
        }
    }

    /// Stable across occurrences — clients may match on it.
    fn title(&self) -> &'static str {
        match self {
            Self::InvalidUrl(_) => "Invalid URL",
            Self::InvalidBody(_) => "Invalid request body",
            Self::NotFound => "Shortcode not found",
            Self::UrlTooLong => "URL too long",
            Self::Database(err) if is_check_violation(err) => "URL too long",
            Self::Database(_) | Self::Internal(_) => "Internal error",
        }
    }

    /// What the client is allowed to see. Never the `sqlx::Error` itself: it
    /// carries table, column and constraint names — a map of the schema handed
    /// to whoever probes the API.
    fn detail(&self) -> String {
        match self {
            Self::InvalidUrl(msg) | Self::Internal(msg) => (*msg).to_owned(),
            Self::InvalidBody(msg) => msg.clone(),
            Self::NotFound => "no url is registered under this shortcode".to_owned(),
            // Both spellings of the same failure quote `MAX_URL_LEN`, so the
            // number in the message cannot drift away from the number enforced.
            Self::UrlTooLong => self.to_string(),
            Self::Database(err) if is_check_violation(err) => {
                format!("url exceeds the maximum length of {MAX_URL_LEN} characters")
            }
            Self::Database(_) => "the request could not be completed".to_owned(),
        }
    }
}

fn is_check_violation(err: &sqlx::Error) -> bool {
    err.as_database_error()
        .and_then(sqlx::error::DatabaseError::code)
        .is_some_and(|code| code == CHECK_VIOLATION)
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let status = self.status();

        // Logged and returned messages are deliberately different: the log gets
        // the full error, the client gets the sanitized version.
        if status == StatusCode::INTERNAL_SERVER_ERROR {
            tracing::error!(error = ?self, "request failed");
        } else {
            tracing::debug!(error = %self, %status, "request rejected");
        }

        let problem =
            ProblemDetails::new(self.kind(), self.title(), status.as_u16(), self.detail());
        let mut response = (status, Json(problem)).into_response();

        // RFC 9457 requires this media type; `Json` would set `application/json`.
        response
            .headers_mut()
            .insert(header::CONTENT_TYPE, HeaderValue::from_static(PROBLEM_JSON));

        response
    }
}
