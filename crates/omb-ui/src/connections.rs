use crate::api::call;
use leptos::prelude::*;
use omb_wire::{EmailSettings, EmailStatus};
use serde::Serialize;

#[derive(Serialize)]
struct SaveArgs<'a> {
    settings: &'a EmailSettings,
    password: Option<&'a str>,
}

#[component]
pub fn Connections() -> impl IntoView {
    let (user, host, port, password) = (RwSignal::new(String::new()), RwSignal::new("imap.titan.email".to_owned()), RwSignal::new("993".to_owned()), RwSignal::new(String::new()));
    let configured = RwSignal::new(false);
    let password_saved = RwSignal::new(false);
    let failure = RwSignal::new(None::<String>);
    let notice = RwSignal::new(None::<String>);

    let refresh = move || {
        leptos::task::spawn_local(async move {
            match call::<EmailStatus>("email_status", ()).await {
                Ok(status) => {
                    password_saved.set(status.password_saved);
                    configured.set(status.settings.is_some());
                    if let Some(settings) = status.settings {
                        user.set(settings.user);
                        host.set(settings.host);
                        port.set(settings.port.to_string());
                    }
                }
                Err(message) => failure.set(Some(message)),
            }
        });
    };
    refresh();

    let save = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        let Ok(port_number) = port.get_untracked().trim().parse::<u16>() else { return failure.set(Some("Enter a port from 1 to 65535".into())) };
        let settings = EmailSettings { user: user.get_untracked().trim().to_owned(), host: host.get_untracked().trim().to_owned(), port: port_number };
        let typed = password.get_untracked();
        leptos::task::spawn_local(async move {
            let args = SaveArgs { settings: &settings, password: Some(typed.as_str()).filter(|p| !p.is_empty()) };
            match call::<()>("save_email", args).await {
                Ok(()) => {
                    password.set(String::new());
                    failure.set(None);
                    notice.set(None);
                    refresh();
                }
                Err(message) => failure.set(Some(message)),
            }
        });
    };

    let test = move |_| {
        leptos::task::spawn_local(async move {
            notice.set(Some("Testing…".into()));
            match call::<String>("test_email", ()).await {
                Ok(said) => notice.set(Some(said)),
                Err(message) => notice.set(Some(message)),
            }
        });
    };

    let remove = move |_| {
        leptos::task::spawn_local(async move {
            match call::<()>("remove_email", ()).await {
                Ok(()) => {
                    for field in [user, password] {
                        field.set(String::new());
                    }
                    notice.set(None);
                    failure.set(None);
                    refresh();
                }
                Err(message) => failure.set(Some(message)),
            }
        });
    };

    view! {
        <h1>"Connections"</h1>
        <h2>"Email"</h2>
        <form class="email-form" on:submit=save>
            <input aria-label="Address" placeholder="Mailbox address" autocomplete="off" spellcheck="false" prop:value=user on:input=move |ev| user.set(event_target_value(&ev)) />
            <input
                aria-label="Password"
                type="password"
                autocomplete="off"
                placeholder=move || if password_saved.get() { "••••••••••••" } else { "Password" }
                prop:value=password
                on:input=move |ev| password.set(event_target_value(&ev))
            />
            <input aria-label="IMAP server" placeholder="IMAP server" autocomplete="off" spellcheck="false" prop:value=host on:input=move |ev| host.set(event_target_value(&ev)) />
            <input aria-label="Port" placeholder="Port" inputmode="numeric" autocomplete="off" prop:value=port on:input=move |ev| port.set(event_target_value(&ev)) />
            <button type="submit" disabled=move || user.with(|u| u.trim().is_empty()) || (!password_saved.get() && password.with(String::is_empty))>"Save"</button>
            <Show when=move || configured.get()>
                <button type="button" class="secondary" on:click=test>"Test"</button>
                <button type="button" class="secondary" on:click=remove>"Remove"</button>
            </Show>
        </form>
        {move || failure.get().map(|message| view! { <p class="failure">{message}</p> })}
        {move || notice.get().map(|message| view! { <p class="notice">{message}</p> })}
    }
}
