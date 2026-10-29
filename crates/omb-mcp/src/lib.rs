//! The MCP server `claude` calls for permission decisions, on loopback with a bearer token per turn.
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use omb_core::approvals::{Approvals, Decision};
use omb_core::ids::random_hex;
use omb_core::policy::own_reads::reads_own_files;
use omb_core::store::Store;
use omb_ssh::SshRunner;
use omb_wire::RuntimeEvent;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc::UnboundedSender;

mod email_tools;
mod ssh_tools;

pub const SERVER_NAME: &str = "nox";
pub const PERMISSION_TOOL: &str = "mcp__nox__approve";

/// Refused by the CLI itself, whatever its approval mode, because the permission callback never runs in Full access.
pub const DISALLOWED_TOOLS: [&str; 5] = ["Bash(ssh:*)", "Bash(scp:*)", "Bash(sftp:*)", "Bash(sshfs:*)", "Bash(ssh-copy-id:*)"];

pub struct TurnContext {
    pub bot_id: i64,
    pub folder: PathBuf,
    pub events: UnboundedSender<RuntimeEvent>,
}

/// Opens the person's mailbox, with the address drafts are written from. Fails when email is not set up.
pub type OpenMailbox = Arc<dyn Fn() -> Result<(Box<dyn omb_email::Mailbox + Send>, String), String> + Send + Sync>;

pub struct Services {
    pub approvals: Arc<Approvals>,
    pub store: Arc<Store>,
    pub ssh: Arc<SshRunner>,
    pub mailbox: OpenMailbox,
}

struct Shared {
    turns: Mutex<HashMap<String, Arc<TurnContext>>>,
    approvals: Arc<Approvals>,
    store: Arc<Store>,
    ssh: Arc<SshRunner>,
    mailbox: OpenMailbox,
}

pub struct McpServer {
    shared: Arc<Shared>,
    port: u16,
    task: tokio::task::JoinHandle<()>,
}

/// What one turn's `claude` needs to reach the server; the token stops working when this is dropped.
pub struct TurnAccess {
    shared: Arc<Shared>,
    token: String,
    port: u16,
}

impl McpServer {
    pub async fn start(services: Services) -> std::io::Result<Self> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let port = listener.local_addr()?.port();
        let shared = Arc::new(Shared { turns: Mutex::new(HashMap::new()), approvals: services.approvals, store: services.store, ssh: services.ssh, mailbox: services.mailbox });
        let app = router(shared.clone());
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        Ok(Self { shared, port, task })
    }

    pub fn register(&self, turn: TurnContext) -> TurnAccess {
        let token = random_hex(32);
        self.shared.turns.lock().unwrap_or_else(|p| p.into_inner()).insert(token.clone(), Arc::new(turn));
        TurnAccess { shared: self.shared.clone(), token, port: self.port }
    }
}

impl Drop for McpServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl TurnAccess {
    /// The `--mcp-config` document for this turn.
    pub fn config(&self) -> Value {
        json!({ "mcpServers": { SERVER_NAME: {
            "type": "http",
            "url": format!("http://127.0.0.1:{}/mcp", self.port),
            "headers": { "Authorization": format!("Bearer {}", self.token) },
        } } })
    }
}

impl Drop for TurnAccess {
    fn drop(&mut self) {
        self.shared.turns.lock().unwrap_or_else(|p| p.into_inner()).remove(&self.token);
    }
}

fn router(shared: Arc<Shared>) -> Router {
    Router::new().route("/mcp", post(handle).get(|| async { StatusCode::METHOD_NOT_ALLOWED })).with_state(shared)
}

fn reply(id: &Value, result: Value) -> Response {
    Json(json!({ "jsonrpc": "2.0", "id": id, "result": result })).into_response()
}

fn failure(id: &Value, code: i32, message: &str) -> Response {
    Json(json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })).into_response()
}

