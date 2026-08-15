//! The Cloudflare Email Service transport: one HTTPS POST per message.
//!
//! **No Worker.** The Workers binding is one of three ways into this service, and
//! the REST endpoint is callable from anywhere — which is what lets a Rust process
//! in k3s use it exactly the way it uses Resend. The dashboard leads with the
//! Workers path, and reading only that screen is how this ends up looking like it
//! needs a Worker written for it.
//!
//! The payload is the same four fields Resend takes, so [`Message`] serves both
//! without a shape in between.

use secrecy::{ExposeSecret, SecretString};
use serde::Serialize;

use super::{MailError, Message};

/// How long a send may take before it is abandoned. Same reasoning as the Resend
/// client: this runs inside a request handler, and a provider that hangs would
/// hold a connection.
const TIMEOUT_SECONDS: u64 = 10;

#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
    api_token: SecretString,
    account_id: String,
    from: String,
}

/// Hand-written so the token cannot reach a log through a derived `Debug`.
impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client")
            .field("account_id", &self.account_id)
            .field("from", &self.from)
            .finish_non_exhaustive()
    }
}

#[derive(Serialize)]
struct Payload<'a> {
    from: &'a str,
    to: &'a str,
    subject: &'a str,
    html: &'a str,
    text: &'a str,
}

impl Client {
    pub fn new(api_token: &str, account_id: &str, from: &str) -> Self {
        Self {
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(TIMEOUT_SECONDS))
                .build()
                .unwrap_or_else(|err| {
                    tracing::error!(%err, "falling back to an HTTP client with no timeout");
                    reqwest::Client::default()
                }),
            api_token: SecretString::from(api_token.to_owned()),
            account_id: account_id.to_owned(),
            from: from.to_owned(),
        }
    }

    fn endpoint(&self) -> String {
        format!(
            "https://api.cloudflare.com/client/v4/accounts/{}/email/sending/send",
            self.account_id
        )
    }

    pub async fn send(&self, message: &Message) -> Result<(), MailError> {
        let response = self
            .http
            .post(self.endpoint())
            .bearer_auth(self.api_token.expose_secret())
            .json(&Payload {
                from: &self.from,
                to: &message.to,
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
        // Cloudflare answers with an `errors` array carrying a code and a
        // message. Kept whole rather than parsed: the shapes differ per failure,
        // and "the provider said no" with no reason is the log line that costs an
        // hour later.
        let detail = response.text().await.unwrap_or_default();

        Err(MailError::Refused(format!("{status}: {detail}")))
    }
}
