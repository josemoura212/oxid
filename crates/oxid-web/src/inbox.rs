//! The two screens a link from an e-mail opens.
//!
//! Both arrive on the site root — `/#verify=…` and `/#reset=…` — because this
//! front end has no router and nginx deliberately refuses to serve `index.html`
//! for unknown paths. A catch-all fallback there would answer 200 for a shortcode
//! that reached the wrong service, hiding the misrouting behind a blank page.
//!
//! **The fragment rather than the query, and that is not cosmetic.** A fragment
//! never leaves the browser. In the query, every click wrote the token into the
//! nginx access log in plain text — and a reset token survives the click, because
//! opening only checks it. Reading pod logs was enough to take an account over
//! before the person finished typing their new password.
//!
//! Reading a query rather than adding a router is also the smaller change: these
//! are two states, not two pages, and they render as dialogs over the page the
//! same way every other screen here does.

use js_sys::JsString;
use leptos::prelude::*;
use oxid_shared::MIN_PASSWORD_LEN;

use crate::{account::Panel, api, i18n::Locale};

/// What the URL is asking for, if anything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Arrival {
    Verify(String),
    Reset(String),
}

/// Reads the query the page was opened with.
///
/// Returns `None` for an ordinary visit, which is almost every visit. A malformed
/// or empty token reads as `None` too: an empty string would open the screen and
/// then fail against the server for a reason nobody could act on.
pub fn arrival() -> Option<Arrival> {
    pick(&web_sys::window()?.location().hash().ok()?)
}

/// The parsing half, split out so it can be tested without a browser.
fn pick(fragment: &str) -> Option<Arrival> {
    let query = fragment.strip_prefix('#').unwrap_or(fragment);

    // `filter_map` rather than `?` on the split. Written with `?`, a pair with no
    // `=` in it returned `None` from the whole function — so `/?ref&verify=abc`
    // silently dropped a valid link, and a bare flag anywhere before ours was
    // enough to do it.
    query
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .find_map(|(key, value)| {
            let value = value.trim();
            if value.is_empty() {
                return None;
            }

            match key {
                "verify" => Some(Arrival::Verify(value.to_owned())),
                "reset" => Some(Arrival::Reset(value.to_owned())),
                _ => None,
            }
        })
}

/// Drops the token from the address bar without reloading.
///
/// A fragment never reaches a server, so this is not about logs — it is about the
/// copy that survives locally: browser history, session restore, and whatever
/// gets pasted when someone copies the address to ask for help.
fn clear_query() {
    let Some(window) = web_sys::window() else {
        return;
    };

    if let Ok(history) = window.history() {
        let _ = history.replace_state_with_url(&JsString::from(""), "", Some("/"));
    }
}

/// What the confirmation screen is doing.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Verifying {
    Working,
    Done,
    /// Carries the server's own sentence rather than a fixed one.
    ///
    /// `is_ok()` used to collapse four different outcomes into "this link expired
    /// or was already used" — which is a *statement of fact about the token*, and
    /// false for three of them. A dropped connection, a 429, or a Redis outage
    /// all left a perfectly valid link, and told the reader to throw it away.
    Failed(String),
}

/// Spends a confirmation link and says what happened.
///
/// Runs on mount rather than behind a button. The person already clicked
/// something — the link in the message — and asking them to click again to
/// confirm the click is a step that carries no decision.
#[component]
pub fn VerifyScreen(token: String, locale: Signal<Locale>) -> impl IntoView {
    let state = RwSignal::new(Verifying::Working);
    let open = RwSignal::new(true);

    Effect::new(move |_| {
        let token = token.clone();
        leptos::task::spawn_local(async move {
            match api::verify_email(token).await {
                Ok(()) => {
                    // Only on success. Clearing after a network failure would take
                    // the token out of the URL without having spent it, so a
                    // refresh could not retry — the link still works, but the only
                    // copy of it is back in the inbox.
                    clear_query();
                    state.set(Verifying::Done);
                }
                Err(message) => state.set(Verifying::Failed(message)),
            }
        });
    });

    view! {
        <Show when=move || open.get()>
            <Panel
                eyebrow=Signal::derive(move || locale.get().strings().account_dialog.to_owned())
                title=Signal::derive(move || locale.get().strings().verify_title.to_owned())
                close_label=Signal::derive(move || locale.get().strings().close.to_owned())
                close=Callback::new(move |()| open.set(false))
            >
                <div class="panel-body">
                    {move || {
                        let strings = locale.get().strings();
                        match state.get() {
                            Verifying::Working => {
                                view! { <p class="status">{strings.verify_working}</p> }.into_any()
                            }
                            Verifying::Done => {
                                view! { <p class="note">{strings.verify_done}</p> }.into_any()
                            }
                            Verifying::Failed(message) => {
                                view! {
                                    <p class="status status--error" role="alert">
                                        {message}
                                    </p>
                                }
                                    .into_any()
                            }
                        }
                    }}
                </div>
                <div class="panel-foot">
                    <button class="btn--link" type="button" on:click=move |_| open.set(false)>
                        {move || locale.get().strings().back_to_sign_in}
                    </button>
                </div>
            </Panel>
        </Show>
    }
}