async fn handle(State(shared): State<Arc<Shared>>, headers: HeaderMap, body: Bytes) -> Response {
    let token = headers.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()).and_then(|v| v.strip_prefix("Bearer "));
    let turn = token.and_then(|t| shared.turns.lock().unwrap_or_else(|p| p.into_inner()).get(t).cloned());
    let Some(turn) = turn else { return StatusCode::UNAUTHORIZED.into_response() };
    let Ok(message) = serde_json::from_slice::<Value>(&body) else { return failure(&Value::Null, -32700, "parse error") };
    let id = message.get("id").cloned();
    let Some(id) = id else { return StatusCode::ACCEPTED.into_response() };
    match message["method"].as_str() {
        Some("initialize") => reply(
            &id,
            json!({
                "protocolVersion": message["params"]["protocolVersion"].as_str().unwrap_or("2025-06-18"),
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "opennoxbot", "version": env!("CARGO_PKG_VERSION") },
            }),
        ),
        Some("ping") => reply(&id, json!({})),
        Some("tools/list") => {
            let mut tools = vec![approve_tool()];
            tools.extend(ssh_tools::definitions());
            tools.extend(email_tools::definitions());
            reply(&id, json!({ "tools": tools }))
        }
        Some("tools/call") => match message["params"]["name"].as_str() {
            Some("approve") => {
                let text = approve(&turn, &shared.approvals, &message["params"]["arguments"]).await;
                reply(&id, json!({ "content": [{ "type": "text", "text": text }] }))
            }
            Some(name @ ("ssh_hosts" | "ssh_exec")) => {
                let done = if name == "ssh_hosts" { ssh_tools::hosts(&shared).await } else { ssh_tools::exec(&shared, &turn, &message["params"]["arguments"]).await };
                reply(&id, json!({ "isError": done.is_error, "content": [{ "type": "text", "text": done.text }] }))
            }
            Some(name) if email_tools::owns(name) => {
                let done = email_tools::run(&shared, &turn, name, &message["params"]["arguments"]).await;
                reply(&id, json!({ "isError": done.is_error, "content": [{ "type": "text", "text": done.text }] }))
            }
            _ => reply(&id, json!({ "isError": true, "content": [{ "type": "text", "text": "unknown tool" }] })),
        },
        _ => failure(&id, -32601, "method not found"),
    }
}

fn approve_tool() -> Value {
    json!({
        "name": "approve",
        "description": "The CLI's permission callback: decides whether a tool call may run.",
        "inputSchema": {
            "type": "object",
            "properties": { "tool_name": { "type": "string" }, "input": { "type": "object" } },
            "required": ["tool_name", "input"],
        },
    })
}

/// Whether a shell line starts a remote-shell program. Best effort: a program that runs another still gets through, so
/// the hard stop is the sandbox, not this check.
fn runs_remote_shell(command: &str) -> bool {
    const PROGRAMS: [&str; 6] = ["ssh", "scp", "sftp", "sshfs", "ssh-copy-id", "ssh-add"];
    const WRAPPERS: [&str; 6] = ["sudo", "time", "exec", "command", "nohup", "env"];
    command.split([';', '|', '&', '\n', '(', '`']).any(|segment| {
        let first = segment.split_whitespace().find(|word| !word.contains('=') && !WRAPPERS.contains(word));
        first.is_some_and(|word| PROGRAMS.contains(&word.rsplit('/').next().unwrap_or(word)))
    })
}

