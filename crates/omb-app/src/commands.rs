use omb_core::approvals::{Approvals, Decision};
use omb_core::ids::random_hex;
use omb_core::recorder::TurnRecorder;
use omb_core::secrets::{self, Secret};
use omb_core::store::Store;
use omb_drivers::{Host, McpAccess, Turn, run_turn};
use omb_mcp::{DISALLOWED_TOOLS, McpServer, OpenMailbox, PERMISSION_TOOL, TurnContext};
use omb_ssh::{ScannedKey, SshRunner};
use omb_wire::{ApprovalMode, Bot, BotEvent, ChatMessage, EmailSettings, EmailStatus, EngineId, EngineStatus, HostPolicy, HostStatus, MessageRole, NewHost, RuntimeEvent, SshHost, SshLogEntry};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, Manager, State};

pub struct AppState {
    pub store: Arc<Store>,
    pub data: PathBuf,
    pub running: Mutex<HashSet<i64>>,
    pub approvals: Arc<Approvals>,
    pub mcp: Arc<McpServer>,
    pub ssh: Arc<SshRunner>,
    pub scanned: Mutex<std::collections::HashMap<i64, Vec<ScannedKey>>>,
}

async fn blocking<T: Send + 'static>(work: impl FnOnce() -> Result<T, String> + Send + 'static) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(work).await.map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn detect_engines() -> Vec<EngineStatus> {
    let mut host = Host::from_process();
    host.jev_api_key = blocking(|| secrets::is_saved(Secret::Jev)).await.unwrap_or(false);
    omb_drivers::detect_all(&host).await
}

#[tauri::command]
pub async fn jev_key_saved() -> Result<bool, String> {
    blocking(|| secrets::is_saved(Secret::Jev)).await
}

#[tauri::command]
pub async fn save_jev_key(key: String) -> Result<(), String> {
    blocking(move || secrets::save(Secret::Jev, &key)).await
}

#[tauri::command]
pub async fn clear_jev_key() -> Result<(), String> {
    blocking(|| secrets::clear(Secret::Jev)).await
}

#[tauri::command]
pub fn list_bots(state: State<'_, AppState>) -> Result<Vec<Bot>, String> {
    state.store.bots()
}

#[tauri::command]
pub fn create_bot(state: State<'_, AppState>, name: String) -> Result<Bot, String> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 60 || name.chars().any(char::is_control) {
        return Err("Enter a name up to 60 characters".into());
    }
    state.store.create_bot(name, EngineId::Claude, None, ApprovalMode::Ask)
}

#[tauri::command]
pub fn delete_bot(state: State<'_, AppState>, bot_id: i64) -> Result<(), String> {
    if state.running.lock().is_ok_and(|running| running.contains(&bot_id)) {
        return Err("This bot is still answering".into());
    }
    state.store.delete_bot(bot_id)
}

#[tauri::command]
pub fn set_bot_approval(state: State<'_, AppState>, bot_id: i64, approval: ApprovalMode) -> Result<(), String> {
    state.store.set_approval(bot_id, approval)
}

#[tauri::command]
pub fn set_bot_model(state: State<'_, AppState>, bot_id: i64, model: Option<String>) -> Result<(), String> {
    state.store.set_model(bot_id, model.as_deref())
}

#[tauri::command]
pub fn list_messages(state: State<'_, AppState>, bot_id: i64) -> Result<Vec<ChatMessage>, String> {
    state.store.messages(bot_id)
}

#[tauri::command]
pub fn running_bots(state: State<'_, AppState>) -> Vec<i64> {
    state.running.lock().map(|running| running.iter().copied().collect()).unwrap_or_default()
}

#[tauri::command]
pub fn send_message(app: AppHandle, state: State<'_, AppState>, bot_id: i64, text: String) -> Result<ChatMessage, String> {
    let text = text.trim().to_owned();
    if text.is_empty() {
        return Err("Write a message first".into());
    }
    let bot = state.store.bot(bot_id)?.ok_or("This bot no longer exists")?;
    if !state.running.lock().map_err(|e| e.to_string())?.insert(bot_id) {
        return Err("This bot is still answering".into());
    }
    let message = match state.store.add_message(bot_id, MessageRole::User, &text) {
        Ok(message) => message,
        Err(error) => {
            if let Ok(mut running) = state.running.lock() {
                running.remove(&bot_id);
            }
            return Err(error);
        }
    };
    let (store, data, mcp) = (state.store.clone(), state.data.clone(), state.mcp.clone());
    let finished = app.clone();
    tauri::async_runtime::spawn(async move {
        play_turn(&app, &store, &data, &mcp, &bot, &text).await;
        let state = finished.state::<AppState>();
        if let Ok(mut running) = state.running.lock() {
            running.remove(&bot.id);
        }
    });
    Ok(message)
}

