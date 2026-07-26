use gloo_net::http::Request;
use leptos::prelude::*;
use oxid_shared::{ProblemDetails, ShortenRequest, ShortenResponse};

fn main() {
    console_error_panic_hook::set_once();
    mount_to_body(App);
}

async fn shorten(url: String) -> Result<ShortenResponse, String> {
    // relative path — trunk proxies it in dev, Nginx serves both on the same
    // origin in production.
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

    // The error body may not be JSON at all — axum's own 404 comes back empty.
    // Falling back to the status beats leaking "EOF while parsing a value", which
    // tells the user nothing about what went wrong.
    let error = response.json::<ProblemDetails>().await.map_or_else(
        |_| format!("{status} {status_text}"),
        |problem| problem.message().to_owned(),
    );

    Err(error)
}

#[component]
fn App() -> impl IntoView {
    let (url, set_url) = signal(String::new());
    // `new_local` — the gloo-net future is not `Send`; WASM is single-threaded.
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
