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
use omb_wire::RuntimeEvent;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc::UnboundedSender;

pub const SERVER_NAME: &str = "nox";
pub const PERMISSION_TOOL: &str = "mcp__nox__approve";

pub struct TurnContext {
    pub bot_id: i64,
    pub folder: PathBuf,
    pub events: UnboundedSender<RuntimeEvent>,
}

struct Shared {
    turns: Mutex<HashMap<String, Arc<TurnContext>>>,
    approvals: Arc<Approvals>,
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
    pub async fn start(approvals: Arc<Approvals>) -> std::io::Result<Self> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let port = listener.local_addr()?.port();
        let shared = Arc::new(Shared { turns: Mutex::new(HashMap::new()), approvals });
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
        Some("tools/list") => reply(&id, json!({ "tools": [approve_tool()] })),
        Some("tools/call") => match message["params"]["name"].as_str() {
            Some("approve") => {
                let text = approve(&turn, &shared.approvals, &message["params"]["arguments"]).await;
                reply(&id, json!({ "content": [{ "type": "text", "text": text }] }))
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

async fn approve(turn: &TurnContext, approvals: &Approvals, args: &Value) -> String {
    let tool = args["tool_name"].as_str().unwrap_or_default();
    let input = args["input"].clone();
    let allowed = reads_own_files(&turn.folder, tool, &input) || approvals.ask(&turn.events, tool, &input).await == Decision::Allow;
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
    use tokio::sync::mpsc::unbounded_channel;
    use tower::ServiceExt;

    struct Fixture {
        server: McpServer,
        access: TurnAccess,
        approvals: Arc<Approvals>,
        events: tokio::sync::mpsc::UnboundedReceiver<RuntimeEvent>,
        folder: tempfile::TempDir,
    }

    async fn fixture() -> Fixture {
        let approvals = Arc::new(Approvals::default());
        let server = McpServer::start(approvals.clone()).await.unwrap();
        let (tx, events) = unbounded_channel();
        let folder = tempfile::tempdir().unwrap();
        let access = server.register(TurnContext { bot_id: 1, folder: folder.path().to_path_buf(), events: tx });
        Fixture { server, access, approvals, events, folder }
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
}
