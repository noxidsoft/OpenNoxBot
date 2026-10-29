use omb_core::recorder::TurnRecorder;
use omb_core::secrets::{self, Secret};
use omb_core::store::Store;
use omb_drivers::{Host, Turn, run_turn};
use omb_wire::{ApprovalMode, Bot, BotEvent, ChatMessage, EngineId, EngineStatus, MessageRole, RuntimeEvent};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, Manager, State};

pub struct AppState {
    pub store: Arc<Store>,
    pub data: PathBuf,
    pub running: Mutex<HashSet<i64>>,
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
    let (store, data) = (state.store.clone(), state.data.clone());
    let finished = app.clone();
    tauri::async_runtime::spawn(async move {
        play_turn(&app, &store, &data, &bot, &text).await;
        let state = finished.state::<AppState>();
        if let Ok(mut running) = state.running.lock() {
            running.remove(&bot.id);
        }
    });
    Ok(message)
}

async fn play_turn(app: &AppHandle, store: &Store, data: &std::path::Path, bot: &Bot, text: &str) {
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
    let (model, approval, prompt) = (bot.model.clone(), bot.approval, text.to_owned());
    let driver = tokio::spawn(async move {
        let turn = Turn { prompt: &prompt, model: model.as_deref(), approval, resume: resume.as_deref(), cwd: &cwd };
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
}