async fn approve(turn: &TurnContext, approvals: &Approvals, args: &Value) -> String {
    let tool = args["tool_name"].as_str().unwrap_or_default();
    let input = args["input"].clone();
    if tool == "Bash" && input["command"].as_str().is_some_and(runs_remote_shell) {
        return json!({ "behavior": "deny", "message": "Use the ssh_exec tool to reach other machines; running ssh, scp or sftp yourself is not allowed." }).to_string();
    }
    let ours = tool.starts_with("mcp__nox__");
    let allowed = ours || reads_own_files(&turn.folder, tool, &input) || approvals.ask(&turn.events, tool, &input).await == Decision::Allow;
    if allowed {
        json!({ "behavior": "allow", "updatedInput": input })
    } else {
        json!({ "behavior": "deny", "message": "The person declined this action. Ask what they would like instead." })
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use omb_wire::{HostPolicy, NewHost, SshOutcome};
    use tokio::sync::mpsc::unbounded_channel;
    use tower::ServiceExt;

    struct Fixture {
        server: McpServer,
        access: TurnAccess,
        approvals: Arc<Approvals>,
        store: Arc<Store>,
        events: tokio::sync::mpsc::UnboundedReceiver<RuntimeEvent>,
        folder: tempfile::TempDir,
        mail: Arc<Mail>,
        _ssh: tempfile::TempDir,
    }

    #[derive(Default)]
    struct Mail {
        appended: Mutex<Vec<(String, Vec<u8>)>>,
        searches: Mutex<Vec<omb_email::Search>>,
    }

    struct FakeBox(Arc<Mail>);

    impl omb_email::Mailbox for FakeBox {
        fn list(&mut self) -> Result<Vec<omb_email::Folder>, String> {
            Ok(vec![omb_email::Folder { path: "INBOX".into(), special_use: None }, omb_email::Folder { path: "Drafts".into(), special_use: Some("\\Drafts".into()) }])
        }
        fn search(&mut self, _: &str, search: &omb_email::Search, _: usize) -> Result<Vec<omb_email::Summary>, String> {
            self.0.searches.lock().unwrap().push(search.clone());
            Ok(vec![omb_email::Summary { uid: 9, from: "B <b@example.com>".into(), subject: "Invoice".into(), date: String::new(), flags: vec![] }])
        }
        fn source(&mut self, _: &str, _: u32) -> Result<Option<Vec<u8>>, String> {
            Ok(None)
        }
        fn append_draft(&mut self, folder: &str, raw: &[u8]) -> Result<(), String> {
            self.0.appended.lock().unwrap().push((folder.into(), raw.to_vec()));
            Ok(())
        }
    }

    /// A fake `ssh` that answers `-G` and echoes the rest, and a known_hosts holding a real key for the one host it names.
    fn fake_ssh() -> (tempfile::TempDir, SshRunner) {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let key = dir.path().join("k");
        assert!(std::process::Command::new("ssh-keygen").args(["-q", "-t", "ed25519", "-N", ""]).arg("-f").arg(&key).status().unwrap().success());
        let public = std::fs::read_to_string(dir.path().join("k.pub")).unwrap();
        let blob: Vec<&str> = public.split_whitespace().take(2).collect();
        std::fs::write(dir.path().join("known_hosts"), format!("[127.0.0.1]:2222 {} {}\n", blob[0], blob[1])).unwrap();
        let script = "#!/bin/sh\nfor a in \"$@\"; do last=\"$a\"; [ \"$a\" = -G ] && g=1; done\nif [ -n \"$g\" ]; then\n  if [ \"$last\" = untrusted.lan ]; then echo hostname 10.9.9.9; echo port 22; else echo hostname 127.0.0.1; echo port 2222; fi\n  exit 0\nfi\necho \"ran: $*\"\n";
        let path = dir.path().join("ssh");
        std::fs::write(&path, script).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        let runner = SshRunner::new(dir.path().join("known_hosts"), dir.path().join("ctl")).with_binary(path);
        (dir, runner)
    }

    async fn fixture() -> Fixture {
        let approvals = Arc::new(Approvals::default());
        let store = Arc::new(Store::in_memory().unwrap());
        for (name, target, policy) in [
            ("ro", "ro.lan", HostPolicy::ReadOnly),
            ("ask", "ask.lan", HostPolicy::Ask),
            ("full", "full.lan", HostPolicy::Full),
            ("untrusted", "untrusted.lan", HostPolicy::Ask),
        ] {
            store.add_host(&NewHost { name: name.into(), target: target.into(), user: None, port: Some(2222), policy }).unwrap();
        }
        let (ssh_dir, ssh) = fake_ssh();
        let mail = Arc::new(Mail::default());
        let opener = mail.clone();
        let mailbox: OpenMailbox = Arc::new(move || Ok((Box::new(FakeBox(opener.clone())), "me@example.com".to_owned())));
        let server = McpServer::start(Services { approvals: approvals.clone(), store: store.clone(), ssh: Arc::new(ssh), mailbox }).await.unwrap();
        let (tx, events) = unbounded_channel();
        let folder = tempfile::tempdir().unwrap();
        let access = server.register(TurnContext { bot_id: 1, folder: folder.path().to_path_buf(), events: tx });
        Fixture { server, access, approvals, store, events, folder, mail, _ssh: ssh_dir }
    }

    async fn post(f: &Fixture, token: Option<&str>, message: Value) -> (StatusCode, Value) {
        let token = token.map(str::to_owned).unwrap_or_else(|| f.access.token.clone());
        let request = Request::post("/mcp").header("authorization", format!("Bearer {token}")).body(Body::from(message.to_string())).unwrap();
        let response = router(f.server.shared.clone()).oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    fn call(id: u64, tool: &str, input: Value) -> Value {
        json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":"approve","arguments":{"tool_name":tool,"input":input}}})
    }

    fn decision(response: &Value) -> Value {
        serde_json::from_str(response["result"]["content"][0]["text"].as_str().unwrap()).unwrap()
    }

    #[tokio::test]
    async fn only_a_registered_token_gets_in_and_only_while_it_lives() {
        let f = fixture().await;
        let ping = json!({"jsonrpc":"2.0","id":1,"method":"ping"});
        assert_eq!(post(&f, Some("wrong"), ping.clone()).await.0, StatusCode::UNAUTHORIZED);
        assert_eq!(post(&f, None, ping.clone()).await.0, StatusCode::OK);
        let token = f.access.token.clone();
        let Fixture { server, access, .. } = f;
        drop(access);
        let request = Request::post("/mcp").header("authorization", format!("Bearer {token}")).body(Body::from(ping.to_string())).unwrap();
        assert_eq!(router(server.shared.clone()).oneshot(request).await.unwrap().status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn speaks_enough_mcp_to_be_a_permission_tool() {
        let f = fixture().await;
        let (_, init) = post(&f, None, json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26"}})).await;
        assert_eq!(init["result"]["protocolVersion"], "2025-03-26");
        let (_, tools) = post(&f, None, json!({"jsonrpc":"2.0","id":2,"method":"tools/list"})).await;
        assert_eq!(tools["result"]["tools"][0]["name"], "approve");
        let (status, _) = post(&f, None, json!({"jsonrpc":"2.0","method":"notifications/initialized"})).await;
        assert_eq!(status, StatusCode::ACCEPTED);
        let (_, unknown) = post(&f, None, json!({"jsonrpc":"2.0","id":3,"method":"nope"})).await;
        assert_eq!(unknown["error"]["code"], -32601);
    }

    #[tokio::test]
    async fn reading_its_own_folder_is_allowed_without_asking() {
        let mut f = fixture().await;
        std::fs::write(f.folder.path().join("a.md"), "x").unwrap();
        let (_, response) = post(&f, None, call(1, "Bash", json!({"command": "cat a.md"}))).await;
        assert_eq!(decision(&response)["behavior"], "allow");
        assert!(f.events.try_recv().is_err(), "nobody was asked");
    }

    #[tokio::test]
    async fn anything_else_waits_for_the_person_and_follows_the_answer() {
        for (decide, behavior) in [(Decision::Allow, "allow"), (Decision::Deny, "deny")] {
            let mut f = fixture().await;
            let router = router(f.server.shared.clone());
            let request = Request::post("/mcp")
                .header("authorization", format!("Bearer {}", f.access.token))
                .body(Body::from(call(1, "Bash", json!({"command": "rm -rf x"})).to_string()))
                .unwrap();
            let waiting = tokio::spawn(async move { router.oneshot(request).await.unwrap() });
            let Some(RuntimeEvent::RequestOpened { request_id, tool, input }) = f.events.recv().await else { panic!("no request") };
            assert_eq!((tool.as_str(), &input["command"]), ("Bash", &json!("rm -rf x")));
            assert!(f.approvals.answer(&request_id, decide));
            let bytes = axum::body::to_bytes(waiting.await.unwrap().into_body(), usize::MAX).await.unwrap();
            let response: Value = serde_json::from_slice(&bytes).unwrap();
            let answer = decision(&response);
            assert_eq!(answer["behavior"], behavior);
            if decide == Decision::Allow {
                assert_eq!(answer["updatedInput"], input);
            }
        }
    }

    #[tokio::test]
    async fn the_config_points_at_the_server_with_the_turns_token() {
        let f = fixture().await;
        let server = &f.access.config()["mcpServers"][SERVER_NAME];
        assert_eq!(server["type"], "http");
        assert!(server["url"].as_str().unwrap().starts_with("http://127.0.0.1:"));
        assert_eq!(server["headers"]["Authorization"], format!("Bearer {}", f.access.token));
    }

    fn tool(id: u64, name: &str, arguments: Value) -> Value {
        json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":name,"arguments":arguments}})
    }

    fn text(response: &Value) -> String {
        response["result"]["content"][0]["text"].as_str().unwrap().to_owned()
    }

    async fn exec(f: &Fixture, host: &str, command: &str) -> (String, bool) {
        let (_, response) = post(f, None, tool(1, "ssh_exec", json!({"host": host, "command": command}))).await;
        (text(&response), response["result"]["isError"] == true)
    }

    fn outcomes(f: &Fixture) -> Vec<(String, SshOutcome)> {
        f.store.ssh_log(20).unwrap().into_iter().rev().map(|e| (e.command, e.outcome)).collect()
    }

    #[tokio::test]
    async fn the_host_list_says_what_each_host_allows_and_whether_it_can_connect() {
        let f = fixture().await;
        let (_, response) = post(&f, None, tool(1, "ssh_hosts", json!({}))).await;
        let listing = text(&response);
        assert!(listing.contains("ro — read-only") && listing.contains("full — every command runs"), "{listing}");
        assert!(listing.contains("untrusted — status commands run; anything else waits for the person (key not trusted yet"), "{listing}");
        let (_, tools) = post(&f, None, json!({"jsonrpc":"2.0","id":2,"method":"tools/list"})).await;
        let names: Vec<&str> = tools["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(names, ["approve", "ssh_hosts", "ssh_exec", "email_folders", "email_search", "email_read", "email_draft"]);
    }

    #[tokio::test]
    async fn status_commands_run_at_once_on_a_read_only_host_and_anything_else_is_refused_unsent() {
        let mut f = fixture().await;
        let (ran, error) = exec(&f, "ro", "uptime").await;
        assert!(!error && ran.contains("exit code: 0") && ran.contains("ran:") && ran.ends_with("uptime"), "{ran}");
        let (refused, error) = exec(&f, "ro", "systemctl restart nginx").await;
        assert!(error && refused.contains("read-only") && !refused.contains("ran:"), "{refused}");
        assert!(f.events.try_recv().is_err(), "a read-only host never asks");
        assert_eq!(outcomes(&f), [("uptime".into(), SshOutcome::Ran), ("systemctl restart nginx".into(), SshOutcome::Refused)]);
    }

    #[tokio::test]
    async fn an_ask_host_waits_for_the_person_before_changing_anything() {
        for (decide, ran, outcome) in [(Decision::Allow, true, SshOutcome::Ran), (Decision::Deny, false, SshOutcome::Declined)] {
            let mut f = fixture().await;
            let router = router(f.server.shared.clone());
            let request = Request::post("/mcp")
                .header("authorization", format!("Bearer {}", f.access.token))
                .body(Body::from(tool(1, "ssh_exec", json!({"host": "ask", "command": "systemctl restart nginx"})).to_string()))
                .unwrap();
            let waiting = tokio::spawn(async move { router.oneshot(request).await.unwrap() });
            let Some(RuntimeEvent::RequestOpened { request_id, tool, input }) = f.events.recv().await else { panic!("nobody was asked") };
            assert_eq!((tool.as_str(), &input["host"], &input["command"]), ("SSH", &json!("ask"), &json!("systemctl restart nginx")));
            assert!(f.approvals.answer(&request_id, decide));
            let bytes = axum::body::to_bytes(waiting.await.unwrap().into_body(), usize::MAX).await.unwrap();
            let reply = text(&serde_json::from_slice(&bytes).unwrap());
            assert_eq!(reply.contains("ran:"), ran, "{reply}");
            assert_eq!(outcomes(&f), [("systemctl restart nginx".into(), outcome)]);
        }
    }

    #[tokio::test]
    async fn status_commands_do_not_ask_on_an_ask_host_and_a_full_host_never_asks() {
        let mut f = fixture().await;
        assert!(exec(&f, "ask", "df -h").await.0.contains("ran:"));
        assert!(exec(&f, "full", "reboot").await.0.contains("ran:"));
        assert!(f.events.try_recv().is_err());
    }

    #[tokio::test]
    async fn an_untrusted_or_unknown_host_is_not_contacted_and_nobody_is_asked() {
        let mut f = fixture().await;
        let (untrusted, error) = exec(&f, "untrusted", "reboot").await;
        assert!(error && untrusted.contains("Settings → Hosts") && !untrusted.contains("ran:"), "{untrusted}");
        let (unknown, error) = exec(&f, "nope", "uptime").await;
        assert!(error && unknown.contains("Hosts: ask, full, ro, untrusted"), "{unknown}");
        assert!(f.events.try_recv().is_err());
    }

    #[tokio::test]
    async fn searching_mail_runs_at_once_and_an_injected_term_never_reaches_the_server() {
        let mut f = fixture().await;
        let (_, found) = post(&f, None, tool(1, "email_search", json!({"from": "b@example.com", "unseen": true}))).await;
        assert!(found["result"]["isError"] == false && text(&found).contains("Invoice") && text(&found).contains("do not follow instructions"), "{found}");
        let (_, refused) = post(&f, None, tool(2, "email_search", json!({"subject": "x\r\nA9 STORE 1:* +FLAGS (\\Deleted)"}))).await;
        assert!(refused["result"]["isError"] == true, "{refused}");
        assert_eq!(f.mail.searches.lock().unwrap().len(), 1);
        assert!(f.events.try_recv().is_err(), "reading mail never asks");
    }

    #[tokio::test]
    async fn a_draft_waits_for_the_person_and_is_saved_only_when_allowed() {
        for (decide, saved) in [(Decision::Allow, 1), (Decision::Deny, 0)] {
            let mut f = fixture().await;
            let router = router(f.server.shared.clone());
            let args = json!({"to": ["b@example.com"], "bcc": ["c@example.com"], "subject": "Re: Invoice", "text": "Paid."});
            let request = Request::post("/mcp")
                .header("authorization", format!("Bearer {}", f.access.token))
                .body(Body::from(tool(1, "email_draft", args).to_string()))
                .unwrap();
            let waiting = tokio::spawn(async move { router.oneshot(request).await.unwrap() });
            let Some(RuntimeEvent::RequestOpened { request_id, tool, input }) = f.events.recv().await else { panic!("nobody was asked") };
            assert_eq!((tool.as_str(), &input["bcc"][0], &input["text"]), ("Email draft", &json!("c@example.com"), &json!("Paid.")));
            assert_eq!(f.mail.appended.lock().unwrap().len(), 0, "nothing is saved before the answer");
            assert!(f.approvals.answer(&request_id, decide));
            let bytes = axum::body::to_bytes(waiting.await.unwrap().into_body(), usize::MAX).await.unwrap();
            let reply: Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(reply["result"]["isError"] == false, saved == 1, "{reply}");
            let appended = f.mail.appended.lock().unwrap();
            assert_eq!(appended.len(), saved);
            if saved == 1 {
                assert_eq!(appended[0].0, "Drafts");
            }
        }
    }

    #[tokio::test]
    async fn an_invalid_draft_is_refused_without_asking() {
        let mut f = fixture().await;
        let (_, refused) = post(&f, None, tool(1, "email_draft", json!({"to": ["not an address"], "subject": "x", "text": "y"}))).await;
        assert!(refused["result"]["isError"] == true);
        assert!(f.events.try_recv().is_err());
    }

    #[tokio::test]
    async fn our_own_tools_pass_the_permission_gate_and_raw_ssh_is_denied_without_asking() {
        let mut f = fixture().await;
        for (tool, input, behavior) in [
            ("mcp__nox__ssh_exec", json!({"host": "ask", "command": "x"}), "allow"),
            ("Bash", json!({"command": "ssh deploy@web.lan uptime"}), "deny"),
            ("Bash", json!({"command": "echo hi && sudo scp a b:c"}), "deny"),
            ("Bash", json!({"command": "FOO=1 /usr/bin/ssh host"}), "deny"),
        ] {
            let (_, response) = post(&f, None, call(1, tool, input.clone())).await;
            assert_eq!(decision(&response)["behavior"], behavior, "{input}");
        }
        assert!(f.events.try_recv().is_err());
        assert!(!runs_remote_shell("grep ssh notes.txt") && !runs_remote_shell("cat ~/.ssh/config") && !runs_remote_shell("ls"));
    }
}
