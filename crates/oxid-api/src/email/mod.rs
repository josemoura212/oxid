//! Outbound e-mail, on Resend.
//!
//! Two messages, and both of them are the product's only way of proving an
//! address belongs to whoever typed it: a confirmation link, and a password
//! reset. Everything else about the account works without either.
//!
//! The shape mirrors [`crate::cache::Cache`] and [`crate::analytics::ClickSink`]
//! — an enum with a disabled variant rather than a trait object. It is the same
//! problem all three have: an outbound dependency that must be absent in
//! development and in tests, where a real send costs provider quota to deliver a
//! link nobody reads.
//!
//! **A send failing must never fail the operation it accompanies.** The account
//! is created first and mailed afterwards; a provider outage becomes "ask for
//! another link", not "signup is down". Every function here returns `Result` so
//! the caller can log it, and every caller logs rather than propagates.

mod cloudflare;
mod message;
mod resend;
mod template;

pub use message::{Lang, Message};

use crate::configuration::{EmailBackend, EmailSettings};

/// How a link is delivered, or that it is not.
#[derive(Debug, Clone)]
pub enum Mailer {
    /// Logs the message instead of sending it, link included.
    ///
    /// The link is what makes this useful rather than merely inert: a developer
    /// completes the whole confirmation flow locally by copying it out of the
    /// log, with no provider account and no key. An implementation that dropped
    /// the message silently would leave the flow untestable by hand.
    Disabled,
    Resend(resend::Client),
    /// Cloudflare Email Service, over its REST endpoint. Same four fields, same
    /// one call per message — no Worker involved.
    Cloudflare(cloudflare::Client),
}

#[derive(Debug, thiserror::Error)]
pub enum MailError {
    #[error("the mail provider refused the message: {0}")]
    Refused(String),
    #[error("could not reach the mail provider")]
    Unreachable(#[from] reqwest::Error),
}

impl Mailer {
    /// Builds the mailer the settings ask for.
    ///
    /// Unlike the cache and the click sink, this does not degrade to disabled
    /// when the provider looks unreachable: there is nothing to connect to at
    /// startup — Resend is one HTTPS call per message — so an unusable key is
    /// only discoverable at send time. Silently downgrading would mean a
    /// production deploy that believes it is mailing and is not.
    pub fn new(settings: &EmailSettings) -> Self {
        match settings.backend {
            EmailBackend::Off => Self::Disabled,
            EmailBackend::Resend => Self::Resend(resend::Client::new(
                settings.resend.api_key(),
                &settings.resend.from,
            )),
            EmailBackend::Cloudflare => Self::Cloudflare(cloudflare::Client::new(
                settings.cloudflare.api_token(),
                &settings.cloudflare.account_id,
                &settings.cloudflare.from,
            )),
        }
    }

    pub const fn disabled() -> Self {
        Self::Disabled
    }

    /// Whether messages actually leave the process. Tests assert on this so a
    /// misconfigured suite cannot quietly become a suite that mails people.
    pub const fn is_active(&self) -> bool {
        matches!(self, Self::Resend(_) | Self::Cloudflare(_))
    }

    /// Which provider is wired, for a log line at boot. `None` when nothing is.
    pub const fn provider(&self) -> Option<&'static str> {
        match self {
            Self::Disabled => None,
            Self::Resend(_) => Some("resend"),
            Self::Cloudflare(_) => Some("cloudflare"),
        }
    }

    pub async fn send(&self, message: &Message) -> Result<(), MailError> {
        match self {
            Self::Disabled => {
                // Deliberately at info: this *is* the delivery in development,
                // and a developer looking for their confirmation link should not
                // have to raise the log level to find it.
                tracing::info!(
                    to = %message.to,
                    subject = %message.subject,
                    "email not sent (mailer disabled); body follows:\n{}",
                    message.text
                );
                Ok(())
            }
            Self::Resend(client) => client.send(message).await,
            Self::Cloudflare(client) => client.send(message).await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Lang, Mailer, Message};
    use crate::configuration::{CloudflareSettings, EmailBackend, EmailSettings, ResendSettings};

    fn settings(backend: EmailBackend) -> EmailSettings {
        EmailSettings {
            backend,
            require_confirmation: true,
            site_url: "https://oxid.uk".to_owned(),
            resend: ResendSettings {
                api_key: "placeholder".to_owned().into(),
                from: "oxid <no-reply@oxid.uk>".to_owned(),
            },
            cloudflare: CloudflareSettings {
                api_token: "placeholder".to_owned().into(),
                account_id: "abc123".to_owned(),
                from: "oxid <no-reply@oxid.uk>".to_owned(),
            },
        }
    }

    /// The backend in the configuration decides which client is built. Getting
    /// this wrong means a deploy that believes it switched provider and did not.
    #[test]
    fn the_configured_backend_is_the_one_built() {
        assert_eq!(Mailer::new(&settings(EmailBackend::Off)).provider(), None);
        assert_eq!(
            Mailer::new(&settings(EmailBackend::Resend)).provider(),
            Some("resend")
        );
        assert_eq!(
            Mailer::new(&settings(EmailBackend::Cloudflare)).provider(),
            Some("cloudflare")
        );
    }

    /// `is_active` is what the test suite asserts on to prove it never mails
    /// anyone, so it has to be true for every provider and false only for off.
    #[test]
    fn only_the_disabled_mailer_is_inactive() {
        assert!(!Mailer::disabled().is_active());
        assert!(Mailer::new(&settings(EmailBackend::Resend)).is_active());
        assert!(Mailer::new(&settings(EmailBackend::Cloudflare)).is_active());
    }

    /// Disabled has to *say* the message, link included: it is the only way the
    /// confirmation flow stays completable on a laptop with no provider account,
    /// and the configuration validator leans on that being true.
    #[tokio::test]
    async fn the_disabled_mailer_accepts_everything_and_sends_nothing() {
        let mailer = Mailer::disabled();
        let message = Message::confirm("a@b.test", "https://oxid.uk", "tok", Lang::Pt);

        assert!(mailer.send(&message).await.is_ok());
        assert!(!mailer.is_active());
    }
}
