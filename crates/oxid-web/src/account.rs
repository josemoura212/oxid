//! Sign in, sign up, and the links that belong to an account.

use leptos::prelude::*;
use oxid_shared::OwnedLink;

use crate::{api, i18n::Locale, storage::SavedLink};

/// Which form the dialog is showing. Two modes rather than two dialogs: the
/// fields are identical and the only difference is where the submit goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    SignIn,
    SignUp,
}

/// Everything the page needs to know about who is signed in.
#[derive(Debug, Clone, Copy)]
pub struct Account {
    /// `None` while the first check is in flight, `Some(None)` once it comes
    /// back anonymous. Three states, not two — rendering "sign in" before the
    /// answer arrives makes the button flicker for someone who is logged in.
    pub user: RwSignal<Option<Option<i64>>>,
    pub links: RwSignal<Vec<OwnedLink>>,
    pub cursor: RwSignal<Option<String>>,
    /// Set right after a signup that imported saved links, so the note appears
    /// once instead of on every visit.
    pub imported: RwSignal<bool>,
}

impl Default for Account {
    fn default() -> Self {
        Self::new()
    }
}

impl Account {
    pub fn new() -> Self {
        Self {
            user: RwSignal::new(None),
            links: RwSignal::new(Vec::new()),
            cursor: RwSignal::new(None),
            imported: RwSignal::new(false),
        }
    }

    pub fn signed_in(self) -> bool {
        matches!(self.user.get(), Some(Some(_)))
    }

    /// Replaces the list from the first page. Called after signing in and after
    /// an import, both of which invalidate whatever was on screen.
    pub async fn reload(self) {
        match api::owned_links(None).await {
            Ok(page) => {
                self.links.set(page.links);
                self.cursor.set(page.next_cursor);
            }
            Err(error) => leptos::logging::warn!("could not load account links: {error}"),
        }
    }

    pub async fn load_more(self) {
        let Some(cursor) = self.cursor.get() else {
            return;
        };

        match api::owned_links(Some(&cursor)).await {
            Ok(page) => {
                self.links.update(|list| list.extend(page.links));
                self.cursor.set(page.next_cursor);
            }
            Err(error) => leptos::logging::warn!("could not load more links: {error}"),
        }
    }
}

/// Sends the browser's saved links to the account, then reloads the list.
///
/// Failure is logged, not surfaced. The account exists and the person is signed
/// in — the import is a convenience, and blocking the screen on it would trade a
/// working session for an error nobody can act on. The local list is untouched
/// either way, so nothing is lost.
async fn import_saved(account: Account, saved: Vec<SavedLink>) {
    if saved.is_empty() {
        return;
    }

    let urls: Vec<String> = saved.into_iter().map(|link| link.long_url).collect();

    match api::import(urls).await {
        Ok(result) => {
            if result.imported > 0 {
                account.imported.set(true);
            }
            if result.rejected > 0 {
                leptos::logging::warn!("{} saved links were rejected", result.rejected);
            }
        }
        Err(error) => leptos::logging::warn!("could not import saved links: {error}"),
    }

    account.reload().await;
}

/// Three bars, inlined like the GitHub mark — one 16×16 path is cheaper than a
/// request, and a strict CSP is on the roadmap.
#[component]
fn HamburgerIcon() -> impl IntoView {
    view! {
        <svg
            class="icon"
            viewBox="0 0 16 16"
            width="16"
            height="16"
            fill="none"
            stroke="currentColor"
            stroke-width="1.6"
            stroke-linecap="round"
            aria-hidden="true"
            focusable="false"
        >
            <path d="M2 4h12M2 8h12M2 12h12"></path>
        </svg>
    }
}

/// Drops this browser to the anonymous state after any sign-out — the cookie it
/// held is gone (revoked, or on its way out), so nothing is left to keep.
fn to_anonymous(account: Account) {
    account.user.set(Some(None));
    account.links.set(Vec::new());
    account.cursor.set(None);
    account.imported.set(false);
}

