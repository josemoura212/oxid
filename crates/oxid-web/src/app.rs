use leptos::prelude::*;
use oxid_shared::ShortenResponse;

use crate::{api, storage, storage::SavedLink};

/// Length of every code the API issues. The meter shows it as the target from
/// the first keystroke, so the page states the promise instead of waiting for a
/// response to reveal it.
const CODE_LEN: usize = 7;

/// Holds only what both halves of the page need: the list and the one message
/// that has to survive a failed write.
#[component]
pub fn App() -> impl IntoView {
    let links = RwSignal::new(storage::load());
    let (storage_error, set_storage_error) = signal(Option::<String>::None);

    view! {
        <div class="shell">
            <header class="masthead">
                <span class="wordmark">"Oxid"</span>
            </header>

            <Shortener links=links set_storage_error=set_storage_error />

            <Vault
                links=links
                storage_error=storage_error
                set_storage_error=set_storage_error
            />
        </div>
    }
}

#[component]
fn Shortener(
    links: RwSignal<Vec<SavedLink>>,
    set_storage_error: WriteSignal<Option<String>>,
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
            persist(links, set_storage_error);
        }
    });

    let pending = action.pending();
    let typed_len = move || url.read().chars().count();

    view! {
        <main class="stage">
            <h1 class="thesis">
                "Long links in. " <span class="thesis-turn">"Seven characters out."</span>
            </h1>

            <form
                class="composer"
                on:submit=move |ev| {
                    ev.prevent_default();
                    action.dispatch(url.get());
                }
            >
                <label class="visually-hidden" for="long-url">
                    "Long URL"
                </label>
                <input
                    id="long-url"
                    class="composer-input"
                    type="url"
                    required
                    autocomplete="off"
                    spellcheck="false"
                    placeholder="https://example.com/a/very/long/url"
                    prop:value=url
                    on:input:target=move |ev| set_url.set(ev.target().value())
                />
                <button class="composer-submit" type="submit" disabled=pending>
                    {move || if pending.get() { "Shortening" } else { "Shorten" }}
                </button>
            </form>

            <Meter typed=Signal::derive(typed_len) result=action.value().into() />

            <div class="outcome" aria-live="polite">
                <Outcome value=action.value().into() copied=copied set_copied=set_copied />
            </div>
        </main>
    }
}

#[component]
fn Vault(
    links: RwSignal<Vec<SavedLink>>,
    storage_error: ReadSignal<Option<String>>,
    set_storage_error: WriteSignal<Option<String>>,
) -> impl IntoView {
    let remove = move |code: String| {
        links.update(|list| list.retain(|saved| saved.code != code));
        persist(links, set_storage_error);
    };

    view! {
        <section class="vault">
            <div class="vault-head">
                <h2 class="vault-title">"In this browser"</h2>
                <p class="vault-tally">{move || tally(&links.read())}</p>
            </div>

            <Show
                when=move || !links.read().is_empty()
                fallback=|| {
                    view! {
                        <p class="vault-empty">
                            "Nothing here yet. Shorten a link and it stays on this device."
                        </p>
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
                                aria-label=format!("Remove {} from this list", link.code)
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

            <p class="vault-note">
                "Kept in this browser only, never sent to the server. Removing a link here does not disable it — anyone holding it keeps being redirected."
            </p>

            <Show when=move || storage_error.read().is_some() fallback=|| ()>
                <p class="vault-warning" role="alert">{move || storage_error.get()}</p>
            </Show>
        </section>
    }
}

/// The signature element: the length of what was pasted collapsing onto the
/// length of what came back. The track is the long URL, the fill is the code.
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
) -> impl IntoView {
    move || match value.get() {
        None => ().into_any(),
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
                        {move || if copied.get() { "Copied" } else { "Copy" }}
                    </button>
                </div>
            }
            .into_any()
        }
    }
}

/// `with_untracked` rather than `read_untracked`: the borrow ends with the
/// closure instead of living until the end of the `if let`.
fn persist(links: RwSignal<Vec<SavedLink>>, set_storage_error: WriteSignal<Option<String>>) {
    let outcome = links.with_untracked(|list| storage::save(list));

    if let Err(error) = outcome {
        // Private browsing and a full quota both reject writes. The link still
        // exists on the server — what is lost is only this browser's memory of
        // it, which is exactly what the person needs to be told.
        set_storage_error.set(Some(format!(
            "This browser refused to save the list ({error}). Copy the link before leaving."
        )));
    } else {
        set_storage_error.set(None);
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

fn tally(links: &[SavedLink]) -> String {
    if links.is_empty() {
        return String::new();
    }

    let saved: usize = links.iter().map(SavedLink::saved_chars).sum();
    let unit = if links.len() == 1 { "link" } else { "links" };

    format!("{} {unit} · {saved} characters saved", links.len())
}

/// The scheme is noise in a list where every row is a URL. Keeping the rest
/// intact matters: two links to the same host differ only after it.
fn strip_scheme(url: &str) -> &str {
    url.strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url)
}