/// What the reset screen is doing.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Resetting {
    /// Asking the server whether the link is still good, before showing a form
    /// somebody would type a password into for nothing.
    Checking,
    Form,
    Done,
    /// The server's sentence, for the same reason as [`Verifying::Failed`].
    Invalid(String),
}

/// Checks a reset link on open, and spends it on save.
///
/// **Two steps on purpose, and this is the decision worth knowing.** Consuming
/// the token when the page loads would kill the link before anyone typed
/// anything — a refresh, a second tab, or a mail client that pre-fetches links
/// would each be enough. So opening only *checks*, and saving is what spends it.
/// A one-line panel body with a footer that only closes. Three of the four reset
/// states are exactly this, and writing them out three times is how the wording
/// of one drifts from the others.
#[component]
fn Outcome(
    #[prop(into)] message: Signal<String>,
    #[prop(optional)] bad: bool,
    open: RwSignal<bool>,
    locale: Signal<Locale>,
) -> impl IntoView {
    view! {
        <div class="panel-body">
            <Show
                when=move || bad
                fallback=move || view! { <p class="note">{move || message.get()}</p> }
            >
                <p class="status status--error" role="alert">
                    {move || message.get()}
                </p>
            </Show>
        </div>
        <div class="panel-foot">
            <button class="btn--link" type="button" on:click=move |_| open.set(false)>
                {move || locale.get().strings().back_to_sign_in}
            </button>
        </div>
    }
}

/// The form itself, once the link has been checked.
#[component]
fn ResetForm(
    locale: Signal<Locale>,
    password: ReadSignal<String>,
    set_password: WriteSignal<String>,
    error: RwSignal<Option<String>>,
    pending: Signal<bool>,
    submit: Callback<()>,
) -> impl IntoView {
    view! {
        <form
            method="post"
            action="/v1/reset-password"
            on:submit=move |ev| {
                ev.prevent_default();
                submit.run(());
            }
        >
            <div class="panel-body">
                <div class="stack">
                    <p class="note">{move || locale.get().strings().reset_body}</p>
                    <label class="field" for="reset-password">
                        <span class="field-label">
                            {move || locale.get().strings().reset_new_password}
                        </span>
                        <input
                            id="reset-password"
                            class="field-input"
                            type="password"
                            name="password"
                            // Tells a password manager this is a change, not a
                            // sign-in, so it offers to update rather than fill.
                            autocomplete="new-password"
                            minlength=MIN_PASSWORD_LEN.to_string()
                            required
                            prop:value=move || password.get()
                            on:input=move |ev| set_password.set(event_target_value(&ev))
                        />
                        <span class="field-hint">
                            {move || locale.get().strings().password_hint}
                        </span>
                    </label>

                    <Show when=move || error.get().is_some()>
                        <p class="status status--error" role="alert">
                            {move || error.get()}
                        </p>
                    </Show>
                </div>
            </div>
            <div class="panel-foot">
                <button class="btn" type="submit" disabled=move || pending.get()>
                    {move || {
                        let strings = locale.get().strings();
                        if pending.get() { strings.working } else { strings.reset_save }
                    }}
                </button>
            </div>
        </form>
    }
}