#[component]
pub fn AccountButton(
    account: Account,
    locale: Signal<Locale>,
    open: RwSignal<bool>,
) -> impl IntoView {
    // Local to the button, not lifted to `App`: nothing else needs to know the
    // menu is open, and keeping it here means the veil and the toggle share one
    // signal without threading it through the tree.
    let menu_open = RwSignal::new(false);

    let sign_out = Action::new_local(move |(): &()| async move {
        let _ = api::logout().await;
        to_anonymous(account);
        menu_open.set(false);
    });

    // Revokes every session, then falls to anonymous like an ordinary sign-out —
    // this browser's cookie was among the ones just revoked.
    let sign_out_all = Action::new_local(move |(): &()| async move {
        match api::logout_all().await {
            Ok(()) => to_anonymous(account),
            Err(error) => leptos::logging::warn!("sign out everywhere failed: {error}"),
        }
        menu_open.set(false);
    });

    view! {
        <Show
            when=move || account.signed_in()
            fallback=move || {
                view! {
                    <button class="lang" type="button" on:click=move |_| open.set(true)>
                        {move || locale.get().strings().sign_in}
                    </button>
                }
            }
        >
            <div class="account-menu">
                <button
                    class="lang account-menu-toggle"
                    type="button"
                    aria-haspopup="menu"
                    aria-expanded=move || if menu_open.get() { "true" } else { "false" }
                    aria-label=move || locale.get().strings().account_menu
                    title=move || locale.get().strings().account_menu
                    on:click=move |_| menu_open.update(|o| *o = !*o)
                >
                    <HamburgerIcon />
                </button>

                <Show when=move || menu_open.get()>
                    // A full-window veil so a click anywhere outside the menu
                    // closes it — the plain-CSS equivalent of a click-away
                    // listener, without reaching for a global event handler.
                    <div
                        class="menu-veil"
                        role="presentation"
                        on:click=move |_| menu_open.set(false)
                    ></div>
                    <div class="account-dropdown" role="menu">
                        <button
                            class="menu-item"
                            type="button"
                            role="menuitem"
                            on:click=move |_| {
                                sign_out.dispatch(());
                            }
                        >
                            {move || locale.get().strings().sign_out}
                        </button>
                        <button
                            class="menu-item"
                            type="button"
                            role="menuitem"
                            on:click=move |_| {
                                sign_out_all.dispatch(());
                            }
                        >
                            {move || locale.get().strings().sign_out_all}
                        </button>
                    </div>
                </Show>
            </div>
        </Show>
    }
}

/// The two fields, extracted so the dialog stays readable.
///
/// Identical between signing in and signing up — only `autocomplete` differs,
/// and that difference is what tells a password manager whether to offer to save
/// or to fill.
#[component]
fn CredentialsFields(
    locale: Signal<Locale>,
    mode: RwSignal<Mode>,
    email: ReadSignal<String>,
    set_email: WriteSignal<String>,
    password: ReadSignal<String>,
    set_password: WriteSignal<String>,
    confirm: ReadSignal<String>,
    set_confirm: WriteSignal<String>,
) -> impl IntoView {
    view! {
        <label class="field" for="account-email">
            <span class="field-label">{move || locale.get().strings().email_label}</span>
            <input
                id="account-email"
                // `name` as well as `autocomplete`: password managers fall back to
                // matching on it, and several will not offer to fill a field that
                // has none.
                name="username"
                class="field-input"
                type="email"
                required
                // `username`, not `email`. The spec reserves `email` for an
                // address that is merely an address; the field that identifies
                // the account is `username`, and it is what a manager pairs with
                // the password to store one credential instead of two orphans.
                autocomplete="username"
                // Gets the @ keyboard on a phone without changing validation.
                inputmode="email"
                autocapitalize="none"
                spellcheck="false"
                autofocus
                prop:value=email
                on:input:target=move |ev| set_email.set(ev.target().value())
            />
        </label>

        <label class="field" for="account-password">
            <span class="field-label">{move || locale.get().strings().password_label}</span>
            <input
                id="account-password"
                name="password"
                class="field-input"
                type="password"
                required
                minlength="12"
                // The one difference between the two modes, and it is what tells
                // the manager whether to offer to fill an existing secret or to
                // generate and save a new one.
                autocomplete=move || match mode.get() {
                    Mode::SignIn => "current-password",
                    Mode::SignUp => "new-password",
                }
                prop:value=password
                on:input:target=move |ev| set_password.set(ev.target().value())
            />
            <Show when=move || mode.get() == Mode::SignUp>
                <span class="field-hint">
                    {move || locale.get().strings().password_hint}
                </span>
            </Show>
        </label>

        // Signup only. On the login form a second field would be asking someone
        // to type a password they are trying to recall — twice.
        <Show when=move || mode.get() == Mode::SignUp>
            <label class="field" for="account-password-confirm">
                <span class="field-label">
                    {move || locale.get().strings().password_confirm_label}
                </span>
                <input
                    id="account-password-confirm"
                    name="confirm_password"
                    class="field-input"
                    type="password"
                    required
                    // Same token as the field above, so a manager recognises the
                    // pair and fills both rather than offering to save two
                    // different secrets for one account.
                    autocomplete="new-password"
                    prop:value=confirm
                    on:input:target=move |ev| set_confirm.set(ev.target().value())
                />
            </label>
        </Show>
    }
}

