use axum::{
    Json,
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use oxid_shared::{PROBLEM_JSON, ProblemDetails};
use thiserror::Error;

/// Postgres `check_violation`. The only CHECK on `urls` is the 2048-char limit on
/// `long_url`, so hitting it means the client sent something too long — a 400,
/// not a 500.
const CHECK_VIOLATION: &str = "23514";

#[derive(Debug, Error)]
pub enum AppError {
    #[error("{0}")]
    InvalidUrl(&'static str),

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
            Self::InvalidUrl(_) | Self::InvalidBody(_) => StatusCode::BAD_REQUEST,
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
            Self::Database(err) if is_check_violation(err) => {
                "url exceeds the maximum length of 2048 characters".to_owned()
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
