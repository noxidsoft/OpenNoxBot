use crate::api::call;
use leptos::prelude::*;
use omb_wire::{AuthState, EngineId, EngineStatus};

fn label(id: EngineId) -> &'static str {
    match id {
        EngineId::Claude => "Claude",
        EngineId::Codex => "Codex",
        EngineId::OpenaiApi => "OpenAI API",
        EngineId::Jev => "Jev",
    }
}

fn state(engine: &EngineStatus) -> &'static str {
    match (engine.available, engine.auth) {
        (false, _) => "Not installed",
        (true, AuthState::SignedIn) => "Signed in",
        (true, AuthState::SignedOut) => "Signed out",
        (true, AuthState::Unknown) => "Unknown",
    }
}

#[component]
pub fn Engines() -> impl IntoView {
    let (engines, set_engines) = signal(Vec::<EngineStatus>::new());
    let (failure, set_failure) = signal(None::<String>);
    leptos::task::spawn_local(async move {
        match call::<Vec<EngineStatus>>("detect_engines", ()).await {
            Ok(found) => set_engines.set(found),
            Err(message) => set_failure.set(Some(message)),
        }
    });

    view! {
        <h1>"Engines"</h1>
        {move || failure.get().map(|message| view! { <p class="failure">{message}</p> })}
        <ul class="engines">
            <For each=move || engines.get() key=|e| e.id children=|e| {
                let detail = [e.account.clone(), e.version.clone()].into_iter().flatten().collect::<Vec<_>>().join(" · ");
                view! {
                    <li data-state=state(&e)>
                        <strong>{label(e.id)}</strong>
                        <span>{state(&e)}</span>
                        <small>{detail}</small>
                    </li>
                }
            } />
        </ul>
    }
}
