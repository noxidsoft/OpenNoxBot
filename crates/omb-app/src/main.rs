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
            store.expire_unfinished(None, Some("The app closed before this turn finished"))?;
            let approvals = Arc::new(omb_core::approvals::Approvals::default());
            let control = std::env::var_os("XDG_RUNTIME_DIR").map(std::path::PathBuf::from).unwrap_or_else(|| std::path::PathBuf::from("/tmp"));
            let owner = std::os::unix::fs::MetadataExt::uid(&std::fs::metadata(&data)?);
            let ssh = Arc::new(omb_ssh::SshRunner::new(data.join("known_hosts"), control.join(format!("nox-ssh-{owner}"))));
            let services = omb_mcp::Services { approvals: approvals.clone(), store: store.clone(), ssh: ssh.clone(), mailbox: commands::mailbox_opener(store.clone()) };
            let mcp = Arc::new(tauri::async_runtime::block_on(omb_mcp::McpServer::start(services))?);
            app.manage(AppState { store, data, running: Mutex::new(HashSet::new()), approvals, mcp, ssh, scanned: Mutex::new(Default::default()) });
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
            commands::list_hosts,
            commands::add_host,
            commands::set_host_policy,
            commands::remove_host,
            commands::scan_host,
            commands::trust_host,
            commands::test_host,
            commands::ssh_activity,
            commands::email_status,
            commands::save_email,
            commands::remove_email,
            commands::test_email,
            commands::usage_report,
            commands::spend_limits,
            commands::set_spend_limits,
        ])
        .run(tauri::generate_context!())
        .expect("failed to run OpenNoxBot");
}
