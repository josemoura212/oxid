use gloo_net::http::Request;
use leptos::prelude::*;
use oxid_shared::{ErrorResponse, ShortenRequest, ShortenResponse};

fn main() {
    console_error_panic_hook::set_once();
    mount_to_body(App);
}

async fn shorten(url: String) -> Result<ShortenResponse, String> {
    // EN: relative path — trunk proxies it in dev, Nginx serves both on the same
    // EN: origin in production.
    // PT: caminho relativo — em dev o trunk faz proxy, em produção o Nginx serve
    // PT: os dois na mesma origem.
    let response = Request::post("/v1/shorten")
        .json(&ShortenRequest { url })
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;

    let status = response.status();
    let status_text = response.status_text();

    if response.ok() {
        return response
            .json::<ShortenResponse>()
            .await
            .map_err(|e| e.to_string());
    }

    // EN: the error body may not be JSON (axum's 404 comes empty); fall back to
    // EN: the status instead of leaking "EOF while parsing a value", which tells
    // EN: the user nothing.
    // PT: o corpo de erro pode não ser JSON (404 do axum vem vazio); cair para o
    // PT: status em vez de vazar "EOF while parsing a value", que não diz nada ao
    // PT: usuário.
    let error = response
        .json::<ErrorResponse>()
        .await
        .map_or_else(|_| format!("{status} {status_text}"), |body| body.error);

    Err(error)
}

#[component]
fn App() -> impl IntoView {
    let (url, set_url) = signal(String::new());
    // EN: `new_local` — the gloo-net future is not `Send`; WASM is single-threaded.
    // PT: `new_local` — o future do gloo-net não é `Send`; WASM é single-threaded.
    let action = Action::new_local(|input: &String| shorten(input.clone()));

    let result = move || {
        action.value().get().map(|outcome| match outcome {
            Ok(response) => {
                let href = response.short_url.clone();
                view! {
                    <p class="ok">
                        <a href=href target="_blank">
                            {response.short_url}
                        </a>
                    </p>
                }
                .into_any()
            }
            Err(error) => view! { <p class="error">{error}</p> }.into_any(),
        })
    };

    view! {
        <main>
            <h1>"oxid"</h1>

            <form on:submit=move |ev| {
                ev.prevent_default();
                action.dispatch(url.get());
            }>
                <input
                    type="url"
                    required
                    placeholder="https://example.com/a/very/long/url"
                    prop:value=url
                    on:input:target=move |ev| set_url.set(ev.target().value())
                />
                <button type="submit" disabled=move || action.pending().get()>
                    {move || if action.pending().get() { "Shortening..." } else { "Shorten" }}
                </button>
            </form>

            {result}
        </main>
    }
}
