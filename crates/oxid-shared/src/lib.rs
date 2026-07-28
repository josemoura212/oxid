use serde::{Deserialize, Serialize};

/// Media type mandated by RFC 9457 for problem responses.
pub const PROBLEM_JSON: &str = "application/problem+json";

/// Longest URL the service accepts.
///
/// Lives in the shared crate so the two sides cannot disagree: the API rejects
/// past it, the error message quotes it, and the front end can warn before
/// sending anything. The database keeps its own CHECK at the same number as a
/// backstop — belt and braces, not duplication of intent.
///
/// 8192 is not arbitrary. Nothing in this stack was pressed by the old 2048:
/// Cloudflare allows 128 KB of headers, and the unique index is on a 32-byte
/// hash rather than the URL itself. The real ceiling is the *destination*
/// server — nginx answers 414 above roughly 8 KB of request line plus headers,
/// so a longer link would be one that does not work where it points.
pub const MAX_URL_LEN: usize = 8192;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShortenRequest {
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShortenResponse {
    pub code: String,
    pub short_url: String,
    pub long_url: String,
}

/// Error body as defined by [RFC 9457](https://www.rfc-editor.org/rfc/rfc9457).
///
/// Fields carry distinct jobs and should not be collapsed: `title` is stable and
/// safe to switch on, `detail` is human-facing prose about this one occurrence.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProblemDetails {
    /// URI identifying the problem type. Relative references are allowed and
    /// resolve against the request URL, which avoids inventing a domain that
    /// serves nothing. `about:blank` means "no type beyond the status code".
    #[serde(rename = "type", default = "ProblemDetails::blank_type")]
    pub kind: String,

    /// Short, human-readable summary. Must not change from occurrence to
    /// occurrence — that is what makes it safe for clients to match on.
    pub title: String,

    pub status: u16,

    /// Explanation specific to this occurrence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,

    /// URI identifying this specific occurrence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance: Option<String>,
}

impl ProblemDetails {
    fn blank_type() -> String {
        "about:blank".to_owned()
    }

    pub fn new(kind: &str, title: &str, status: u16, detail: String) -> Self {
        Self {
            kind: kind.to_owned(),
            title: title.to_owned(),
            status,
            detail: Some(detail),
            instance: None,
        }
    }

    /// Message worth showing to a person: `detail` when present, `title` otherwise.
    pub fn message(&self) -> &str {
        self.detail.as_deref().unwrap_or(&self.title)
    }
}
