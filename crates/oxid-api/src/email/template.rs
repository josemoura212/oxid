//! The HTML shell every message is poured into.
//!
//! **E-mail is not the web, and this file is where that stops being surprising.**
//! The site's stylesheet cannot be reused at all: Outlook renders through Word,
//! which has no flexbox, no grid, no CSS variables and no `border-radius`; Gmail
//! strips `<style>` blocks in several contexts; and a webfont does not load
//! anywhere worth counting on. So the layout is tables, every rule is inline, and
//! `JetBrains Mono` degrades to whatever monospace the reader has.
//!
//! What survives the translation is the part that carries the identity: the burnt
//! iron page, the oxide accent, the mono wordmark, and the three surface depths.
//! The tokens are duplicated here as literals because there is nowhere to
//! reference them from — if the palette moves, it moves here too.
//!
//! Every message also ships a plain-text part. That is not a fallback nobody
//! sees: a message with no text alternative scores worse with spam filters, and
//! for a domain with no sending reputation yet that is the difference between the
//! inbox and the spam folder.

/// Burnt iron. The same `--page` the site uses.
const PAGE: &str = "#14110e";
/// One step up from the page — `--surface`, the panel a dialog sits on.
const SURFACE: &str = "#1c1815";
const EDGE: &str = "#2e2823";
const FG: &str = "#ece8e3";
const MUTED: &str = "#a29890";
const ACCENT: &str = "#d4693a";
/// What sits *on* the accent. White on `#d4693a` is 3.57:1 and was the contrast
/// failure Lighthouse reported on the site; this is the colour that fixed it.
const ACCENT_INK: &str = "#1a1206";

const MONO: &str = "'JetBrains Mono', ui-monospace, SFMono-Regular, Menlo, Consolas, monospace";
const PROSE: &str =
    "-apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, Helvetica, Arial, sans-serif";

/// A call to action, when the message has one.
pub(super) struct Cta<'a> {
    pub(super) label: &'a str,
    pub(super) url: &'a str,
    /// Shown under the button, in full.
    ///
    /// Buttons get stripped, blocked, or rendered unclickable often enough that a
    /// message whose only route is a button is a message that sometimes cannot be
    /// acted on. The raw URL is the fallback that always works, and for a security
    /// e-mail it is also what lets a careful reader see where they are going
    /// before they go.
    pub(super) raw_hint: &'a str,
}

