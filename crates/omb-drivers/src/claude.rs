use crate::host::Host;
use omb_core::proc::spawn;
use omb_wire::{ApprovalMode, RuntimeEvent};
use serde_json::{Value, json};
use std::path::Path;
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc::UnboundedSender;

const STDERR_TAIL: usize = 2048;

/// Only this config's servers are loaded, and `permission_tool` answers the CLI's permission prompts.
pub struct McpAccess<'a> {
    pub config: &'a Path,
    pub permission_tool: &'a str,
    pub disallowed_tools: &'a [&'a str],
}

pub struct Turn<'a> {
    pub prompt: &'a str,
    pub model: Option<&'a str>,
    pub approval: ApprovalMode,
    pub resume: Option<&'a str>,
    pub cwd: &'a Path,
    pub mcp: Option<McpAccess<'a>>,
}

fn permission_mode(approval: ApprovalMode) -> &'static str {
    match approval {
        ApprovalMode::Ask | ApprovalMode::Custom => "default",
        ApprovalMode::Edits => "acceptEdits",
        ApprovalMode::Auto => "auto",
        ApprovalMode::Full => "bypassPermissions",
    }
}

/// Runs one turn on a fresh `claude` process and returns the session id the CLI reported, for `--resume` on the next turn.
pub async fn run_turn(bin: &Path, host: &Host, turn: &Turn<'_>, events: &UnboundedSender<RuntimeEvent>) -> Option<String> {
    let fail = |message: String| {
        let _ = events.send(RuntimeEvent::RuntimeError { message });
        None
    };

    let mut cmd = Command::new(bin);
    cmd.args(["-p", "--output-format", "stream-json", "--input-format", "stream-json", "--verbose", "--include-partial-messages"])
        .args(["--permission-mode", permission_mode(turn.approval)]);
    if let Some(model) = turn.model {
        cmd.args(["--model", model]);
    }
    if let Some(session) = turn.resume {
        cmd.args(["--resume", session]);
    }
    if let Some(mcp) = &turn.mcp {
        cmd.arg("--mcp-config").arg(mcp.config).args(["--strict-mcp-config", "--permission-prompt-tool", mcp.permission_tool]);
        if !mcp.disallowed_tools.is_empty() {
            cmd.arg("--disallowedTools").args(mcp.disallowed_tools);
        }
    }
    cmd.current_dir(turn.cwd)
        .env("PATH", &host.path)
        .env("HOME", &host.home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let mut child = match spawn(&mut cmd).await {
        Ok(child) => child,
        Err(error) => return fail(format!("could not start claude: {error}")),
    };
    let (mut stdin, stdout, mut stderr) = (child.stdin.take()?, child.stdout.take()?, child.stderr.take()?);

    let stderr_tail = tokio::spawn(async move {
        let mut text = String::new();
        let _ = (&mut stderr).take(STDERR_TAIL as u64).read_to_string(&mut text).await;
        let _ = tokio::io::copy(&mut stderr, &mut tokio::io::sink()).await;
        text
    });

    let prompt = json!({ "type": "user", "message": { "role": "user", "content": turn.prompt } });
    if stdin.write_all(format!("{prompt}\n").as_bytes()).await.is_err() {
        return fail("claude closed its input before the prompt was sent".into());
    }
    drop(stdin);

    let mut session = None;
    let mut completed = false;
    let mut state = Parse::default();
    let mut lines = BufReader::new(stdout).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        for event in parse_frame(&line, &mut state) {
            match &event {
                RuntimeEvent::TurnStarted { session_id, .. } if !session_id.is_empty() => session = Some(session_id.clone()),
                RuntimeEvent::TurnCompleted { .. } => completed = true,
                _ => {}
            }
            let _ = events.send(event);
        }
    }
    let _ = child.wait().await;

    if !completed {
        let said = stderr_tail.await.unwrap_or_default();
        let said = said.trim();
        let _ = events.send(RuntimeEvent::RuntimeError {
            message: if said.is_empty() { "claude exited before the turn finished".into() } else { said.to_owned() },
        });
    }
    session
}

#[derive(Default)]
struct Parse {
    streamed: bool,
}

