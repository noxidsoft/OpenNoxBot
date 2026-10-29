mod api;
mod api_keys;
mod chat;
mod engines;
mod hosts;
mod settings;

use chat::Chat;
use leptos::prelude::*;
use settings::Settings;

#[component]
fn App() -> impl IntoView {
    let (settings_open, set_settings_open) = signal(false);
    view! {
        <div class="pane" class:hidden=move || settings_open.get()>
            <Chat open_settings=move |_| set_settings_open.set(true) />
        </div>
        <Show when=move || settings_open.get()>
            <Settings close=move |_| set_settings_open.set(false) />
        </Show>
    }
}

fn main() {
    mount_to_body(App);
}