/// Escapes the five characters that would otherwise close a tag or an attribute.
///
/// Everything interpolated here is either ours or an opaque token, so this is not
/// load-bearing today — but an address or a name will end up in one of these
/// messages eventually, and the moment it does this is the difference between a
/// template and an injection point.
fn escape(raw: &str) -> String {
    raw.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn paragraph(text: &str) -> String {
    format!(
        r#"<p style="margin:0 0 16px;font-family:{PROSE};font-size:15px;line-height:1.6;color:{FG};">{}</p>"#,
        escape(text)
    )
}

/// The button, as a table.
///
/// A styled `<a>` is not clickable across its padding in Outlook — only the text
/// is — so the padding lives on a table cell and the anchor fills it. This is the
/// oldest trick in the file and the one most likely to be "simplified" away by
/// someone who has not opened the result in Outlook.
fn button(cta: &Cta<'_>) -> String {
    format!(
        r#"<table role="presentation" cellpadding="0" cellspacing="0" border="0" style="margin:0 0 16px;">
  <tr>
    <td bgcolor="{ACCENT}" style="background-color:{ACCENT};border-radius:4px;">
      <a href="{url}" style="display:inline-block;padding:14px 28px;font-family:{MONO};font-size:15px;font-weight:600;line-height:1;color:{ACCENT_INK};text-decoration:none;">{label}</a>
    </td>
  </tr>
</table>
<p style="margin:0 0 4px;font-family:{PROSE};font-size:12px;line-height:1.5;color:{MUTED};">{hint}</p>
<p style="margin:0 0 8px;font-family:{MONO};font-size:12px;line-height:1.5;color:{ACCENT};word-break:break-all;">
  <a href="{url}" style="color:{ACCENT};text-decoration:underline;">{url_text}</a>
</p>"#,
        url = escape(cta.url),
        url_text = escape(cta.url),
        label = escape(cta.label),
        hint = escape(cta.raw_hint),
    )
}

/// Wraps the pieces in the shell.
///
/// `preheader` is the line inbox lists show next to the subject. Left out, clients
/// grab whatever text comes first — which here would be the wordmark, so every
/// message would preview as "oxid oxid".
pub(super) fn shell(
    preheader: &str,
    eyebrow: &str,
    title: &str,
    paragraphs: &[&str],
    cta: Option<&Cta<'_>>,
    footer: &str,
) -> String {
    let body: String = paragraphs.iter().map(|text| paragraph(text)).collect();
    let action = cta.map(button).unwrap_or_default();

    format!(
        r#"<!doctype html>
<html lang="pt-BR">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<meta name="color-scheme" content="dark">
<meta name="supported-color-schemes" content="dark">
<title>{title_text}</title>
</head>
<body style="margin:0;padding:0;background-color:{PAGE};">
<div style="display:none;max-height:0;overflow:hidden;opacity:0;">{preheader_text}</div>
<table role="presentation" width="100%" cellpadding="0" cellspacing="0" border="0" bgcolor="{PAGE}" style="background-color:{PAGE};">
  <tr>
    <td align="center" style="padding:32px 16px;">
      <table role="presentation" width="100%" cellpadding="0" cellspacing="0" border="0" style="max-width:520px;">

        <tr>
          <td style="padding:0 0 20px;font-family:{MONO};font-size:17px;font-weight:600;letter-spacing:0.01em;color:{ACCENT};">
            oxid
          </td>
        </tr>

        <tr>
          <td bgcolor="{SURFACE}" style="background-color:{SURFACE};border:1px solid {EDGE};border-radius:10px;padding:28px 28px 24px;">
            <p style="margin:0 0 6px;font-family:{MONO};font-size:11px;font-weight:600;letter-spacing:0.09em;text-transform:uppercase;color:{MUTED};">{eyebrow_text}</p>
            <h1 style="margin:0 0 18px;font-family:{MONO};font-size:19px;font-weight:600;line-height:1.25;color:{FG};">{title_text}</h1>
            {body}
            {action}
          </td>
        </tr>

        <tr>
          <td style="padding:18px 4px 0;font-family:{PROSE};font-size:12px;line-height:1.6;color:{MUTED};">
            {footer_text}
          </td>
        </tr>

      </table>
    </td>
  </tr>
</table>
</body>
</html>"#,
        preheader_text = escape(preheader),
        eyebrow_text = escape(eyebrow),
        title_text = escape(title),
        footer_text = escape(footer),
    )
}

#[cfg(test)]
mod tests {
    use super::{Cta, escape, shell};

    #[test]
    fn interpolated_text_cannot_close_a_tag_or_an_attribute() {
        assert_eq!(escape(r#"<script>"&'"#), "&lt;script&gt;&quot;&amp;&#39;");

        let page = shell(
            "pre",
            "eyebrow",
            "<img src=x onerror=alert(1)>",
            &["body"],
            None,
            "footer",
        );

        assert!(!page.contains("<img"));
        assert!(page.contains("&lt;img"));
    }

    /// The ampersand has to be escaped first, or the escaped forms get escaped
    /// again and `&lt;` renders as literal `&lt;` to the reader.
    #[test]
    fn escaping_does_not_double_up() {
        assert_eq!(escape("a & b < c"), "a &amp; b &lt; c");
    }

    /// A button-only message is one a stripped or blocked button makes
    /// unactionable, so the URL is always written out too.
    #[test]
    fn the_link_is_reachable_without_clicking_the_button() {
        let cta = Cta {
            label: "Confirmar",
            url: "https://oxid.uk/verify-email?token=abc",
            raw_hint: "Ou copie:",
        };
        let page = shell("pre", "eyebrow", "Title", &["body"], Some(&cta), "footer");

        assert_eq!(
            page.matches("https://oxid.uk/verify-email?token=abc")
                .count(),
            3,
            "the href twice and the visible URL once"
        );
        assert!(page.contains("Ou copie:"));
    }

    /// Without it, clients preview the first text in the document — the wordmark —
    /// and every message looks identical in an inbox list.
    #[test]
    fn the_preheader_comes_before_the_wordmark() {
        let page = shell("preview line", "eyebrow", "Title", &[], None, "footer");

        let preheader = page.find("preview line").expect("preheader is rendered");
        let wordmark = page.find("oxid\n").expect("wordmark is rendered");

        assert!(preheader < wordmark);
    }
}