/// Submit plus the link that switches between the two modes.
///
/// Switching clears the error: a message about the wrong password makes no sense
/// once the form has become a signup, and leaving it there reads as a failure
/// that just happened.
#[component]
fn DialogActions(
    locale: Signal<Locale>,
    mode: RwSignal<Mode>,
    error: RwSignal<Option<String>>,
    pending: Signal<bool>,
) -> impl IntoView {
    view! {
        <div class="dialog-actions">
            <button class="composer-submit" type="submit" disabled=move || pending.get()>
                {move || {
                    let strings = locale.get().strings();
                    if pending.get() {
                        strings.working
                    } else if mode.get() == Mode::SignIn {
                        strings.sign_in
                    } else {
                        strings.sign_up
                    }
                }}
            </button>

            <button
                class="dialog-switch"
                type="button"
                on:click=move |_| {
                    error.set(None);
                    mode
                        .set(
                            if mode.get() == Mode::SignIn { Mode::SignUp } else { Mode::SignIn },
                        );
                }
            >
                {move || {
                    let strings = locale.get().strings();
                    if mode.get() == Mode::SignIn {
                        strings.no_account
                    } else {
                        strings.have_account
                    }
                }}
            </button>
        </div>
    }
}

#[component]
pub fn AccountDialog(
    account: Account,
    locale: Signal<Locale>,
    open: RwSignal<bool>,
    saved: RwSignal<Vec<SavedLink>>,
) -> impl IntoView {
    let mode = RwSignal::new(Mode::SignIn);
    let (email, set_email) = signal(String::new());
    let (password, set_password) = signal(String::new());
    let (confirm, set_confirm) = signal(String::new());
    let error = RwSignal::new(Option::<String>::None);

    let submit = Action::new_local(move |(): &()| {
        let email = email.get();
        let password = password.get();
        let mode = mode.get();

        // Checked here rather than on the server: the mismatch is a typo in this
        // browser, and the server has no business receiving a second copy of a
        // password to compare — it would be one more place the secret travels
        // and appears in a body that could be logged.
        let mismatch = mode == Mode::SignUp && password != confirm.get();

        async move {
            if mismatch {
                error.set(Some(
                    locale
                        .get_untracked()
                        .strings()
                        .password_mismatch
                        .to_owned(),
                ));
                return;
            }

            let result = match mode {
                Mode::SignIn => api::login(email, password).await,
                Mode::SignUp => api::signup(email, password).await,
            };

            match result {
                Ok(response) => {
                    error.set(None);
                    account.user.set(Some(Some(response.id)));
                    open.set(false);
                    // Cleared, but only after the submit event has been and gone
                    // — a manager that has not yet decided whether to offer to
                    // save reads the fields, and emptying them too early makes
                    // the prompt disappear.
                    set_password.set(String::new());
                    set_confirm.set(String::new());

                    // Only a fresh account imports. Signing in on a second
                    // device would otherwise re-import that browser's list
                    // every time, creating nothing new but reloading for no
                    // reason.
                    if mode == Mode::SignUp {
                        import_saved(account, saved.get()).await;
                    } else {
                        account.reload().await;
                    }
                }
                Err(message) => error.set(Some(message)),
            }
        }
    });

    let pending = submit.pending();

    view! {
        <Show when=move || open.get()>
            <div
                class="dialog-veil"
                role="presentation"
                on:click=move |_| open.set(false)
            ></div>

            <div
                class="dialog"
                role="dialog"
                aria-modal="true"
                aria-label=move || locale.get().strings().account_dialog
            >
                <form
                    class="dialog-form"
                    // Declared even though the submit is intercepted. Password
                    // managers look for a form that posts somewhere before they
                    // offer to save a credential — a form with neither method
                    // nor action reads as a widget, not a login. The path is the
                    // real endpoint, so if scripting ever fails the browser
                    // posts to something that exists.
                    method="post"
                    action=move || match mode.get() {
                        Mode::SignIn => "/v1/login",
                        Mode::SignUp => "/v1/signup",
                    }
                    on:submit=move |ev| {
                        ev.prevent_default();
                        submit.dispatch(());
                    }
                >
                    <CredentialsFields
                        locale=locale
                        mode=mode
                        email=email
                        set_email=set_email
                        password=password
                        set_password=set_password
                        confirm=confirm
                        set_confirm=set_confirm
                    />

                    <Show when=move || error.get().is_some()>
                        <p class="dialog-error" role="alert">{move || error.get()}</p>
                    </Show>

                    <DialogActions
                        locale=locale
                        mode=mode
                        error=error
                        pending=Signal::derive(move || pending.get())
                    />
                </form>

                <button
                    class="dialog-close"
                    type="button"
                    aria-label=move || locale.get().strings().close
                    on:click=move |_| open.set(false)
                >
                    "×"
                </button>
            </div>
        </Show>
    }
}