/// Checks a reset link on open, and spends it on save.
///
/// **Two steps on purpose, and this is the decision worth knowing.** Consuming
/// the token when the page loads would kill the link before anyone typed
/// anything — a refresh, a second tab, or a mail client that pre-fetches links
/// would each be enough. So opening only *checks*, and saving is what spends it.
#[component]
pub fn ResetScreen(token: String, locale: Signal<Locale>) -> impl IntoView {
    let state = RwSignal::new(Resetting::Checking);
    let open = RwSignal::new(true);
    let (password, set_password) = signal(String::new());
    let error = RwSignal::new(Option::<String>::None);

    let held = token.clone();
    Effect::new(move |_| {
        let token = held.clone();
        leptos::task::spawn_local(async move {
            state.set(match api::check_reset(&token).await {
                Ok(()) => Resetting::Form,
                Err(message) => Resetting::Invalid(message),
            });
        });
    });

    let save = Action::new_local(move |(): &()| {
        let token = token.clone();
        let password = password.get();
        async move {
            match api::reset_password(token, password).await {
                Ok(()) => {
                    error.set(None);
                    set_password.set(String::new());
                    clear_query();
                    state.set(Resetting::Done);
                }
                Err(message) => error.set(Some(message)),
            }
        }
    });

    let pending = save.pending();

    view! {
        <Show when=move || open.get()>
            <Panel
                eyebrow=Signal::derive(move || locale.get().strings().account_dialog.to_owned())
                title=Signal::derive(move || locale.get().strings().reset_title.to_owned())
                close_label=Signal::derive(move || locale.get().strings().close.to_owned())
                close=Callback::new(move |()| open.set(false))
            >
                {move || match state.get() {
                    Resetting::Checking => {
                        view! {
                            <div class="panel-body">
                                <p class="status">
                                    {move || locale.get().strings().reset_checking}
                                </p>
                            </div>
                        }
                            .into_any()
                    }
                    Resetting::Invalid(message) => {
                        view! {
                            <Outcome
                                message=Signal::derive(move || message.clone())
                                bad=true
                                open=open
                                locale=locale
                            />
                        }
                            .into_any()
                    }
                    Resetting::Done => {
                        view! {
                            <Outcome
                                message=Signal::derive(move || {
                                    locale.get().strings().reset_done.to_owned()
                                })
                                open=open
                                locale=locale
                            />
                        }
                            .into_any()
                    }
                    Resetting::Form => {
                        view! {
                            <ResetForm
                                locale=locale
                                password=password
                                set_password=set_password
                                error=error
                                pending=Signal::derive(move || pending.get())
                                submit=Callback::new(move |()| {
                                    save.dispatch(());
                                })
                            />
                        }
                            .into_any()
                    }
                }}
            </Panel>
        </Show>
    }
}

#[cfg(test)]
mod tests {
    use super::{Arrival, pick};

    #[test]
    fn an_ordinary_visit_carries_nothing() {
        assert_eq!(pick(""), None);
        assert_eq!(pick("#"), None);
        assert_eq!(pick("#utm_source=x"), None);
    }

    #[test]
    fn both_kinds_are_recognised() {
        assert_eq!(pick("#verify=abc"), Some(Arrival::Verify("abc".to_owned())));
        assert_eq!(pick("#reset=abc"), Some(Arrival::Reset("abc".to_owned())));
    }

    /// The bug this replaced: written with `?` on the split, a pair without an
    /// `=` returned `None` for the whole query — so a bare flag before ours threw
    /// away a valid link.
    #[test]
    fn a_valueless_pair_does_not_swallow_the_rest() {
        assert_eq!(
            pick("#ref&verify=abc"),
            Some(Arrival::Verify("abc".to_owned()))
        );
        assert_eq!(
            pick("#a=1&b&reset=xyz"),
            Some(Arrival::Reset("xyz".to_owned()))
        );
    }

    /// An empty value would open the screen and then fail against the server for
    /// a reason nobody could act on.
    #[test]
    fn an_empty_token_is_not_an_arrival() {
        assert_eq!(pick("#verify="), None);
        assert_eq!(
            pick("#verify=&reset=abc"),
            Some(Arrival::Reset("abc".to_owned()))
        );
    }
}
