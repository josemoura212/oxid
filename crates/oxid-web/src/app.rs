use leptos::prelude::*;
use oxid_shared::ShortenResponse;

use crate::{
    account::{Account, AccountButton, AccountDialog, AccountVault},
    api,
    i18n::Locale,
    storage::{self, SavedLink},
};

/// Length of every code the API issues. The meter shows it as the target from
/// the first keystroke, so the page states the promise instead of waiting for a
/// response to reveal it.
const CODE_LEN: usize = 7;

#[component]
pub fn App() -> impl IntoView {
    let locale = RwSignal::new(Locale::resolve());
    let links = RwSignal::new(storage::load());
    // A flag, not a message: the text has to follow the language, and a string
    // stored at failure time would stay in whatever locale was active then.
    let storage_failed = RwSignal::new(false);

    let account = Account::new();
    let dialog_open = RwSignal::new(false);

    // The served HTML is always `lang="en"` — it is a static file. Screen
    // readers pick pronunciation from this attribute, so it has to be corrected
    // once the app knows better.
    Effect::new(move |_| apply_to_document(locale.get()));

    // Asks once, on mount, whether there is a session. Cheap enough not to
    // matter and the only way to render the right button on first paint —
    // guessing "signed out" would make it flicker for whoever is signed in.
    Effect::new(move |_| {
        leptos::task::spawn_local(async move {
            match api::session().await {
                Ok(user) => {
                    account.user.set(Some(user));
                    if user.is_some() {
                        account.reload().await;
                    }
                }
                // Treated as anonymous: the endpoint being unreachable is not a
                // reason to block a page whose main feature needs no account.
                Err(error) => {
                    leptos::logging::warn!("could not read the session: {error}");
                    account.user.set(Some(None));
                }
            }
        });
    });

    view! {
        <header class="topbar">
            <div class="topbar-inner">
                <span class="wordmark">"Oxid"</span>
                <nav class="topbar-actions">
                    <LanguagePicker locale=locale />
                    <a
                        class="icon-link"
                        href="https://github.com/josemoura212/oxid"
                        target="_blank"
                        rel="noreferrer"
                        aria-label=move || locale.get().strings().repository
                        title=move || locale.get().strings().repository
                    >
                        <GithubMark />
                    </a>
                    <AccountButton account=account locale=locale.into() open=dialog_open />
                </nav>
            </div>
        </header>

        <div class="shell">
            <Shortener links=links storage_failed=storage_failed locale=locale.into() />

            // One list at a time. Showing both would leave the same URL on
            // screen twice under different codes, which is accurate and
            // useless — the browser list stays intact underneath either way.
            <Show
                when=move || account.signed_in()
                fallback=move || {
                    view! {
                        <Vault
                            links=links
                            storage_failed=storage_failed
                            locale=locale.into()
                        />
                    }
                }
            >
                <AccountVault account=account locale=locale.into() />
            </Show>
        </div>

        <AccountDialog
            account=account
            locale=locale.into()
            open=dialog_open
            saved=links
        />
    }
}

/// Inlined rather than fetched: a strict CSP is on the roadmap, and one 16×16
/// path is cheaper than an extra request plus whatever a sprite sheet would
/// bring with it.
#[component]
fn GithubMark() -> impl IntoView {
    view! {
        <svg
            class="icon"
            viewBox="0 0 16 16"
            width="16"
            height="16"
            fill="currentColor"
            aria-hidden="true"
            focusable="false"
        >
            <path d="M8 0C3.58 0 0 3.58 0 8c0 3.54 2.29 6.53 5.47 7.59.4.07.55-.17.55-.38 0-.19-.01-.82-.01-1.49-2.01.37-2.53-.49-2.69-.94-.09-.23-.48-.94-.82-1.13-.28-.15-.68-.52-.01-.53.63-.01 1.08.58 1.23.82.72 1.21 1.87.87 2.33.66.07-.52.28-.87.51-1.07-1.78-.2-3.64-.89-3.64-3.95 0-.87.31-1.59.82-2.15-.08-.2-.36-1.02.08-2.12 0 0 .67-.21 2.2.82.64-.18 1.32-.27 2-.27s1.36.09 2 .27c1.53-1.04 2.2-.82 2.2-.82.44 1.1.16 1.92.08 2.12.51.56.82 1.27.82 2.15 0 3.07-1.87 3.75-3.65 3.95.29.25.54.73.54 1.48 0 1.07-.01 1.93-.01 2.2 0 .21.15.46.55.38A8.012 8.012 0 0 0 16 8c0-4.42-3.58-8-8-8z"></path>
        </svg>
    }
}