/// The account's links, replacing the browser list once signed in.
#[component]
pub fn AccountVault(account: Account, locale: Signal<Locale>) -> impl IntoView {
    let more = Action::new_local(move |(): &()| async move { account.load_more().await });

    view! {
        <section class="vault">
            <div class="vault-head">
                // Sign out and sign-out-everywhere both live in the top-bar menu
                // now, so the list header carries only its title.
                <h2 class="vault-title">
                    {move || locale.get().strings().vault_account_title}
                </h2>
            </div>

            <Show when=move || account.imported.get()>
                <p class="vault-note">{move || locale.get().strings().import_note}</p>
            </Show>

            <Show
                when=move || !account.links.read().is_empty()
                fallback=move || {
                    view! {
                        <p class="vault-empty">
                            {move || locale.get().strings().vault_account_empty}
                        </p>
                    }
                }
            >
                <ul class="vault-list">
                    <For
                        each=move || account.links.get()
                        key=|link| link.code.clone()
                        let:link
                    >
                        {
                            // Computed before the view so the full URL can move
                            // into `title` instead of being cloned for it.
                            let target = crate::app::strip_scheme(&link.long_url).to_owned();
                            view! {
                                <li class="vault-item">
                                    <a
                                        class="vault-code"
                                        href=link.short_url
                                        target="_blank"
                                        rel="noreferrer"
                                    >
                                        {link.code}
                                    </a>
                                    <span class="vault-target" title=link.long_url>
                                        {target}
                                    </span>
                                </li>
                            }
                        }
                    </For>
                </ul>
            </Show>

            <Show when=move || account.cursor.get().is_some()>
                <button
                    class="dialog-switch"
                    type="button"
                    on:click=move |_| {
                        more.dispatch(());
                    }
                >
                    {move || locale.get().strings().load_more}
                </button>
            </Show>
        </section>
    }
}
