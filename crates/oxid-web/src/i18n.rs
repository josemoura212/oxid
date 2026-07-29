//! Two locales, one catalogue, no dependency.
//!
//! `leptos_i18n` buys Fluent, plurals and interpolation; at eighteen strings and
//! two languages a `match` costs less and reads plainly. The moment real plural
//! rules or date formatting show up, that trade flips — see `ROADMAP.md`,
//! stage 5.3.

use gloo_storage::{LocalStorage, Storage};
use leptos::logging;

/// Separate from the saved-links key, and versioned for the same reason.
const KEY: &str = "oxid.locale.v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Locale {
    En,
    PtBr,
}

impl Locale {
    /// Portuguese, matching what `index.html` ships with. English is served only
    /// to someone whose browser asks for it.
    pub const DEFAULT: Self = Self::PtBr;

    /// The explicit choice wins over the browser. Someone running an English
    /// system who reads Portuguese has no other way out, and detection alone
    /// would trap them.
    pub fn resolve() -> Self {
        Self::stored().unwrap_or_else(Self::from_browser)
    }

    fn stored() -> Option<Self> {
        LocalStorage::get::<String>(KEY)
            .ok()
            .and_then(|tag| Self::from_tag(&tag))
    }

    /// `language()` rather than `languages()`: the first entry is the one the
    /// person put first, and reading the full list would pull in `js_sys` to
    /// walk a JS array for a tie-break that almost never happens.
    ///
    /// Anything the catalogue does not cover lands on Portuguese, which is also
    /// what the static `index.html` declares — so the language never changes
    /// under someone between the first paint and the app mounting.
    fn from_browser() -> Self {
        let Some(window) = web_sys::window() else {
            return Self::DEFAULT;
        };

        window
            .navigator()
            .language()
            .and_then(|tag| Self::from_tag(&tag))
            .unwrap_or(Self::DEFAULT)
    }

    /// Matches on the primary subtag, so `pt`, `pt-BR` and `pt-PT` all land on
    /// Portuguese, and `en-GB` on English. Anything else is `None` and the
    /// caller falls back to [`Self::DEFAULT`].
    fn from_tag(tag: &str) -> Option<Self> {
        let primary = tag.split('-').next()?.to_lowercase();

        match primary.as_str() {
            "pt" => Some(Self::PtBr),
            "en" => Some(Self::En),
            _ => None,
        }
    }

    pub fn remember(self) {
        if let Err(error) = LocalStorage::set(KEY, self.tag()) {
            // Not worth interrupting anyone over: the page still shows the
            // chosen language, it just will not remember it next time.
            logging::warn!("could not store the language preference: {error}");
        }
    }

    pub const fn tag(self) -> &'static str {
        match self {
            Self::En => "en",
            Self::PtBr => "pt-BR",
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::En => "EN",
            Self::PtBr => "PT",
        }
    }

    pub const fn strings(self) -> &'static Strings {
        match self {
            Self::En => &EN,
            Self::PtBr => &PT_BR,
        }
    }

    pub fn tally(self, links: usize, saved: usize) -> String {
        let unit = if links == 1 {
            self.strings().link_one
        } else {
            self.strings().link_many
        };

        format!("{links} {unit} · {saved} {}", self.strings().chars_saved)
    }

    pub fn remove_label(self, code: &str) -> String {
        format!("{} {code}", self.strings().remove_prefix)
    }
}

/// Every piece of text on the page. Keeping them in one struct means a new
/// locale is a compile error until it is complete, instead of a silent fallback
/// to English at runtime.
#[derive(Debug)]
pub struct Strings {
    pub document_title: &'static str,
    pub document_description: &'static str,
    pub thesis_lead: &'static str,
    pub thesis_turn: &'static str,
    pub url_label: &'static str,
    pub url_placeholder: &'static str,
    pub shorten: &'static str,
    pub shortening: &'static str,
    pub copy: &'static str,
    pub copied: &'static str,
    pub vault_title: &'static str,
    pub vault_empty: &'static str,
    pub vault_note: &'static str,
    pub storage_error: &'static str,
    pub link_one: &'static str,
    pub link_many: &'static str,
    pub chars_saved: &'static str,
    pub remove_prefix: &'static str,
    pub language_group: &'static str,
    pub repository: &'static str,

