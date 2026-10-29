mod commands;

use commands::AppState;
use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use tauri::Manager;

fn main() {
    // WebKitGTK aborts with a Wayland protocol error when its DMABUF renderer is on for some GPU/compositor pairs.
    #[cfg(target_os = "linux")]
    if std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_none() {
        // SAFETY: runs first in main, before any thread exists.
        unsafe { std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1") };
    }

    tauri::Builder::default()
        .setup(|app| {
            let data = match std::env::var_os("OPENNOXBOT_DATA") {
                Some(dir) => std::path::PathBuf::from(dir),
                None => app.path().app_data_dir()?,
            };
            std::fs::create_dir_all(&data)?;
            let store = Arc::new(omb_core::store::Store::open(&data.join("opennoxbot.db"))?);
            store.expire_pending_approvals()?;
            let approvals = Arc::new(omb_core::approvals::Approvals::default());
            let mcp = Arc::new(tauri::async_runtime::block_on(omb_mcp::McpServer::start(approvals.clone()))?);
            app.manage(AppState { store, data, running: Mutex::new(HashSet::new()), approvals, mcp });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::detect_engines,
            commands::jev_key_saved,
            commands::save_jev_key,
            commands::clear_jev_key,
            commands::list_bots,
            commands::create_bot,
            commands::delete_bot,
            commands::set_bot_approval,
            commands::set_bot_model,
            commands::list_messages,
            commands::running_bots,
            commands::send_message,
            commands::answer_request,
        ])
        .run(tauri::generate_context!())
        .expect("failed to run OpenNoxBot");
}
