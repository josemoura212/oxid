//! What each message says, in the language the browser asked for.
//!
//! The copy lives here rather than at the call sites so the three messages can
//! be read against each other. Two of them are anti-enumeration devices as much
//! as they are notifications, and that only works if they are consistent: the
//! HTTP response to "sign up" is identical whether the address is new or taken,
//! and the only thing that differs is which of these arrives.
//!
//! Every message is built twice, as HTML and as plain text. That is not
//! belt-and-braces: a message with no text alternative scores worse with spam
//! filters, and a domain with no sending reputation cannot spare the points.

use super::template::{Cta, shell};

/// Which language a message is written in.
///
/// Taken from `Accept-Language` on the request that triggered the send, not from
/// a column on `users`. A column would be a second source of truth to keep in
/// step with the language switcher on the site, for a guess the browser already
/// makes better — and it would have to be backfilled for every existing account.
///
/// The cost is honest and small: someone who signs up in one browser and asks
/// for a reset in another may get two languages. Nobody is harmed by that.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Lang {
    #[default]
    Pt,
    En,
}

impl Lang {
    /// The primary subtag decides, and anything that is not Portuguese is
    /// English. Matching the full tag would put `pt-PT` and `pt-BR` in different
    /// buckets for copy that is identical in both.
    pub fn from_header(value: Option<&str>) -> Self {
        let Some(value) = value else {
            return Self::Pt;
        };

        let primary = value
            .split(',')
            .next()
            .unwrap_or("")
            .split(';')
            .next()
            .unwrap_or("")
            .split('-')
            .next()
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();

        if primary == "pt" { Self::Pt } else { Self::En }
    }
}

/// One message, ready to hand to a provider.
#[derive(Debug, Clone)]
pub struct Message {
    pub to: String,
    pub subject: String,
    /// The plain-text part. Also what the disabled mailer logs, which is why it
    /// carries the link in full rather than deferring to the HTML.
    pub text: String,
    pub html: String,
}

/// Builds `<site>/#<kind>=<token>` — the site root, token in the **fragment**.
///
/// **The root, not a path, and that is a constraint rather than a preference.**
/// The front end is a single static page with no router, and nginx deliberately
/// refuses to serve `index.html` for unknown paths: a catch-all fallback would
/// answer 200 for a shortcode that reached the wrong service, hiding the
/// misrouting behind a blank page. `/verify-email` would land on the API, match
/// `/{code}`, and 404.
///
/// **The fragment, not the query, and that is a security decision.** A fragment
/// is never sent to a server. In the query, every click on a reset link wrote
/// `GET /?reset=<token>` into the nginx access log in plain text — and a reset
/// token stays valid between the click and the submit, because opening only
/// checks it. Anyone who could read pod logs could take the account over before
/// the person finished typing. It also keeps the token out of the `Referer` of
/// any outbound click and out of the API's own request spans.
fn link(site_url: &str, kind: &str, token: &str) -> String {
    let site = site_url.trim_end_matches('/');
    format!("{site}/#{kind}={token}")
}

/// The line every message ends on. One place, so the three cannot drift.
const fn sign_off(lang: Lang) -> &'static str {
    match lang {
        Lang::Pt => "Se não foi você, ignore esta mensagem.",
        Lang::En => "If this was not you, ignore this message.",
    }
}

const fn raw_hint(lang: Lang) -> &'static str {
    match lang {
        Lang::Pt => "Se o botão não funcionar, copie este endereço:",
        Lang::En => "If the button does not work, copy this address:",
    }
}

/// The plain-text twin of the HTML, assembled from the same pieces so the two
/// say the same thing.
fn as_text(paragraphs: &[&str], url: Option<&str>, lang: Lang) -> String {
    let mut text = paragraphs.join("\n\n");

    if let Some(url) = url {
        text.push_str("\n\n");
        text.push_str(url);
    }

    text.push_str("\n\n");
    text.push_str(sign_off(lang));
    text.push_str("\n\n— oxid");
    text
}