#[component]
fn LanguagePicker(locale: RwSignal<Locale>) -> impl IntoView {
    let options = [Locale::En, Locale::PtBr];

    view! {
        <div
            class="langs"
            role="group"
            aria-label=move || locale.get().strings().language_group
        >
            {options
                .into_iter()
                .map(|option| {
                    let active = move || locale.get() == option;
                    view! {
                        <button
                            class="lang"
                            type="button"
                            class:is-active=active
                            aria-pressed=move || if active() { "true" } else { "false" }
                            on:click=move |_| {
                                locale.set(option);
                                option.remember();
                            }
                        >
                            {option.label()}
                        </button>
                    }
                })
                .collect_view()}
        </div>
    }
}

#[component]
fn Shortener(
    links: RwSignal<Vec<SavedLink>>,
    storage_failed: RwSignal<bool>,
    locale: Signal<Locale>,
) -> impl IntoView {
    let (url, set_url) = signal(String::new());
    let (copied, set_copied) = signal(false);

    // `new_local`: the gloo-net future is not `Send`, and wasm is single
    // threaded anyway.
    let action = Action::new_local(|input: &String| api::shorten(input.clone()));

    // Runs whenever the action resolves. Saving here rather than in the submit
    // handler keeps a single path: whatever the server accepted is what gets
    // stored.
    Effect::new(move |_| {
        if let Some(Ok(response)) = action.value().get() {
            set_copied.set(false);
            links.update(|list| storage::prepend(list, SavedLink::from(response)));
            persist(links, storage_failed);
        }
    });

    let pending = action.pending();
    let typed_len = move || url.read().chars().count();

    view! {
        <main class="stage">
            <h1 class="thesis">
                {move || locale.get().strings().thesis_lead}
                <span class="thesis-turn">{move || locale.get().strings().thesis_turn}</span>
            </h1>

            <form
                class="composer"
                on:submit=move |ev| {
                    ev.prevent_default();
                    action.dispatch(url.get());
                }
            >
                <label class="visually-hidden" for="long-url">
                    {move || locale.get().strings().url_label}
                </label>
                <input
                    id="long-url"
                    class="composer-input"
                    type="url"
                    required
                    autocomplete="off"
                    spellcheck="false"
                    placeholder=move || locale.get().strings().url_placeholder
                    prop:value=url
                    on:input:target=move |ev| set_url.set(ev.target().value())
                />
                <button class="composer-submit" type="submit" disabled=pending>
                    {move || {
                        let strings = locale.get().strings();
                        if pending.get() { strings.shortening } else { strings.shorten }
                    }}
                </button>
            </form>

            <Meter typed=Signal::derive(typed_len) result=action.value().into() />

            <div class="outcome" aria-live="polite">
                <Outcome
                    value=action.value().into()
                    copied=copied
                    set_copied=set_copied
                    locale=locale
                />
            </div>
        </main>
    }
}

#[component]
fn Vault(
    links: RwSignal<Vec<SavedLink>>,
    storage_failed: RwSignal<bool>,
    locale: Signal<Locale>,
) -> impl IntoView {
    let remove = move |code: String| {
        links.update(|list| list.retain(|saved| saved.code != code));
        persist(links, storage_failed);
    };

    let tally = move || {
        let list = links.read();
        if list.is_empty() {
            return String::new();
        }
        let saved: usize = list.iter().map(SavedLink::saved_chars).sum();
        locale.get().tally(list.len(), saved)
    };

    view! {
        <section class="vault">
            <div class="vault-head">
                <h2 class="vault-title">{move || locale.get().strings().vault_title}</h2>
                <p class="vault-tally">{tally}</p>
            </div>

            <Show
                when=move || !links.read().is_empty()
                fallback=move || {
                    view! {
                        <p class="vault-empty">{move || locale.get().strings().vault_empty}</p>
                    }
                }
            >
                <ul class="vault-list">
                    <For each=move || links.get() key=|link| link.code.clone() let:link>
                        <li class="vault-item">
                            <a
                                class="vault-code"
                                href=link.short_url.clone()
                                target="_blank"
                                rel="noreferrer"
                            >
                                {link.code.clone()}
                            </a>
                            <span class="vault-target" title=link.long_url.clone()>
                                {strip_scheme(&link.long_url).to_owned()}
                            </span>
                            <button
                                class="vault-remove"
                                type="button"
                                aria-label={
                                    let code = link.code.clone();
                                    move || locale.get().remove_label(&code)
                                }
                                on:click={
                                    let code = link.code;
                                    move |_| remove(code.clone())
                                }
                            >
                                "×"
                            </button>
                        </li>
                    </For>
                </ul>
            </Show>

            <p class="vault-note">{move || locale.get().strings().vault_note}</p>

            <Show when=move || storage_failed.get() fallback=|| ()>
                <p class="vault-warning" role="alert">
                    {move || locale.get().strings().storage_error}
                </p>
            </Show>
        </section>
    }
}

