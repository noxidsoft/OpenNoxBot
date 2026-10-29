use crate::api::call;
use leptos::prelude::*;
use serde::Serialize;

#[derive(Serialize)]
struct Key<'a> {
    key: &'a str,
}

#[component]
pub fn ApiKeys() -> impl IntoView {
    let (saved, set_saved) = signal(false);
    let (key, set_key) = signal(String::new());
    let (failure, set_failure) = signal(None::<String>);

    leptos::task::spawn_local(async move {
        match call::<bool>("jev_key_saved", ()).await {
            Ok(found) => set_saved.set(found),
            Err(message) => set_failure.set(Some(message)),
        }
    });

    let save = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        let value = key.get_untracked();
        leptos::task::spawn_local(async move {
            match call::<()>("save_jev_key", Key { key: &value }).await {
                Ok(()) => {
                    set_key.set(String::new());
                    set_failure.set(None);
                    set_saved.set(true);
                }
                Err(message) => set_failure.set(Some(message)),
            }
        });
    };

    let remove = move |_| {
        leptos::task::spawn_local(async move {
            match call::<()>("clear_jev_key", ()).await {
                Ok(()) => {
                    set_failure.set(None);
                    set_saved.set(false);
                }
                Err(message) => set_failure.set(Some(message)),
            }
        });
    };

    view! {
        <h1>"API keys"</h1>
        <form class="key-row" on:submit=save>
            <label for="jev-key">"Jev"</label>
            <input
                id="jev-key"
                type="password"
                autocomplete="off"
                spellcheck="false"
                placeholder=move || if saved.get() { "••••••••••••" } else { "API key" }
                prop:value=key
                on:input=move |ev| set_key.set(event_target_value(&ev))
            />
            <button type="submit" disabled=move || key.with(|k| k.trim().is_empty())>"Save"</button>
            <Show when=move || saved.get()>
                <button type="button" class="secondary" on:click=remove>"Remove"</button>
            </Show>
        </form>
        {move || failure.get().map(|message| view! { <p class="failure">{message}</p> })}
    }
}