impl Message {
    /// The confirmation link for a freshly created account.
    pub fn confirm(to: &str, site_url: &str, token: &str, lang: Lang) -> Self {
        let url = link(site_url, "verify", token);

        let (subject, eyebrow, title, body, label) = match lang {
            Lang::Pt => (
                "Confirme seu e-mail no oxid",
                "Conta",
                "Confirme seu e-mail",
                "Sua conta no oxid foi criada. Confirme este endereço para poder entrar.",
                "Confirmar e-mail",
            ),
            Lang::En => (
                "Confirm your email on oxid",
                "Account",
                "Confirm your email",
                "Your oxid account was created. Confirm this address so you can sign in.",
                "Confirm email",
            ),
        };

        Self {
            to: to.to_owned(),
            subject: subject.to_owned(),
            text: as_text(&[body], Some(&url), lang),
            html: shell(
                body,
                eyebrow,
                title,
                &[body],
                Some(&Cta {
                    label,
                    url: &url,
                    raw_hint: raw_hint(lang),
                }),
                sign_off(lang),
            ),
        }
    }

    /// Sent when someone tries to sign up with an address that already has an
    /// account.
    ///
    /// **This message is the anti-enumeration mechanism, not a courtesy.** The
    /// signup answers the same 200 either way, so the only thing that reveals
    /// which case it was is this — and it travels through the one channel the
    /// attacker does not control. It deliberately carries no link: there is
    /// nothing to confirm, and a "this was not me" link would be a new endpoint
    /// reachable by anyone holding an address.
    pub fn already_registered(to: &str, site_url: &str, lang: Lang) -> Self {
        let site = site_url.trim_end_matches('/');

        let (subject, eyebrow, title, first, second) = match lang {
            Lang::Pt => (
                "Alguém tentou criar uma conta com seu e-mail",
                "Conta",
                "Este endereço já tem conta",
                "Alguém tentou se cadastrar no oxid com este endereço.".to_owned(),
                format!(
                    "Nenhuma conta nova foi criada e nada mudou na sua. Se foi você, entre \
                     normalmente em {site} — ou use \"esqueci minha senha\" se não lembra dela."
                ),
            ),
            Lang::En => (
                "Someone tried to create an account with your email",
                "Account",
                "This address already has an account",
                "Someone tried to sign up to oxid with this address.".to_owned(),
                format!(
                    "No new account was created and nothing changed on yours. If it was you, \
                     sign in at {site} — or use \"forgot my password\" if you do not remember it."
                ),
            ),
        };

        let paragraphs = [first.as_str(), second.as_str()];

        Self {
            to: to.to_owned(),
            subject: subject.to_owned(),
            text: as_text(&paragraphs, None, lang),
            html: shell(&first, eyebrow, title, &paragraphs, None, sign_off(lang)),
        }
    }

