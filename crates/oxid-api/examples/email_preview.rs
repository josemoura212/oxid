//! Writes every message to `target/email-preview/` so the template can be opened
//! in a browser — and forwarded to a real client — without sending anything.
//!
//! ```text
//! cargo run -p oxid-api --example email_preview
//! ```
//!
//! An example rather than a test because the output is meant to be looked at.
//! What a test can assert about an e-mail template is that the link is present
//! and the escaping holds, which the unit tests do; whether it renders is a
//! question only Outlook can answer.

use oxid::email::{Lang, Message};

fn main() -> std::io::Result<()> {
    let out = std::path::Path::new("target/email-preview");
    std::fs::create_dir_all(out)?;

    let site = "https://oxid.uk";
    // Spelled out rather than random-looking: a preview token that reads like a
    // credential trips secret scanners, and this one is about to be committed.
    let token = "TOKEN-DE-EXEMPLO-NAO-E-SEGREDO";

    let messages = [
        (
            "confirm-pt",
            Message::confirm("voce@exemplo.com", site, token, Lang::Pt),
        ),
        (
            "confirm-en",
            Message::confirm("you@example.com", site, token, Lang::En),
        ),
        (
            "already-registered-pt",
            Message::already_registered("voce@exemplo.com", site, Lang::Pt),
        ),
        (
            "reset-pt",
            Message::reset("voce@exemplo.com", site, token, 2, Lang::Pt),
        ),
        (
            "reset-en",
            Message::reset("you@example.com", site, token, 2, Lang::En),
        ),
    ];

    for (name, message) in messages {
        std::fs::write(out.join(format!("{name}.html")), &message.html)?;
        std::fs::write(out.join(format!("{name}.txt")), &message.text)?;
        println!("{name}  —  {}", message.subject);
    }

    println!("\nwrote {}", out.display());
    Ok(())
}
