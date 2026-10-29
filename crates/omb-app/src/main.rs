fn main() {
    // WebKitGTK aborts with a Wayland protocol error when its DMABUF renderer is on for some GPU/compositor pairs.
    #[cfg(target_os = "linux")]
    if std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_none() {
        // SAFETY: runs first in main, before any thread exists.
        unsafe { std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1") };
    }

    tauri::Builder::default()
        .run(tauri::generate_context!())
        .expect("failed to run OpenNoxBot");
}
