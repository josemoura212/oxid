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

/// Click analytics for one of the owner's links, over a time window.
///
/// Timestamps are RFC 3339 strings, like [`OwnedLink::created_at`] — the wire
/// contract stays free of a date library, and the front formats them itself.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClickStats {
    pub total: u64,
    /// Distinct visitors — `uniq(visitor_hash)` on the server.
    pub unique: u64,
    pub series: Vec<ClickPoint>,
    /// Where the clicks came from, ranked. Rides along in the same response
    /// rather than on its own endpoint: the dashboard shows it beside the total,
    /// so a second round trip would only buy a second loading state.
    #[serde(default)]
    pub breakdown: ClickBreakdown,
}

/// The ranked dimensions under a link's chart.
///
/// Bots are counted but kept out of the lists. "Top countries" is a question
/// about people, and for a shortener the crawlers are not a rounding error — a
/// link pasted into a group chat is fetched by the platform before anyone opens
/// it. The count stays visible so the exclusion is legible rather than a silent
/// discrepancy between this and `total`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ClickBreakdown {
    pub bots: u64,
    pub countries: Vec<ClickSlice>,
    pub devices: Vec<ClickSlice>,
    pub referrers: Vec<ClickSlice>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClickSlice {
    pub value: String,
    pub clicks: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClickPoint {
    /// Start of the day this bucket counts, RFC 3339.
    pub at: String,
    pub clicks: u64,
}

/// The aggregate screen: every one of the owner's links on one day axis.
///
/// The axis is shared and dense — one entry per day in the window — so each
/// link's `clicks` lines up index-for-index with `days` and the front can draw a
/// line per link without reconciling different date sets. The server fills the
/// gaps with zeros; a day nobody clicked is a zero, not a missing point.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OverviewStats {
    /// Day-starts over the window, RFC 3339, oldest first.
    pub days: Vec<String>,
    /// One line per link, ranked so the busiest come first.
    pub links: Vec<OverviewLink>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OverviewLink {
    pub code: String,
    /// Clicks over the whole window — what the links are ranked by.
    pub total: u64,
    /// Clicks per day, the same length and order as [`OverviewStats::days`].
    pub clicks: Vec<u64>,
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

/// Longest name a token can carry. The field exists so a list of tokens reads as
/// a list of decisions ("laptop", "work phone"); past this it stops being a label
/// and starts being storage.
pub const MAX_TOKEN_NAME_LEN: usize = 60;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateTokenRequest {
    pub name: String,
}

/// A token, as the list shows it. Deliberately without the secret — that exists
/// in exactly one response, [`CreatedToken`], and never again.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiTokenSummary {
    pub id: i64,
    pub name: String,
    /// RFC 3339.
    pub created_at: String,
    /// `None` when the token has never authenticated a request. Absent rather
    /// than a placeholder date, because "never used" is what makes an unfamiliar
    /// token safe to revoke and that has to be unambiguous.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_used_at: Option<String>,
}

/// The one response that carries the secret.
///
/// A separate type from [`ApiTokenSummary`] so the secret cannot leak into a list
/// by someone adding a field: the type that has it is returned by exactly one
/// handler, and the type the list returns has nowhere to put it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreatedToken {
    #[serde(flatten)]
    pub token: ApiTokenSummary,
    /// Shown once. The server keeps only a digest and cannot show it again.
    pub secret: String,
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
