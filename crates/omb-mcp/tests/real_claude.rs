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
    let store = Arc::new(omb_core::store::Store::in_memory().unwrap());
    let ssh = Arc::new(omb_ssh::SshRunner::new(std::env::temp_dir().join("nox-test-known"), std::env::temp_dir().join("nox-test-ctl")));
    play_with(prompt, decide, ApprovalMode::Ask, store, ssh).await
}

async fn play_with(prompt: &str, decide: Decision, approval: ApprovalMode, store: Arc<omb_core::store::Store>, ssh: Arc<omb_ssh::SshRunner>) -> Outcome {
    let host = Host::from_process();
    let bin = host.resolve("claude").expect("claude is installed");
    let approvals = Arc::new(Approvals::default());
    let server = McpServer::start(Services { approvals: approvals.clone(), store, ssh, mailbox: Arc::new(|| Err("Email is not set up".into())) }).await.unwrap();
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
                approval,
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

struct Lab {
    _sshd: tokio::process::Child,
    root: tempfile::TempDir,
    store: Arc<omb_core::store::Store>,
    ssh: Arc<omb_ssh::SshRunner>,
}

/// A throwaway `sshd` on loopback, with its key already pinned, registered as the host "lab" in `Ask` mode.
async fn lab() -> Option<Lab> {
    use std::process::Stdio;
    use tokio::process::Command;
    if !std::path::Path::new("/usr/bin/sshd").exists() {
        eprintln!("skipped: no /usr/bin/sshd");
        return None;
    }
    let root = tempfile::tempdir().unwrap();
    for name in ["host_key", "client_key"] {
        let made = Command::new("ssh-keygen").args(["-q", "-t", "ed25519", "-N", ""]).arg("-f").arg(root.path().join(name)).status().await.unwrap();
        assert!(made.success());
    }
    std::fs::copy(root.path().join("client_key.pub"), root.path().join("authorized_keys")).unwrap();
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let config = format!(
        "Port {port}\nListenAddress 127.0.0.1\nHostKey {}\nPidFile {}\nAuthorizedKeysFile {}\nPasswordAuthentication no\nKbdInteractiveAuthentication no\nPubkeyAuthentication yes\nUsePAM no\nStrictModes no\nLogLevel ERROR\n",
        root.path().join("host_key").display(),
        root.path().join("sshd.pid").display(),
        root.path().join("authorized_keys").display()
    );
    std::fs::write(root.path().join("sshd_config"), config).unwrap();
    let sshd = Command::new("/usr/bin/sshd").args(["-D", "-e", "-f"]).arg(root.path().join("sshd_config")).stdout(Stdio::null()).stderr(Stdio::null()).kill_on_drop(true).spawn().unwrap();
    for _ in 0..50 {
        if tokio::net::TcpStream::connect(("127.0.0.1", port)).await.is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let ssh = omb_ssh::SshRunner::new(root.path().join("known_hosts"), root.path().join("ctl")).with_extra_args(vec![
        "-F".into(),
        "/dev/null".into(),
        "-i".into(),
        root.path().join("client_key").into(),
        "-o".into(),
        "IdentitiesOnly=yes".into(),
    ]);
    let store = Arc::new(omb_core::store::Store::in_memory().unwrap());
    let host = store
        .add_host(&omb_wire::NewHost { name: "lab".into(), target: "127.0.0.1".into(), user: None, port: Some(port), policy: omb_wire::HostPolicy::Ask })
        .unwrap();
    ssh.pin(&ssh.scan(&host).await.unwrap()).unwrap();
    Some(Lab { _sshd: sshd, root, store, ssh: Arc::new(ssh) })
}

#[tokio::test]
#[ignore = "needs a logged-in claude and spends a few cents"]
async fn the_agent_reaches_a_server_through_ssh_exec_and_changes_wait_for_the_person() {
    let Some(lab) = lab().await else { return };
    let marker = lab.root.path().join("changed.txt");
    let prompt = format!("Use the ssh_exec tool on the host named lab. First run `uptime`. Then run `touch {}`. Then reply done.", marker.display());
    let out = play_with(&prompt, Decision::Allow, ApprovalMode::Ask, lab.store.clone(), lab.ssh.clone()).await;

    let asked: Vec<&RuntimeEvent> = out.events.iter().filter(|e| matches!(e, RuntimeEvent::RequestOpened { tool, .. } if tool == "SSH")).collect();
    assert_eq!(asked.len(), 1, "only the change should ask: {:#?}", out.events);
    assert!(matches!(asked[0], RuntimeEvent::RequestOpened { input, .. } if input["host"] == "lab" && input["command"].as_str().unwrap().starts_with("touch ")));
    assert!(marker.exists(), "the approved command ran on the server: {:#?}", out.events);
    let log = lab.store.ssh_log(10).unwrap();
    assert_eq!(log.iter().filter(|e| e.outcome == omb_wire::SshOutcome::Ran).count(), 2, "{log:#?}");
    let host = lab.store.host_by_name("lab").unwrap().unwrap();
    lab.ssh.disconnect(&host).await;
}

#[tokio::test]
#[ignore = "needs a logged-in claude and spends a few cents"]
async fn a_declined_change_never_reaches_the_server() {
    let Some(lab) = lab().await else { return };
    let marker = lab.root.path().join("declined.txt");
    let prompt = format!("Use the ssh_exec tool on the host named lab to run `touch {}`, then reply done.", marker.display());
    let out = play_with(&prompt, Decision::Deny, ApprovalMode::Ask, lab.store.clone(), lab.ssh.clone()).await;
    assert!(out.events.iter().any(|e| matches!(e, RuntimeEvent::RequestResolved { allowed: false, .. })), "{:#?}", out.events);
    assert!(!marker.exists());
    assert_eq!(lab.store.ssh_log(10).unwrap()[0].outcome, omb_wire::SshOutcome::Declined);
}

#[tokio::test]
#[ignore = "needs a logged-in claude and spends a few cents"]
async fn raw_ssh_is_refused_even_with_full_access() {
    let out = {
        let store = Arc::new(omb_core::store::Store::in_memory().unwrap());
        let ssh = Arc::new(omb_ssh::SshRunner::new(std::env::temp_dir().join("nox-test-known"), std::env::temp_dir().join("nox-test-ctl")));
        play_with("Run the shell command `ssh -V` and tell me exactly what it printed.", Decision::Allow, ApprovalMode::Full, store, ssh).await
    };
    let attempt = out.events.iter().find_map(|e| match e {
        RuntimeEvent::ItemStarted { id, name, input } if name == "Bash" && input["command"].as_str().is_some_and(|c| c.contains("ssh")) => Some(id.clone()),
        _ => None,
    });
    let attempt = attempt.unwrap_or_else(|| panic!("the agent never tried ssh, so nothing was tested: {:#?}", out.events));
    let blocked = out.events.iter().any(|e| matches!(e, RuntimeEvent::ItemCompleted { id, ok: false, .. } if *id == attempt));
    assert!(blocked, "ssh was not blocked under Full access: {:#?}", out.events);
    assert!(!out.events.iter().any(|e| matches!(e, RuntimeEvent::RequestOpened { .. })), "the CLI refuses it before any request");
}
