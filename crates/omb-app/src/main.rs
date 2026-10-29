use omb_core::secrets::{self, Secret};

async fn blocking<T: Send + 'static>(work: impl FnOnce() -> Result<T, String> + Send + 'static) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(work).await.map_err(|e| e.to_string())?
}

#[tauri::command]
async fn detect_engines() -> Vec<omb_wire::EngineStatus> {
    let mut host = omb_drivers::Host::from_process();
    host.jev_api_key = blocking(|| secrets::is_saved(Secret::Jev)).await.unwrap_or(false);
    omb_drivers::detect_all(&host).await
}

#[tauri::command]
async fn jev_key_saved() -> Result<bool, String> {
    blocking(|| secrets::is_saved(Secret::Jev)).await
}

#[tauri::command]
async fn save_jev_key(key: String) -> Result<(), String> {
    blocking(move || secrets::save(Secret::Jev, &key)).await
}

#[tauri::command]
async fn clear_jev_key() -> Result<(), String> {
    blocking(|| secrets::clear(Secret::Jev)).await
}

fn main() {
    // WebKitGTK aborts with a Wayland protocol error when its DMABUF renderer is on for some GPU/compositor pairs.
    #[cfg(target_os = "linux")]
    if std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_none() {
        // SAFETY: runs first in main, before any thread exists.
        unsafe { std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1") };
    }

    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![detect_engines, jev_key_saved, save_jev_key, clear_jev_key])
        .run(tauri::generate_context!())
        .expect("failed to run OpenNoxBot");
}