async fn play_turn(app: &AppHandle, store: &Store, data: &std::path::Path, mcp: &McpServer, bot: &Bot, text: &str) {
    let emit = |event: RuntimeEvent| {
        let _ = app.emit("runtime", BotEvent { bot_id: bot.id, event });
    };
    let host = Host::from_process();
    let Some(bin) = host.resolve("claude") else {
        let event = RuntimeEvent::RuntimeError { message: "claude is not installed".into() };
        let _ = TurnRecorder::new(store, bot.id).apply(&event);
        return emit(event);
    };
    let cwd = data.join("bots").join(bot.id.to_string());
    if let Err(error) = std::fs::create_dir_all(&cwd) {
        let event = RuntimeEvent::RuntimeError { message: format!("could not create the bot's folder: {error}") };
        let _ = TurnRecorder::new(store, bot.id).apply(&event);
        return emit(event);
    }
    let resume = store.session(bot.id).ok().flatten();

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let access = mcp.register(TurnContext { bot_id: bot.id, folder: cwd.clone(), events: tx.clone(), mail_seen: Default::default() });
    let config = match write_turn_config(data, &access.config()) {
        Ok(path) => path,
        Err(error) => {
            let event = RuntimeEvent::RuntimeError { message: format!("could not prepare the turn: {error}") };
            let _ = TurnRecorder::new(store, bot.id).apply(&event);
            return emit(event);
        }
    };
    let approval = if store.email().ok().flatten().is_some() { bot.approval.with_mail() } else { bot.approval };
    let (model, prompt, config_path) = (bot.model.clone(), text.to_owned(), config.clone());
    let driver = tokio::spawn(async move {
        let _access = access;
        let mcp = McpAccess { config: &config_path, permission_tool: PERMISSION_TOOL, disallowed_tools: &DISALLOWED_TOOLS };
        let turn = Turn { prompt: &prompt, model: model.as_deref(), approval, resume: resume.as_deref(), cwd: &cwd, mcp: Some(mcp) };
        run_turn(&bin, &host, &turn, &tx).await
    });

    let mut recorder = TurnRecorder::new(store, bot.id);
    while let Some(event) = rx.recv().await {
        if let Err(message) = recorder.apply(&event) {
            emit(RuntimeEvent::RuntimeError { message });
        }
        emit(event);
    }
    let _ = driver.await;
    let _ = std::fs::remove_file(&config);
    let _ = store.expire_unfinished(Some(bot.id), None);
}

/// The turn's MCP config holds its bearer token, so only the owner may read it.
fn write_turn_config(data: &std::path::Path, config: &serde_json::Value) -> std::io::Result<PathBuf> {
    use std::io::Write;
    let dir = data.join("run");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{}.json", random_hex(16)));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    options.open(&path)?.write_all(config.to_string().as_bytes())?;
    Ok(path)
}

#[tauri::command]
pub fn answer_request(state: State<'_, AppState>, request_id: String, allow: bool) -> Result<(), String> {
    let decision = if allow { Decision::Allow } else { Decision::Deny };
    if state.approvals.answer(&request_id, decision) { Ok(()) } else { Err("This request is no longer waiting".into()) }
}

#[tauri::command]
pub async fn list_hosts(state: State<'_, AppState>) -> Result<Vec<HostStatus>, String> {
    let mut statuses = Vec::new();
    for host in state.store.hosts()? {
        let trusted = state.ssh.is_trusted(&host).await;
        statuses.push(HostStatus { host, trusted });
    }
    Ok(statuses)
}

#[tauri::command]
pub fn add_host(state: State<'_, AppState>, host: NewHost) -> Result<SshHost, String> {
    state.store.add_host(&host)
}

#[tauri::command]
pub async fn set_host_policy(state: State<'_, AppState>, host_id: i64, policy: HostPolicy) -> Result<(), String> {
    state.store.set_host_policy(host_id, policy)?;
    if let Some(host) = state.store.host(host_id)? {
        state.ssh.disconnect(&host).await;
    }
    Ok(())
}