fn parse_frame(line: &str, state: &mut Parse) -> Vec<RuntimeEvent> {
    let Ok(frame) = serde_json::from_str::<Value>(line) else { return Vec::new() };
    if !frame["parent_tool_use_id"].is_null() {
        return Vec::new();
    }
    let text = |value: &Value| value.as_str().unwrap_or_default().to_owned();
    match frame["type"].as_str() {
        Some("system") if frame["subtype"] == "init" => vec![RuntimeEvent::TurnStarted {
            session_id: text(&frame["session_id"]),
            model: frame["model"].as_str().map(str::to_owned),
        }],
        Some("stream_event") => {
            let delta = &frame["event"]["delta"];
            match (frame["event"]["type"].as_str(), delta["type"].as_str(), delta["text"].as_str()) {
                (Some("content_block_delta"), Some("text_delta"), Some(chunk)) => {
                    state.streamed = true;
                    vec![RuntimeEvent::ContentDelta { text: chunk.to_owned() }]
                }
                _ => Vec::new(),
            }
        }
        Some("assistant") => {
            let streamed = std::mem::take(&mut state.streamed);
            blocks(&frame)
                .filter_map(|block| match block["type"].as_str() {
                    Some("text") if !streamed => Some(RuntimeEvent::ContentDelta { text: text(&block["text"]) }),
                    Some("tool_use") => Some(RuntimeEvent::ItemStarted { id: text(&block["id"]), name: text(&block["name"]), input: block["input"].clone() }),
                    _ => None,
                })
                .collect()
        }
        Some("user") => blocks(&frame)
            .filter(|block| block["type"] == "tool_result")
            .map(|block| RuntimeEvent::ItemCompleted {
                id: text(&block["tool_use_id"]),
                ok: block["is_error"] != true,
                output: tool_output(&block["content"]),
            })
            .collect(),
        Some("result") => vec![RuntimeEvent::TurnCompleted {
            is_error: frame["is_error"] == true,
            result: frame["result"].as_str().map(str::to_owned),
            cost_usd: frame["total_cost_usd"].as_f64(),
        }],
        _ => Vec::new(),
    }
}

fn blocks(frame: &Value) -> impl Iterator<Item = &Value> {
    frame["message"]["content"].as_array().into_iter().flatten()
}