    // Accounts.
    pub sign_in: &'static str,
    pub sign_out: &'static str,
    pub sign_out_all: &'static str,
    pub sign_up: &'static str,
    pub email_label: &'static str,
    pub password_label: &'static str,
    pub password_hint: &'static str,
    pub password_confirm_label: &'static str,
    pub password_mismatch: &'static str,
    pub have_account: &'static str,
    pub no_account: &'static str,
    pub close: &'static str,
    pub working: &'static str,
    pub account_dialog: &'static str,
    pub vault_account_title: &'static str,
    pub vault_account_empty: &'static str,
    /// The sentence that keeps the import from looking like a bug. The codes
    /// created are new, so the list changes addresses after signing up, and
    /// saying so is cheaper than answering the question later.
    pub import_note: &'static str,
    pub load_more: &'static str,
}

static EN: Strings = Strings {
    document_title: "Oxid",
    document_description: "High-performance URL shortener in Rust",
    thesis_lead: "Long links in. ",
    thesis_turn: "Seven characters out.",
    url_label: "Long URL",
    url_placeholder: "https://example.com/a/very/long/url",
    shorten: "Shorten",
    shortening: "Shortening",
    copy: "Copy",
    copied: "Copied",
    vault_title: "In this browser",
    vault_empty: "Nothing here yet. Shorten a link and it stays on this device.",
    vault_note: "Kept in this browser only, never sent to the server. Removing a link here does not disable it — anyone holding it keeps being redirected.",
    storage_error: "This browser refused to save the list. Copy the link before leaving.",
    link_one: "link",
    link_many: "links",
    chars_saved: "characters saved",
    remove_prefix: "Remove from this list:",
    language_group: "Language",
    repository: "Source code on GitHub",

    sign_in: "Sign in",
    sign_out: "Sign out",
    sign_out_all: "Sign out of all devices",
    sign_up: "Create account",
    email_label: "Email",
    password_label: "Password",
    password_hint: "At least 12 characters. Length is the only rule.",
    password_confirm_label: "Confirm password",
    password_mismatch: "The two passwords do not match.",
    have_account: "Already have an account?",
    no_account: "No account yet?",
    close: "Close",
    working: "Working",
    account_dialog: "Account",
    vault_account_title: "In your account",
    vault_account_empty: "No links in this account yet.",
    import_note: "Links saved in this browser were added to your account with new codes. The old links still work.",
    load_more: "Load more",
};

static PT_BR: Strings = Strings {
    document_title: "Oxid",
    document_description: "Encurtador de URL de alta performance em Rust",
    thesis_lead: "Link longo entra. ",
    thesis_turn: "Sete caracteres saem.",
    url_label: "URL longa",
    url_placeholder: "https://exemplo.com/um/caminho/bem/longo",
    shorten: "Encurtar",
    shortening: "Encurtando",
    copy: "Copiar",
    copied: "Copiado",
    vault_title: "Neste navegador",
    vault_empty: "Nada aqui ainda. Encurte um link e ele fica neste aparelho.",
    vault_note: "Guardado só neste navegador, nunca enviado ao servidor. Remover um link daqui não o desativa — quem tiver o link continua sendo redirecionado.",
    storage_error: "Este navegador recusou salvar a lista. Copie o link antes de sair.",
    link_one: "link",
    link_many: "links",
    chars_saved: "caracteres economizados",
    remove_prefix: "Remover desta lista:",
    language_group: "Idioma",
    repository: "Código-fonte no GitHub",

    sign_in: "Entrar",
    sign_out: "Sair",
    sign_out_all: "Sair de todos os dispositivos",
    sign_up: "Criar conta",
    email_label: "E-mail",
    password_label: "Senha",
    password_hint: "No mínimo 12 caracteres. Tamanho é a única regra.",
    password_confirm_label: "Confirmar senha",
    password_mismatch: "As duas senhas não coincidem.",
    have_account: "Já tem conta?",
    no_account: "Ainda não tem conta?",
    close: "Fechar",
    working: "Enviando",
    account_dialog: "Conta",
    vault_account_title: "Na sua conta",
    vault_account_empty: "Nenhum link nesta conta ainda.",
    import_note: "Os links salvos neste navegador entraram na sua conta com códigos novos. Os antigos continuam funcionando.",
    load_more: "Carregar mais",
};

#[cfg(test)]
mod tests {
    use super::Locale;

    #[test]
    fn primary_subtag_decides() {
        assert_eq!(Locale::from_tag("pt-BR"), Some(Locale::PtBr));
        assert_eq!(Locale::from_tag("pt"), Some(Locale::PtBr));
        assert_eq!(Locale::from_tag("PT-pt"), Some(Locale::PtBr));
        assert_eq!(Locale::from_tag("en-GB"), Some(Locale::En));
        assert_eq!(Locale::from_tag("fr"), None);
        assert_eq!(Locale::from_tag(""), None);
    }
}