/// The signature element: the length of what was pasted collapsing onto the
/// length of what came back. The track is the long URL, the fill is the code.
///
/// Deliberately free of text, so it needs no locale.
#[component]
fn Meter(
    typed: Signal<usize>,
    result: Signal<Option<Result<ShortenResponse, String>>>,
) -> impl IntoView {
    // Reference length for the "before" bar. There is no natural maximum for a
    // URL, so this is the length at which the track reads as full — around what
    // a link worth shortening actually measures.
    const FULL_AT: f64 = 120.0;

    let share = move || {
        let Some(Ok(response)) = result.get() else {
            return None;
        };
        let long = u32::try_from(response.long_url.chars().count()).unwrap_or(u32::MAX);
        let code = u32::try_from(response.code.chars().count()).unwrap_or(u32::MAX);
        if long == 0 {
            return None;
        }
        // Floor at 1.5% so a very long URL still leaves something visible.
        Some(f64::max(f64::from(code) / f64::from(long) * 100.0, 1.5))
    };

    // Empty track at rest, filling as the URL grows, collapsing onto the code's
    // share once the answer arrives. A track that starts full would read as
    // "zero characters is the maximum".
    let width = move || {
        if let Some(compressed) = share() {
            return compressed;
        }
        let typed = u32::try_from(typed.get()).unwrap_or(u32::MAX);
        f64::min(f64::from(typed) / FULL_AT * 100.0, 100.0)
    };

    view! {
        <div class="meter" aria-hidden="true">
            <span class="meter-from">
                {move || match result.get() {
                    Some(Ok(response)) => response.long_url.chars().count(),
                    _ => typed.get(),
                }}
            </span>
            <span class="meter-track">
                <span
                    class="meter-fill"
                    class:is-compressed=move || share().is_some()
                    style:width=move || format!("{:.2}%", width())
                ></span>
            </span>
            <span class="meter-to" class:is-reached=move || share().is_some()>
                {CODE_LEN}
            </span>
        </div>
    }
}

#[component]
fn Outcome(
    value: Signal<Option<Result<ShortenResponse, String>>>,
    copied: ReadSignal<bool>,
    set_copied: WriteSignal<bool>,
    locale: Signal<Locale>,
) -> impl IntoView {
    move || match value.get() {
        None => ().into_any(),
        // Left as the server wrote it. RFC 9457 says `title` is stable and safe
        // to match on, so translating here is possible — but it would duplicate
        // the catalogue and only ever serve this one client. Negotiating
        // `Accept-Language` on the API is the honest fix; see ROADMAP stage 5.3.
        Some(Err(error)) => view! { <p class="outcome-error">{error}</p> }.into_any(),
        Some(Ok(response)) => {
            // Three owners on purpose: the macro moves what it renders, so the
            // href, the label and the click handler each need their own.
            let href = response.short_url.clone();
            let label = response.short_url.clone();
            let to_copy = response.short_url;
            view! {
                <div class="outcome-ok">
                    <a class="outcome-link" href=href target="_blank" rel="noreferrer">
                        {label}
                    </a>
                    <button
                        class="outcome-copy"
                        type="button"
                        on:click=move |_| {
                            copy_to_clipboard(&to_copy);
                            set_copied.set(true);
                        }
                    >
                        {move || {
                            let strings = locale.get().strings();
                            if copied.get() { strings.copied } else { strings.copy }
                        }}
                    </button>
                </div>
            }
            .into_any()
        }
    }
}

/// Keeps `lang`, the tab title and the description in step with the choice.
/// Every step is best-effort: a missing element means a slightly stale tab, not
/// a broken page.
fn apply_to_document(locale: Locale) {
    let Some(document) = web_sys::window().and_then(|window| window.document()) else {
        return;
    };

    if let Some(root) = document.document_element() {
        let _outcome = root.set_attribute("lang", locale.tag());
    }

    document.set_title(locale.strings().document_title);

    if let Ok(Some(meta)) = document.query_selector("meta[name='description']") {
        let _outcome = meta.set_attribute("content", locale.strings().document_description);
    }
}

/// `with_untracked` rather than `read_untracked`: the borrow ends with the
/// closure instead of living until the end of the `if let`.
fn persist(links: RwSignal<Vec<SavedLink>>, storage_failed: RwSignal<bool>) {
    match links.with_untracked(|list| storage::save(list)) {
        Ok(()) => storage_failed.set(false),
        Err(error) => {
            // Private browsing and a full quota both reject writes. The link
            // still exists on the server — what is lost is only this browser's
            // memory of it, which is what the person is told. The reason itself
            // goes to the console: it is for whoever is debugging, not for them.
            leptos::logging::warn!("could not save the link list: {error}");
            storage_failed.set(true);
        }
    }
}

/// Fire and forget. `write_text` hands back a promise, and awaiting it would buy
/// nothing: the only failure modes are a denied permission and a non-secure
/// origin, neither of which the person can act on from here.
fn copy_to_clipboard(text: &str) {
    if let Some(window) = web_sys::window() {
        let _promise = window.navigator().clipboard().write_text(text);
    }
}

/// The scheme is noise in a list where every row is a URL. Keeping the rest
/// intact matters: two links to the same host differ only after it.
pub(crate) fn strip_scheme(url: &str) -> &str {
    url.strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url)
}
