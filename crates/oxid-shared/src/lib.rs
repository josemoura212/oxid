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

/// Shortest password the service accepts.
///
/// Length is the only rule. Composition requirements — a digit, a symbol, mixed
/// case — push people towards `Password1!` and shrink the search space an
/// attacker has to cover, which is the opposite of the intent.
pub const MIN_PASSWORD_LEN: usize = 12;

/// Longest password accepted, so a multi-megabyte body cannot turn one request
/// into unbounded Argon2 work. Well past anything a person or a manager types.
pub const MAX_PASSWORD_LEN: usize = 256;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CredentialsRequest {
    pub email: String,
    pub password: String,
}

/// What the client learns about the signed-in account. No password material,
/// not even the hash — this crosses the wire.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountResponse {
    pub id: i64,
    pub email: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OwnedLink {
    pub code: String,
    pub short_url: String,
    pub long_url: String,
    /// RFC 3339. Doubles as the pagination cursor, alongside `code`.
    pub created_at: String,
}

/// One page of an owner's links.
///
/// `next_cursor` is `None` when the last page has been reached. Clients should
/// treat it as opaque and echo it back — its shape is the server's business.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LinkPage {
    pub links: Vec<OwnedLink>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

/// Most URLs one import call will take.
///
/// Bounded because the alternative is a single request that walks an unbounded
/// list, holding a database connection from a pool of eight while it does. The
/// browser splits a longer list across calls.
pub const MAX_IMPORT: usize = 100;

/// Brings links saved in the browser into the signed-in account.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportRequest {
    pub urls: Vec<String>,
}

/// What the import produced.
///
/// Counts rather than the links themselves: the caller reloads the list right
/// afterwards, and returning rows here would mean inventing a `created_at` for
/// entries that may have already existed.
///
/// The codes created are **new**. A code is unique per owner, and the anonymous
/// one is shared by everybody who shortened that URL without an account —
/// claiming it would take the link away from them. The old links keep working
/// and these are additional, which is the part a caller has to surface rather
/// than hide.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportResponse {
    pub imported: usize,
    /// URLs the server refused. No per-entry reason: the client knows what it
    /// sent, and a list of excuses is noise on a screen nobody reads twice.
    pub rejected: usize,
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
