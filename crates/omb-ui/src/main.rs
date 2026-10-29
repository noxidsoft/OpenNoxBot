mod api;
mod api_keys;
mod engines;

use api_keys::ApiKeys;
use engines::Engines;
use leptos::prelude::*;

#[derive(Clone, Copy, PartialEq)]
enum Page {
    Engines,
    ApiKeys,
}

#[component]
fn App() -> impl IntoView {
    let (page, set_page) = signal(Page::Engines);
    let item = move |target: Page, name: &'static str| {
        view! {
            <button
                class:active=move || page.get() == target
                aria-current=move || (page.get() == target).then_some("page")
                on:click=move |_| set_page.set(target)
            >
                {name}
            </button>
        }
    };

    view! {
        <div class="settings">
            <nav>
                <h2>"Settings"</h2>
                {item(Page::Engines, "Engines")}
                {item(Page::ApiKeys, "API keys")}
            </nav>
            <main>
                {move || match page.get() {
                    Page::Engines => view! { <Engines /> }.into_any(),
                    Page::ApiKeys => view! { <ApiKeys /> }.into_any(),
                }}
            </main>
        </div>
    }
}

fn main() {
    mount_to_body(App);
}
