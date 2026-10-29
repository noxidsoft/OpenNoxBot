use leptos::prelude::*;
use omb_wire::{AuthState, EngineId, EngineStatus};
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = ["window", "__TAURI__", "core"], catch)]
    async fn invoke(cmd: &str) -> Result<JsValue, JsValue>;
}

async fn detect_engines() -> Result<Vec<EngineStatus>, String> {
    let value = invoke("detect_engines").await.map_err(|e| format!("{e:?}"))?;
    serde_wasm_bindgen::from_value(value).map_err(|e| e.to_string())
}

fn label(id: EngineId) -> &'static str {
    match id {
        EngineId::Claude => "Claude",
        EngineId::Codex => "Codex",
        EngineId::OpenaiApi => "OpenAI API",
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
fn App() -> impl IntoView {
    let (engines, set_engines) = signal(Vec::<EngineStatus>::new());
    let (failure, set_failure) = signal(None::<String>);
    leptos::task::spawn_local(async move {
        match detect_engines().await {
            Ok(found) => set_engines.set(found),
            Err(message) => set_failure.set(Some(message)),
        }
    });

    view! {
        <main>
            <h1>"Engines"</h1>
            {move || failure.get().map(|message| view! { <p class="failure">{message}</p> })}
            <ul>
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
        </main>
    }
}

fn main() {
    mount_to_body(App);
}