    /// The password reset link.
    ///
    /// Says how long it lasts and what completing it ends. A link that has quietly
    /// expired is indistinguishable from one that never worked, and the difference
    /// decides whether someone asks for another or gives up.
    pub fn reset(to: &str, site_url: &str, token: &str, hours: u64, lang: Lang) -> Self {
        let url = link(site_url, "reset", token);

        let (subject, eyebrow, title, first, second, label) = match lang {
            Lang::Pt => (
                "Redefinir sua senha no oxid",
                "Segurança",
                "Redefinir sua senha",
                "Use o botão abaixo para definir uma nova senha.".to_owned(),
                format!(
                    "O link vale por {hours} horas e só pode ser usado uma vez. Ao concluir, \
                     todas as suas sessões serão encerradas."
                ),
                "Definir nova senha",
            ),
            Lang::En => (
                "Reset your oxid password",
                "Security",
                "Reset your password",
                "Use the button below to set a new password.".to_owned(),
                format!(
                    "The link lasts {hours} hours and can only be used once. When you finish, \
                     every one of your sessions ends."
                ),
                "Set a new password",
            ),
        };

        let paragraphs = [first.as_str(), second.as_str()];

        Self {
            to: to.to_owned(),
            subject: subject.to_owned(),
            text: as_text(&paragraphs, Some(&url), lang),
            html: shell(
                &first,
                eyebrow,
                title,
                &paragraphs,
                Some(&Cta {
                    label,
                    url: &url,
                    raw_hint: raw_hint(lang),
                }),
                sign_off(lang),
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Lang, Message};

    #[test]
    fn the_primary_subtag_decides_and_portuguese_is_the_default() {
        assert_eq!(Lang::from_header(None), Lang::Pt);
        assert_eq!(Lang::from_header(Some("pt-BR,pt;q=0.9")), Lang::Pt);
        assert_eq!(Lang::from_header(Some("pt-PT")), Lang::Pt);
        assert_eq!(Lang::from_header(Some("PT")), Lang::Pt);
        assert_eq!(Lang::from_header(Some("en-US,en;q=0.9")), Lang::En);
        assert_eq!(Lang::from_header(Some("de")), Lang::En);
        assert_eq!(Lang::from_header(Some("")), Lang::En);
    }

    #[test]
    fn a_trailing_slash_on_the_site_does_not_double_up() {
        let with = Message::confirm("a@b.test", "https://oxid.uk/", "tok", Lang::Pt);
        let without = Message::confirm("a@b.test", "https://oxid.uk", "tok", Lang::Pt);

        assert!(with.text.contains("https://oxid.uk/#verify=tok"));
        assert_eq!(with.text, without.text);
    }

    /// The whole anti-enumeration design rests on this message existing and
    /// carrying no way to act on it.
    #[test]
    fn the_already_registered_notice_offers_no_link_to_click() {
        let message = Message::already_registered("a@b.test", "https://oxid.uk", Lang::Pt);

        for part in [&message.text, &message.html] {
            assert!(!part.contains("#verify="));
            assert!(!part.contains("#reset="));
        }
    }

    #[test]
    fn the_reset_says_how_long_it_lasts_and_what_it_ends() {
        let message = Message::reset("a@b.test", "https://oxid.uk", "tok", 2, Lang::Pt);

        assert!(message.text.contains("2 horas"));
        assert!(message.text.contains("sessões serão encerradas"));

        let english = Message::reset("a@b.test", "https://oxid.uk", "tok", 2, Lang::En);
        assert!(english.text.contains("2 hours"));
    }

    /// Both parts carry the link, because which one the reader sees is not ours
    /// to decide — and a text part that says "see the HTML" is useless in the
    /// disabled mailer's log, which is how the flow is tested by hand.
    #[test]
    fn the_link_is_in_the_text_part_as_well_as_the_html() {
        for (message, query) in [
            (
                Message::confirm("a@b.test", "https://oxid.uk", "tok", Lang::Pt),
                "#verify=tok",
            ),
            (
                Message::reset("a@b.test", "https://oxid.uk", "tok", 2, Lang::Pt),
                "#reset=tok",
            ),
        ] {
            assert!(message.text.contains(query), "{}", message.subject);
            assert!(message.html.contains(query), "{}", message.subject);
        }
    }

    #[test]
    fn every_message_tells_the_reader_what_to_do_if_it_was_not_them() {
        let site = "https://oxid.uk";
        for lang in [Lang::Pt, Lang::En] {
            for message in [
                Message::confirm("a@b.test", site, "tok", lang),
                Message::already_registered("a@b.test", site, lang),
                Message::reset("a@b.test", site, "tok", 2, lang),
            ] {
                let expected = if lang == Lang::Pt {
                    "Se não foi você"
                } else {
                    "If this was not you"
                };
                assert!(
                    message.text.contains(expected) && message.html.contains(expected),
                    "missing sign-off: {}",
                    message.subject
                );
            }
        }
    }
}