#[tauri::command]
pub async fn remove_host(state: State<'_, AppState>, host_id: i64) -> Result<(), String> {
    if let Some(host) = state.store.host(host_id)? {
        state.ssh.disconnect(&host).await;
    }
    state.store.remove_host(host_id)
}

/// Fetches the host's keys and keeps them until the person confirms the fingerprints they were shown.
#[tauri::command]
pub async fn scan_host(state: State<'_, AppState>, host_id: i64) -> Result<Vec<String>, String> {
    let host = state.store.host(host_id)?.ok_or("This host no longer exists")?;
    let keys = state.ssh.scan(&host).await?;
    let fingerprints = keys.iter().map(|key| key.fingerprint.clone()).collect();
    state.scanned.lock().map_err(|e| e.to_string())?.insert(host_id, keys);
    Ok(fingerprints)
}

#[tauri::command]
pub fn trust_host(state: State<'_, AppState>, host_id: i64) -> Result<(), String> {
    let keys = state.scanned.lock().map_err(|e| e.to_string())?.remove(&host_id).ok_or("Scan the host first")?;
    state.ssh.pin(&keys)
}

#[tauri::command]
pub async fn test_host(state: State<'_, AppState>, host_id: i64) -> Result<String, String> {
    let host = state.store.host(host_id)?.ok_or("This host no longer exists")?;
    let done = state.ssh.exec(&host, "true", std::time::Duration::from_secs(15)).await;
    match (done.error, done.timed_out, done.exit_code) {
        (Some(error), _, _) => Err(error),
        (_, true, _) => Err("Timed out".into()),
        (_, _, Some(0)) => Ok("Connected".into()),
        _ => Err(done.stderr.trim().lines().last().unwrap_or("Could not connect").to_owned()),
    }
}

#[tauri::command]
pub fn ssh_activity(state: State<'_, AppState>, limit: u32) -> Result<Vec<SshLogEntry>, String> {
    state.store.ssh_log(limit.min(200))
}

/// Reads the saved settings and keyring password each time, so a change in Settings applies to the next tool call.
pub fn mailbox_opener(store: Arc<Store>) -> OpenMailbox {
    Arc::new(move || {
        let settings = store.email()?.ok_or("Email is not set up. The person adds it in Settings → Connections.")?;
        let password = secrets::read(Secret::EmailPassword)?.ok_or("The mailbox password is missing. The person enters it in Settings → Connections.")?;
        let config = omb_email::Config { host: settings.host, port: settings.port, user: settings.user.clone(), password };
        Ok((Box::new(omb_email::ImapMailbox::connect(&config)?) as Box<dyn omb_email::Mailbox + Send>, settings.user))
    })
}

#[tauri::command]
pub async fn email_status(state: State<'_, AppState>) -> Result<EmailStatus, String> {
    let settings = state.store.email()?;
    let password_saved = blocking(|| secrets::is_saved(Secret::EmailPassword)).await?;
    Ok(EmailStatus { settings, password_saved })
}

/// A blank password keeps the saved one; the first save needs one.
#[tauri::command]
pub async fn save_email(state: State<'_, AppState>, settings: EmailSettings, password: Option<String>) -> Result<(), String> {
    settings.validate()?;
    let password = password.filter(|p| !p.is_empty());
    let store = state.store.clone();
    blocking(move || {
        match password {
            Some(password) => secrets::save(Secret::EmailPassword, &password)?,
            None if !secrets::is_saved(Secret::EmailPassword)? => return Err("Enter the mailbox password".into()),
            None => {}
        }
        store.set_email(&settings)
    })
    .await
}

#[tauri::command]
pub async fn remove_email(state: State<'_, AppState>) -> Result<(), String> {
    let store = state.store.clone();
    blocking(move || {
        store.clear_email()?;
        secrets::clear(Secret::EmailPassword)
    })
    .await
}

#[tauri::command]
pub async fn test_email(state: State<'_, AppState>) -> Result<String, String> {
    let open = mailbox_opener(state.store.clone());
    let work = tauri::async_runtime::spawn_blocking(move || {
        let (mut mailbox, _) = open()?;
        mailbox.list().map(|folders| format!("Connected, {} folders", folders.len()))
    });
    match tokio::time::timeout(std::time::Duration::from_secs(30), work).await {
        Ok(done) => done.map_err(|e| e.to_string())?,
        Err(_) => Err("The mail server did not answer".into()),
    }
}
