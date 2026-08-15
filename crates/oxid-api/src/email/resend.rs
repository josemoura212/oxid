//! The Resend transport: one HTTPS POST per message.
//!
//! No SDK. The API this uses is a single endpoint taking four fields, and a
//! crate for that would be more surface to keep current than the request it
//! replaces.

use secrecy::{ExposeSecret, SecretString};
use serde::Serialize;

use super::{MailError, Message};

const ENDPOINT: &str = "https://api.resend.com/emails";

/// How long a send may take before it is abandoned.
///
/// Bounded because this runs inside a request handler: the account is already
/// created and the response is waiting on it. A provider that hangs would hold a
/// connection and make signup look broken over something the person can retry
/// from the resend button.
const TIMEOUT_SECONDS: u64 = 10;

#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
    api_key: SecretString,
    from: String,
}

/// Hand-written so the key cannot reach a log through a derived `Debug`.
/// `missing_debug_implementations` is denied workspace-wide, so the choice is
/// between writing this and leaking the credential.
impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client")
            .field("from", &self.from)
            .finish_non_exhaustive()
    }
}

/// Both parts, always.
///
/// A multipart message with a text alternative scores better with spam filters
/// than HTML alone, and for a domain with no sending reputation that margin is
/// the inbox. It is also what a reader on a client that refuses HTML sees.
#[derive(Serialize)]
struct Payload<'a> {
    from: &'a str,
    to: [&'a str; 1],
    subject: &'a str,
    html: &'a str,
    text: &'a str,
}

impl Client {
    pub fn new(api_key: &str, from: &str) -> Self {
        Self {
            // Built once and cloned: a `reqwest::Client` owns the connection pool,
            // and constructing one per message would open a fresh TLS session
            // every time.
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(TIMEOUT_SECONDS))
                .build()
                .unwrap_or_else(|err| {
                    // The default client has **no timeout**, so falling back
                    // silently would trade the bounded send this type promises
                    // for one that can hang forever — inside a request handler.
                    // Building a client only fails on a broken TLS backend, which
                    // is a deployment problem worth a loud line rather than a
                    // quiet downgrade.
                    tracing::error!(%err, "falling back to an HTTP client with no timeout");
                    reqwest::Client::default()
                }),
            api_key: SecretString::from(api_key.to_owned()),
            from: from.to_owned(),
        }
    }

    pub async fn send(&self, message: &Message) -> Result<(), MailError> {
        let response = self
            .http
            .post(ENDPOINT)
            .bearer_auth(self.api_key.expose_secret())
            .json(&Payload {
                from: &self.from,
                to: [&message.to],
                subject: &message.subject,
                html: &message.html,
                text: &message.text,
            })
            .send()
            .await?;

        if response.status().is_success() {
            return Ok(());
        }

        let status = response.status();
        // The body carries the reason — an unverified sending domain, a
        // malformed address, a spent quota. Kept, because "the provider said no"
        // with no reason is the kind of log line that costs an hour later.
        let detail = response.text().await.unwrap_or_default();

        Err(MailError::Refused(format!("{status}: {detail}")))
    }
}
