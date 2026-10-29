use omb_wire::{ApprovalMode, ApprovalRequest, ApprovalState, Bot, ChatMessage, EngineId, MessageRole};
use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::{Serialize, de::DeserializeOwned};
use std::path::Path;
use std::sync::Mutex;

pub struct Store {
    conn: Mutex<Connection>,
}

fn text_of<T: Serialize>(value: T) -> String {
    match serde_json::to_value(value) {
        Ok(serde_json::Value::String(text)) => text,
        _ => String::new(),
    }
}

fn value_of<T: DeserializeOwned>(text: String) -> rusqlite::Result<T> {
    serde_json::from_value(serde_json::Value::String(text))
        .map_err(|e| rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e)))
}

fn bot_of(row: &Row) -> rusqlite::Result<Bot> {
    Ok(Bot { id: row.get(0)?, name: row.get(1)?, engine: value_of(row.get(2)?)?, model: row.get(3)?, approval: value_of(row.get(4)?)? })
}

fn message_of(row: &Row) -> rusqlite::Result<ChatMessage> {
    Ok(ChatMessage { id: row.get(0)?, bot_id: row.get(1)?, role: value_of(row.get(2)?)?, body: row.get(3)? })
}

fn failed(error: rusqlite::Error) -> String {
    error.to_string()
}

impl Store {
    pub fn open(path: &Path) -> Result<Self, String> {
        Self::from(Connection::open(path).map_err(failed)?)
    }

    pub fn in_memory() -> Result<Self, String> {
        Self::from(Connection::open_in_memory().map_err(failed)?)
    }

    fn from(conn: Connection) -> Result<Self, String> {
        conn.execute_batch(
            "PRAGMA foreign_keys = ON;
             CREATE TABLE IF NOT EXISTS bots (
                 id INTEGER PRIMARY KEY,
                 name TEXT NOT NULL,
                 engine TEXT NOT NULL,
                 model TEXT,
                 approval TEXT NOT NULL,
                 session_id TEXT
             );
             CREATE TABLE IF NOT EXISTS messages (
                 id INTEGER PRIMARY KEY,
                 bot_id INTEGER NOT NULL REFERENCES bots(id) ON DELETE CASCADE,
                 role TEXT NOT NULL,
                 body TEXT NOT NULL
             );
             CREATE INDEX IF NOT EXISTS messages_by_bot ON messages(bot_id, id);",
        )
        .map_err(failed)?;
        Ok(Self { conn: Mutex::new(conn) })
    }

