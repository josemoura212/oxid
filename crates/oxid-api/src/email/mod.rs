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
        }
    }

    pub const fn disabled() -> Self {
        Self::Disabled
    }

    /// Whether messages actually leave the process. Tests assert on this so a
    /// misconfigured suite cannot quietly become a suite that mails people.
    pub const fn is_active(&self) -> bool {
        matches!(self, Self::Resend(_))
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
        }
    }
}
