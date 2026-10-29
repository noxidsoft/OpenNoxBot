use serde::{Serialize, de::DeserializeOwned};
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = ["window", "__TAURI__", "core"], catch)]
    async fn invoke(cmd: &str, args: JsValue) -> Result<JsValue, JsValue>;
}

pub async fn call<T: DeserializeOwned>(cmd: &str, args: impl Serialize) -> Result<T, String> {
    let args = serde_wasm_bindgen::to_value(&args).map_err(|e| e.to_string())?;
    let value = invoke(cmd, args).await.map_err(|e| e.as_string().unwrap_or_else(|| format!("{e:?}")))?;
    serde_wasm_bindgen::from_value(value).map_err(|e| e.to_string())
}

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = ["window", "__TAURI__", "event"], catch)]
    async fn listen(event: &str, handler: &Closure<dyn FnMut(JsValue)>) -> Result<JsValue, JsValue>;
}

/// Subscribes for the life of the window.
pub fn on_event<T: serde::de::DeserializeOwned + 'static>(name: &'static str, mut handler: impl FnMut(T) + 'static) {
    let closure = Closure::<dyn FnMut(JsValue)>::new(move |event: JsValue| {
        let payload = js_sys::Reflect::get(&event, &JsValue::from_str("payload")).unwrap_or(JsValue::UNDEFINED);
        if let Ok(value) = serde_wasm_bindgen::from_value::<T>(payload) {
            handler(value);
        }
    });
    leptos::task::spawn_local(async move {
        let _ = listen(name, &closure).await;
        closure.forget();
    });
}