fn tool_output(content: &Value) -> String {
    match content {
        Value::String(text) => text.clone(),
        Value::Array(parts) => parts.iter().filter_map(|part| part["text"].as_str()).collect::<Vec<_>>().join("\n"),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{fake, host};
    use tokio::sync::mpsc::unbounded_channel;

    const INIT: &str = r#"{"type":"system","subtype":"init","session_id":"s1","model":"m1"}"#;
    const DELTA_HEL: &str = r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"Hel"}}}"#;
    const DELTA_LO: &str = r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"lo"}}}"#;
    const ASSISTANT_HELLO: &str = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Hello"}]}}"#;
    const RESULT: &str = r#"{"type":"result","is_error":false,"result":"Hello","total_cost_usd":0.01}"#;

    async fn play(frames: &[&str], approval: ApprovalMode, model: Option<&str>, resume: Option<&str>) -> (Option<String>, Vec<RuntimeEvent>, std::path::PathBuf) {
        play_with(frames, approval, model, resume, None).await
    }

    async fn play_with(
        frames: &[&str],
        approval: ApprovalMode,
        model: Option<&str>,
        resume: Option<&str>,
        mcp_config: Option<&Path>,
    ) -> (Option<String>, Vec<RuntimeEvent>, std::path::PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.keep();
        let host = host(&root);
        let body = format!("/bin/cat > '{r}/stdin'\nprintf '%s\\n' \"$@\" > '{r}/args'\n/bin/cat <<'EOF'\n{}\nEOF", frames.join("\n"), r = root.display());
        fake(&root.join("bin"), "claude", &body);
        let (tx, mut rx) = unbounded_channel();
        let mcp = mcp_config.map(|config| McpAccess { config, permission_tool: "mcp__nox__approve", disallowed_tools: &["Bash(ssh:*)"] });
        let turn = Turn { prompt: "hi there", model, approval, resume, cwd: &root, mcp };
        let session = run_turn(&root.join("bin/claude"), &host, &turn, &tx).await;
        drop(tx);
        let mut events = Vec::new();
        while let Some(event) = rx.recv().await {
            events.push(event);
        }
        (session, events, root)
    }

    #[tokio::test]
    async fn streamed_text_is_not_repeated_by_the_assistant_message() {
        let (session, events, _) = play(&[INIT, DELTA_HEL, DELTA_LO, ASSISTANT_HELLO, RESULT], ApprovalMode::Ask, None, None).await;
        assert_eq!(session.as_deref(), Some("s1"));
        assert_eq!(
            events,
            vec![
                RuntimeEvent::TurnStarted { session_id: "s1".into(), model: Some("m1".into()) },
                RuntimeEvent::ContentDelta { text: "Hel".into() },
                RuntimeEvent::ContentDelta { text: "lo".into() },
                RuntimeEvent::TurnCompleted { is_error: false, result: Some("Hello".into()), cost_usd: Some(0.01) },
            ]
        );
    }

    #[tokio::test]
    async fn assistant_text_is_used_when_nothing_streamed() {
        let (_, events, _) = play(&[INIT, ASSISTANT_HELLO, RESULT], ApprovalMode::Ask, None, None).await;
        assert!(events.contains(&RuntimeEvent::ContentDelta { text: "Hello".into() }));
    }

    #[tokio::test]
    async fn tool_calls_start_and_complete_by_id() {
        let call = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"pwd"}}]}}"#;
        let done = r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","is_error":true,"content":[{"type":"text","text":"denied"}]}]}}"#;
        let (_, events, _) = play(&[INIT, call, done, RESULT], ApprovalMode::Ask, None, None).await;
        assert!(events.contains(&RuntimeEvent::ItemStarted { id: "t1".into(), name: "Bash".into(), input: json!({"command":"pwd"}) }));
        assert!(events.contains(&RuntimeEvent::ItemCompleted { id: "t1".into(), ok: false, output: "denied".into() }));
    }

    #[tokio::test]
    async fn subagent_frames_and_garbage_lines_are_ignored() {
        let sub = r#"{"type":"assistant","parent_tool_use_id":"t9","message":{"content":[{"type":"text","text":"inner"}]}}"#;
        let (_, events, _) = play(&[INIT, "not json", sub, RESULT], ApprovalMode::Ask, None, None).await;
        assert_eq!(events.len(), 2);
    }

    #[tokio::test]
    async fn exiting_without_a_result_is_a_runtime_error() {
        let (_, events, _) = play(&[INIT], ApprovalMode::Ask, None, None).await;
        assert_eq!(events.last(), Some(&RuntimeEvent::RuntimeError { message: "claude exited before the turn finished".into() }));
    }

    #[tokio::test]
    async fn a_missing_binary_is_a_runtime_error() {
        let tmp = tempfile::tempdir().unwrap();
        let (tx, mut rx) = unbounded_channel();
        let turn = Turn { prompt: "x", model: None, approval: ApprovalMode::Ask, resume: None, cwd: tmp.path(), mcp: None };
        assert_eq!(run_turn(&tmp.path().join("nope"), &host(tmp.path()), &turn, &tx).await, None);
        assert!(matches!(rx.recv().await, Some(RuntimeEvent::RuntimeError { .. })));
    }

    #[tokio::test]
    async fn the_prompt_and_flags_reach_the_cli() {
        let (_, _, root) = play(&[INIT, RESULT], ApprovalMode::Edits, Some("sonnet"), Some("abc")).await;
        let stdin: Value = serde_json::from_str(std::fs::read_to_string(root.join("stdin")).unwrap().trim()).unwrap();
        assert_eq!(stdin, json!({"type":"user","message":{"role":"user","content":"hi there"}}));
        let args = std::fs::read_to_string(root.join("args")).unwrap();
        let args: Vec<&str> = args.lines().collect();
        for pair in [["--permission-mode", "acceptEdits"], ["--model", "sonnet"], ["--resume", "abc"]] {
            assert!(args.windows(2).any(|w| w == pair), "{pair:?} missing from {args:?}");
        }
    }

    #[tokio::test]
    async fn the_mcp_config_and_permission_tool_reach_the_cli() {
        let config = std::path::PathBuf::from("/run/turn.json");
        let (_, _, root) = play_with(&[INIT, RESULT], ApprovalMode::Ask, None, None, Some(&config)).await;
        let args = std::fs::read_to_string(root.join("args")).unwrap();
        let args: Vec<&str> = args.lines().collect();
        for pair in [["--mcp-config", "/run/turn.json"], ["--permission-prompt-tool", "mcp__nox__approve"]] {
            assert!(args.windows(2).any(|w| w == pair), "{pair:?} missing from {args:?}");
        }
        assert!(args.contains(&"--strict-mcp-config"));
        assert!(args.windows(2).any(|w| w == ["--disallowedTools", "Bash(ssh:*)"]), "{args:?}");
    }

    #[test]
    fn approval_levels_map_to_claude_permission_modes() {
        assert_eq!(
            [ApprovalMode::Ask, ApprovalMode::Edits, ApprovalMode::Auto, ApprovalMode::Full].map(permission_mode),
            ["default", "acceptEdits", "auto", "bypassPermissions"]
        );
    }
}
