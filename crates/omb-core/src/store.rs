use omb_wire::{ApprovalMode, ApprovalRequest, ApprovalState, Bot, ChatMessage, EmailSettings, EngineId, HostPolicy, MessageRole, NewHost, SshHost, SshLogEntry, SshOutcome, ToolCall};
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

fn host_of(row: &Row) -> rusqlite::Result<SshHost> {
    Ok(SshHost { id: row.get(0)?, name: row.get(1)?, target: row.get(2)?, user: row.get(3)?, port: row.get(4)?, policy: value_of(row.get(5)?)? })
}

fn log_of(row: &Row) -> rusqlite::Result<SshLogEntry> {
    Ok(SshLogEntry {
        id: row.get(0)?,
        at: row.get(1)?,
        bot_id: row.get(2)?,
        host: row.get(3)?,
        command: row.get(4)?,
        outcome: value_of(row.get(5)?)?,
        exit_code: row.get(6)?,
        millis: row.get(7)?,
    })
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
             CREATE INDEX IF NOT EXISTS messages_by_bot ON messages(bot_id, id);
             CREATE TABLE IF NOT EXISTS hosts (
                 id INTEGER PRIMARY KEY,
                 name TEXT NOT NULL UNIQUE,
                 target TEXT NOT NULL,
                 user TEXT,
                 port INTEGER,
                 policy TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS email (
                 id INTEGER PRIMARY KEY CHECK (id = 1),
                 user TEXT NOT NULL,
                 host TEXT NOT NULL,
                 port INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS ssh_log (
                 id INTEGER PRIMARY KEY,
                 at INTEGER NOT NULL,
                 bot_id INTEGER,
                 host TEXT NOT NULL,
                 command TEXT NOT NULL,
                 outcome TEXT NOT NULL,
                 exit_code INTEGER,
                 millis INTEGER
             );",
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

    /// Work nobody can finish any more (its turn ended, or the app was closed) is closed out: a waiting request is recorded
    /// as denied and a running tool call as failed. `note` is posted to each affected bot's conversation.
    pub fn expire_unfinished(&self, bot: Option<i64>, note: Option<&str>) -> Result<(), String> {
        let rows: Vec<(i64, i64, String, String)> = {
            let conn = self.conn();
            let mut statement = conn.prepare("SELECT id, bot_id, role, body FROM messages WHERE role IN (?1, ?2)").map_err(failed)?;
            let found = statement
                .query_map(params![text_of(MessageRole::Approval), text_of(MessageRole::Tool)], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)))
                .map_err(failed)?;
            found.filter_map(Result::ok).filter(|(_, bot_id, _, _)| bot.is_none_or(|only| only == *bot_id)).collect()
        };
        let mut affected = std::collections::BTreeSet::new();
        for (id, bot_id, role, body) in rows {
            let closed = if role == text_of(MessageRole::Approval) {
                serde_json::from_str::<ApprovalRequest>(&body).ok().filter(|r| r.state == ApprovalState::Pending).map(|mut r| {
                    r.state = ApprovalState::Denied;
                    serde_json::to_string(&r).unwrap_or_default()
                })
            } else {
                serde_json::from_str::<ToolCall>(&body).ok().filter(|c| c.ok.is_none()).map(|mut c| {
                    c.ok = Some(false);
                    c.output = "Interrupted".into();
                    serde_json::to_string(&c).unwrap_or_default()
                })
            };
            if let Some(closed) = closed {
                self.set_message_body(id, &closed)?;
                affected.insert(bot_id);
            }
        }
        if let Some(note) = note {
            for bot_id in affected {
                self.add_message(bot_id, MessageRole::Error, note)?;
            }
        }
        Ok(())
    }

    pub fn add_host(&self, host: &NewHost) -> Result<SshHost, String> {
        host.validate()?;
        let conn = self.conn();
        conn.execute(
            "INSERT INTO hosts (name, target, user, port, policy) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![host.name, host.target, host.user, host.port, text_of(host.policy)],
        )
        .map_err(|e| match e {
            rusqlite::Error::SqliteFailure(code, _) if code.code == rusqlite::ErrorCode::ConstraintViolation => "A host with that name already exists".to_owned(),
            other => failed(other),
        })?;
        Ok(SshHost { id: conn.last_insert_rowid(), name: host.name.clone(), target: host.target.clone(), user: host.user.clone(), port: host.port, policy: host.policy })
    }

    pub fn hosts(&self) -> Result<Vec<SshHost>, String> {
        let conn = self.conn();
        let mut statement = conn.prepare("SELECT id, name, target, user, port, policy FROM hosts ORDER BY name").map_err(failed)?;
        statement.query_map([], host_of).map_err(failed)?.collect::<Result<_, _>>().map_err(failed)
    }

    pub fn host_by_name(&self, name: &str) -> Result<Option<SshHost>, String> {
        self.conn().query_row("SELECT id, name, target, user, port, policy FROM hosts WHERE name = ?1", [name], host_of).optional().map_err(failed)
    }

    pub fn host(&self, id: i64) -> Result<Option<SshHost>, String> {
        self.conn().query_row("SELECT id, name, target, user, port, policy FROM hosts WHERE id = ?1", [id], host_of).optional().map_err(failed)
    }

    pub fn remove_host(&self, id: i64) -> Result<(), String> {
        self.conn().execute("DELETE FROM hosts WHERE id = ?1", [id]).map(|_| ()).map_err(failed)
    }

    pub fn set_host_policy(&self, id: i64, policy: HostPolicy) -> Result<(), String> {
        self.conn().execute("UPDATE hosts SET policy = ?2 WHERE id = ?1", params![id, text_of(policy)]).map(|_| ()).map_err(failed)
    }

    pub fn email(&self) -> Result<Option<EmailSettings>, String> {
        self.conn()
            .query_row("SELECT user, host, port FROM email WHERE id = 1", [], |row| Ok(EmailSettings { user: row.get(0)?, host: row.get(1)?, port: row.get(2)? }))
            .optional()
            .map_err(failed)
    }

    pub fn set_email(&self, settings: &EmailSettings) -> Result<(), String> {
        settings.validate()?;
        self.conn()
            .execute("INSERT OR REPLACE INTO email (id, user, host, port) VALUES (1, ?1, ?2, ?3)", params![settings.user, settings.host, settings.port])
            .map(|_| ())
            .map_err(failed)
    }

    pub fn clear_email(&self) -> Result<(), String> {
        self.conn().execute("DELETE FROM email", []).map(|_| ()).map_err(failed)
    }

    pub fn log_ssh(&self, bot_id: Option<i64>, host: &str, command: &str, outcome: SshOutcome, exit_code: Option<i32>, millis: Option<i64>) -> Result<(), String> {
        let at = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or_default();
        self.conn()
            .execute(
                "INSERT INTO ssh_log (at, bot_id, host, command, outcome, exit_code, millis) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![at, bot_id, host, command, text_of(outcome), exit_code, millis],
            )
            .map(|_| ())
            .map_err(failed)
    }

    /// Newest first.
    pub fn ssh_log(&self, limit: u32) -> Result<Vec<SshLogEntry>, String> {
        let conn = self.conn();
        let mut statement = conn
            .prepare("SELECT id, at, bot_id, host, command, outcome, exit_code, millis FROM ssh_log ORDER BY id DESC LIMIT ?1")
            .map_err(failed)?;
        statement.query_map([limit], log_of).map_err(failed)?.collect::<Result<_, _>>().map_err(failed)
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
    fn unfinished_work_is_closed_out_for_the_bots_asked_about_and_noted_once() {
        let store = Store::in_memory().unwrap();
        let a = store.create_bot("A", EngineId::Claude, None, ApprovalMode::Ask).unwrap();
        let b = store.create_bot("B", EngineId::Claude, None, ApprovalMode::Ask).unwrap();
        let request = |state| serde_json::to_string(&ApprovalRequest { request_id: "r".into(), tool: "Bash".into(), input: serde_json::json!({}), state }).unwrap();
        let call = |ok| serde_json::to_string(&ToolCall { name: "Bash".into(), input: serde_json::json!({}), ok, output: String::new() }).unwrap();
        for bot in [&a, &b] {
            store.add_message(bot.id, MessageRole::Approval, &request(ApprovalState::Pending)).unwrap();
            store.add_message(bot.id, MessageRole::Tool, &call(None)).unwrap();
            store.add_message(bot.id, MessageRole::Tool, &call(Some(true))).unwrap();
        }

        store.expire_unfinished(Some(a.id), None).unwrap();
        let of = |bot: &Bot| store.messages(bot.id).unwrap();
        assert_eq!(of(&a).len(), 3, "no note unless asked");
        let first: ApprovalRequest = serde_json::from_str(&of(&a)[0].body).unwrap();
        assert_eq!(first.state, ApprovalState::Denied);
        let stuck: ToolCall = serde_json::from_str(&of(&a)[1].body).unwrap();
        assert_eq!((stuck.ok, stuck.output.as_str()), (Some(false), "Interrupted"));
        let done: ToolCall = serde_json::from_str(&of(&a)[2].body).unwrap();
        assert_eq!(done.ok, Some(true), "finished calls are left alone");
        let other: ApprovalRequest = serde_json::from_str(&of(&b)[0].body).unwrap();
        assert_eq!(other.state, ApprovalState::Pending, "another bot's turn may still be running");

        store.expire_unfinished(None, Some("The app closed before this turn finished")).unwrap();
        assert_eq!(of(&b).last().map(|m| (m.role, m.body.as_str())), Some((MessageRole::Error, "The app closed before this turn finished")));
        assert_eq!(of(&a).len(), 3, "a was already closed out, so it gets no note");
    }

    #[test]
    fn the_mailbox_settings_are_validated_replaced_and_cleared() {
        let store = Store::in_memory().unwrap();
        assert_eq!(store.email().unwrap(), None);
        let settings = |user: &str, host: &str, port| EmailSettings { user: user.into(), host: host.into(), port };
        store.set_email(&settings("me@example.com", "imap.titan.email", 993)).unwrap();
        store.set_email(&settings("you@example.com", "imap.example.com", 143)).unwrap();
        assert_eq!(store.email().unwrap(), Some(settings("you@example.com", "imap.example.com", 143)));
        for bad in [settings("nobody", "h.com", 993), settings("a@b.com", "-h", 993), settings("a@b.com", "h h", 993), settings("a@b.com", "h.com", 0)] {
            assert!(store.set_email(&bad).is_err());
        }
        store.clear_email().unwrap();
        assert_eq!(store.email().unwrap(), None);
    }

    #[test]
    fn hosts_are_validated_unique_and_updatable() {
        let store = Store::in_memory().unwrap();
        let new = |name: &str| NewHost { name: name.into(), target: "10.0.0.5".into(), user: Some("deploy".into()), port: Some(2222), policy: HostPolicy::Ask };
        let web = store.add_host(&new("web")).unwrap();
        store.add_host(&new("db")).unwrap();
        assert_eq!(store.add_host(&new("web")), Err("A host with that name already exists".into()));
        assert!(store.add_host(&NewHost { target: "-oProxyCommand=x".into(), ..new("bad") }).is_err());
        assert_eq!(store.hosts().unwrap().iter().map(|h| h.name.as_str()).collect::<Vec<_>>(), ["db", "web"]);
        store.set_host_policy(web.id, HostPolicy::ReadOnly).unwrap();
        assert_eq!(store.host_by_name("web").unwrap().unwrap().policy, HostPolicy::ReadOnly);
        assert_eq!(store.host(web.id).unwrap().unwrap().port, Some(2222));
        store.remove_host(web.id).unwrap();
        assert_eq!(store.host_by_name("web").unwrap(), None);
    }

    #[test]
    fn the_ssh_log_lists_newest_first() {
        let store = Store::in_memory().unwrap();
        store.log_ssh(Some(1), "web", "uptime", SshOutcome::Ran, Some(0), Some(120)).unwrap();
        store.log_ssh(Some(1), "web", "rm -rf /", SshOutcome::Declined, None, None).unwrap();
        let log = store.ssh_log(10).unwrap();
        assert_eq!(log.iter().map(|e| (e.command.as_str(), e.outcome)).collect::<Vec<_>>(), [("rm -rf /", SshOutcome::Declined), ("uptime", SshOutcome::Ran)]);
        assert_eq!(store.ssh_log(1).unwrap().len(), 1);
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