    fn conn(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn create_bot(&self, name: &str, engine: EngineId, model: Option<&str>, approval: ApprovalMode) -> Result<Bot, String> {
        let conn = self.conn();
        conn.execute(
            "INSERT INTO bots (name, engine, model, approval) VALUES (?1, ?2, ?3, ?4)",
            params![name, text_of(engine), model, text_of(approval)],
        )
        .map_err(failed)?;
        Ok(Bot { id: conn.last_insert_rowid(), name: name.to_owned(), engine, model: model.map(str::to_owned), approval })
    }

    pub fn bots(&self) -> Result<Vec<Bot>, String> {
        let conn = self.conn();
        let mut statement = conn.prepare("SELECT id, name, engine, model, approval FROM bots ORDER BY id").map_err(failed)?;
        statement.query_map([], bot_of).map_err(failed)?.collect::<Result<_, _>>().map_err(failed)
    }

    pub fn bot(&self, id: i64) -> Result<Option<Bot>, String> {
        self.conn()
            .query_row("SELECT id, name, engine, model, approval FROM bots WHERE id = ?1", [id], bot_of)
            .optional()
            .map_err(failed)
    }

    pub fn delete_bot(&self, id: i64) -> Result<(), String> {
        self.conn().execute("DELETE FROM bots WHERE id = ?1", [id]).map(|_| ()).map_err(failed)
    }

    pub fn set_approval(&self, id: i64, approval: ApprovalMode) -> Result<(), String> {
        self.conn().execute("UPDATE bots SET approval = ?2 WHERE id = ?1", params![id, text_of(approval)]).map(|_| ()).map_err(failed)
    }

    pub fn set_model(&self, id: i64, model: Option<&str>) -> Result<(), String> {
        self.conn().execute("UPDATE bots SET model = ?2 WHERE id = ?1", params![id, model]).map(|_| ()).map_err(failed)
    }

    pub fn session(&self, id: i64) -> Result<Option<String>, String> {
        self.conn()
            .query_row("SELECT session_id FROM bots WHERE id = ?1", [id], |row| row.get(0))
            .optional()
            .map(Option::flatten)
            .map_err(failed)
    }

    pub fn set_session(&self, id: i64, session: &str) -> Result<(), String> {
        self.conn().execute("UPDATE bots SET session_id = ?2 WHERE id = ?1", params![id, session]).map(|_| ()).map_err(failed)
    }

    pub fn add_message(&self, bot_id: i64, role: MessageRole, body: &str) -> Result<ChatMessage, String> {
        let conn = self.conn();
        conn.execute("INSERT INTO messages (bot_id, role, body) VALUES (?1, ?2, ?3)", params![bot_id, text_of(role), body]).map_err(failed)?;
        Ok(ChatMessage { id: conn.last_insert_rowid(), bot_id, role, body: body.to_owned() })
    }

    pub fn set_message_body(&self, id: i64, body: &str) -> Result<(), String> {
        self.conn().execute("UPDATE messages SET body = ?2 WHERE id = ?1", params![id, body]).map(|_| ()).map_err(failed)
    }

    /// A request nobody can answer any more (its turn ended, or the app was closed) is recorded as denied.
    pub fn expire_pending_approvals(&self) -> Result<(), String> {
        let pending: Vec<(i64, ApprovalRequest)> = {
            let conn = self.conn();
            let mut statement = conn.prepare("SELECT id, body FROM messages WHERE role = ?1").map_err(failed)?;
            let rows = statement.query_map([text_of(MessageRole::Approval)], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))).map_err(failed)?;
            rows.filter_map(Result::ok)
                .filter_map(|(id, body)| serde_json::from_str::<ApprovalRequest>(&body).ok().map(|request| (id, request)))
                .filter(|(_, request)| request.state == ApprovalState::Pending)
                .collect()
        };
        for (id, mut request) in pending {
            request.state = ApprovalState::Denied;
            self.set_message_body(id, &serde_json::to_string(&request).unwrap_or_default())?;
        }
        Ok(())
    }

    pub fn messages(&self, bot_id: i64) -> Result<Vec<ChatMessage>, String> {
        let conn = self.conn();
        let mut statement = conn.prepare("SELECT id, bot_id, role, body FROM messages WHERE bot_id = ?1 ORDER BY id").map_err(failed)?;
        statement.query_map([bot_id], message_of).map_err(failed)?.collect::<Result<_, _>>().map_err(failed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bots_round_trip_their_settings() {
        let store = Store::in_memory().unwrap();
        let bot = store.create_bot("Skye", EngineId::Claude, Some("haiku"), ApprovalMode::Ask).unwrap();
        assert_eq!(store.bots().unwrap(), vec![bot.clone()]);
        store.set_approval(bot.id, ApprovalMode::Edits).unwrap();
        store.set_model(bot.id, None).unwrap();
        let updated = store.bot(bot.id).unwrap().unwrap();
        assert_eq!((updated.approval, updated.model), (ApprovalMode::Edits, None));
        assert_eq!(store.bot(999).unwrap(), None);
    }

    #[test]
    fn messages_keep_order_and_belong_to_their_bot() {
        let store = Store::in_memory().unwrap();
        let a = store.create_bot("A", EngineId::Claude, None, ApprovalMode::Ask).unwrap();
        let b = store.create_bot("B", EngineId::Claude, None, ApprovalMode::Ask).unwrap();
        store.add_message(a.id, MessageRole::User, "one").unwrap();
        store.add_message(b.id, MessageRole::User, "other").unwrap();
        let second = store.add_message(a.id, MessageRole::Assistant, "two").unwrap();
        store.set_message_body(second.id, "two!").unwrap();
        let bodies: Vec<String> = store.messages(a.id).unwrap().into_iter().map(|m| m.body).collect();
        assert_eq!(bodies, ["one", "two!"]);
    }

    #[test]
    fn unanswered_requests_expire_as_denied_and_answered_ones_are_kept() {
        let store = Store::in_memory().unwrap();
        let bot = store.create_bot("A", EngineId::Claude, None, ApprovalMode::Ask).unwrap();
        let body = |state| serde_json::to_string(&ApprovalRequest { request_id: "r".into(), tool: "Bash".into(), input: serde_json::json!({}), state }).unwrap();
        store.add_message(bot.id, MessageRole::Approval, &body(ApprovalState::Pending)).unwrap();
        store.add_message(bot.id, MessageRole::Approval, &body(ApprovalState::Allowed)).unwrap();
        store.expire_pending_approvals().unwrap();
        let states: Vec<ApprovalState> = store.messages(bot.id).unwrap().iter().map(|m| serde_json::from_str::<ApprovalRequest>(&m.body).unwrap().state).collect();
        assert_eq!(states, [ApprovalState::Denied, ApprovalState::Allowed]);
    }

    #[test]
    fn sessions_start_empty_and_deleting_a_bot_removes_its_messages() {
        let store = Store::in_memory().unwrap();
        let bot = store.create_bot("A", EngineId::Claude, None, ApprovalMode::Ask).unwrap();
        assert_eq!(store.session(bot.id).unwrap(), None);
        store.set_session(bot.id, "s1").unwrap();
        assert_eq!(store.session(bot.id).unwrap().as_deref(), Some("s1"));
        store.add_message(bot.id, MessageRole::User, "x").unwrap();
        store.delete_bot(bot.id).unwrap();
        assert!(store.messages(bot.id).unwrap().is_empty());
        assert!(store.bots().unwrap().is_empty());
    }
}
