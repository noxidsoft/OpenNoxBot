//! Drives the real `claude` CLI against the permission server. Needs a logged-in `claude` and costs a few cents:
//! `cargo test -p omb-mcp --test real_claude -- --ignored --test-threads=1`
use omb_core::approvals::{Approvals, Decision};
use omb_drivers::{Host, McpAccess, Turn, run_turn};
use omb_mcp::{DISALLOWED_TOOLS, McpServer, PERMISSION_TOOL, Services, TurnContext};
use omb_wire::{ApprovalMode, RuntimeEvent};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc::unbounded_channel;

struct Outcome {
    events: Vec<RuntimeEvent>,
    folder: tempfile::TempDir,
}

async fn play(prompt: &str, decide: Decision) -> Outcome {
    let host = Host::from_process();
    let bin = host.resolve("claude").expect("claude is installed");
    let approvals = Arc::new(Approvals::default());
    let store = Arc::new(omb_core::store::Store::in_memory().unwrap());
    let ssh = Arc::new(omb_ssh::SshRunner::new(std::env::temp_dir().join("nox-test-known"), std::env::temp_dir().join("nox-test-ctl")));
    let server = McpServer::start(Services { approvals: approvals.clone(), store, ssh }).await.unwrap();
    let folder = tempfile::tempdir().unwrap();
    let (tx, mut rx) = unbounded_channel();
    let access = server.register(TurnContext { bot_id: 1, folder: folder.path().to_path_buf(), events: tx.clone() });
    let config = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(config.path(), access.config().to_string()).unwrap();

    let driver = {
        let (cwd, config, prompt) = (folder.path().to_path_buf(), config.path().to_path_buf(), prompt.to_owned());
        tokio::spawn(async move {
            let turn = Turn {
                prompt: &prompt,
                model: Some("haiku"),
                approval: ApprovalMode::Ask,
                resume: None,
                cwd: &cwd,
                mcp: Some(McpAccess { config: &config, permission_tool: PERMISSION_TOOL, disallowed_tools: &DISALLOWED_TOOLS }),
            };
            run_turn(&bin, &host, &turn, &tx).await
        })
    };

    let mut events = Vec::new();
    let finished = tokio::time::timeout(Duration::from_secs(180), async {
        while let Some(event) = rx.recv().await {
            if let RuntimeEvent::RequestOpened { request_id, .. } = &event {
                assert!(approvals.answer(request_id, decide));
            }
            let done = matches!(event, RuntimeEvent::TurnCompleted { .. } | RuntimeEvent::RuntimeError { .. });
            events.push(event);
            if done {
                break;
            }
        }
    })
    .await;
    assert!(finished.is_ok(), "the turn did not finish: {events:#?}");
    let _ = driver.await;
    Outcome { events, folder }
}

#[tokio::test]
#[ignore = "needs a logged-in claude and spends a few cents"]
async fn an_allowed_write_is_shown_first_and_then_happens() {
    let out = play("Create a file named allowed.txt in the current folder containing the word hi, then reply done.", Decision::Allow).await;
    let (id, tool) = out
        .events
        .iter()
        .find_map(|e| match e {
            RuntimeEvent::RequestOpened { request_id, tool, .. } => Some((request_id.clone(), tool.clone())),
            _ => None,
        })
        .unwrap_or_else(|| panic!("nothing was asked: {:#?}", out.events));
    assert!(["Write", "Bash", "Edit"].contains(&tool.as_str()), "{tool}");
    assert!(out.events.contains(&RuntimeEvent::RequestResolved { request_id: id, allowed: true }));
    let written = std::fs::read_to_string(out.folder.path().join("allowed.txt")).unwrap_or_else(|_| panic!("the file was not written: {:#?}", out.events));
    assert!(written.contains("hi"), "{written:?}");
}

#[tokio::test]
#[ignore = "needs a logged-in claude and spends a few cents"]
async fn a_denied_write_does_not_happen() {
    let out = play("Create a file named denied.txt in the current folder containing the word hi.", Decision::Deny).await;
    assert!(out.events.iter().any(|e| matches!(e, RuntimeEvent::RequestOpened { .. })), "{:#?}", out.events);
    assert!(out.events.iter().any(|e| matches!(e, RuntimeEvent::RequestResolved { allowed: false, .. })));
    assert!(!out.folder.path().join("denied.txt").exists(), "the denied file was written");
}
