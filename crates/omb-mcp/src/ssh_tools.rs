use crate::{Shared, TurnContext};
use omb_core::approvals::Decision;
use omb_ssh::{Gate, gate};
use omb_wire::{HostPolicy, SshOutcome};
use serde_json::{Value, json};
use std::time::{Duration, Instant};

pub struct Reply {
    pub text: String,
    pub is_error: bool,
}

fn ok(text: String) -> Reply {
    Reply { text, is_error: false }
}

fn error(text: impl Into<String>) -> Reply {
    Reply { text: text.into(), is_error: true }
}

pub fn definitions() -> Vec<Value> {
    vec![
        json!({
            "name": "ssh_hosts",
            "description": "Lists the servers you may run commands on, with what each one allows. Add no host yourself; the person does that in Settings.",
            "inputSchema": { "type": "object", "properties": {} },
        }),
        json!({
            "name": "ssh_exec",
            "description": "Runs one shell command on a listed server over SSH and returns its exit code and output. Status commands (uptime, df, systemctl status, journalctl, docker ps, tail of /var/log) run at once; other commands may be refused or may wait for the person to approve them, depending on the host. Use this instead of running ssh yourself.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "host": { "type": "string", "description": "A name from ssh_hosts" },
                    "command": { "type": "string" },
                    "timeout_seconds": { "type": "integer", "minimum": 1, "maximum": 300 },
                },
                "required": ["host", "command"],
            },
        }),
    ]
}

fn describe(policy: HostPolicy) -> &'static str {
    match policy {
        HostPolicy::ReadOnly => "read-only: only status commands run",
        HostPolicy::Ask => "status commands run; anything else waits for the person",
        HostPolicy::Full => "every command runs",
    }
}

pub async fn hosts(shared: &Shared) -> Reply {
    let hosts = match shared.store.hosts() {
        Ok(hosts) => hosts,
        Err(message) => return error(message),
    };
    if hosts.is_empty() {
        return ok("No hosts have been added. The person adds them in Settings → Hosts.".into());
    }
    let mut lines = Vec::new();
    for host in hosts {
        let key = if shared.ssh.is_trusted(&host).await { "ready" } else { "key not trusted yet, cannot connect" };
        lines.push(format!("{} — {} ({key})", host.name, describe(host.policy)));
    }
    ok(lines.join("\n"))
}

pub async fn exec(shared: &Shared, turn: &TurnContext, args: &Value) -> Reply {
    let (name, command) = (args["host"].as_str().unwrap_or_default(), args["command"].as_str().unwrap_or_default());
    let timeout = args["timeout_seconds"].as_u64().unwrap_or(30).clamp(1, 300);
    let host = match shared.store.host_by_name(name) {
        Ok(Some(host)) => host,
        Ok(None) => {
            let known = shared.store.hosts().unwrap_or_default().iter().map(|h| h.name.clone()).collect::<Vec<_>>().join(", ");
            return error(format!("There is no host named \"{name}\". Hosts: {}", if known.is_empty() { "none".into() } else { known }));
        }
        Err(message) => return error(message),
    };
    let log = |outcome, code, millis| {
        let _ = shared.store.log_ssh(Some(turn.bot_id), &host.name, command, outcome, code, millis);
    };

    let needs_person = match gate(host.policy, command) {
        Gate::Refuse(why) => {
            log(SshOutcome::Refused, None, None);
            return error(why);
        }
        Gate::Ask => true,
        Gate::Run => false,
    };
    if !shared.ssh.is_trusted(&host).await {
        log(SshOutcome::Failed, None, None);
        return error("This host's key has not been trusted yet. Ask the person to trust it in Settings → Hosts.");
    }
    if needs_person && shared.approvals.ask(&turn.events, "SSH", &json!({ "host": host.name, "command": command })).await != Decision::Allow {
        log(SshOutcome::Declined, None, None);
        return error("The person declined to run this command. Ask what they would like instead.");
    }

    let started = Instant::now();
    let done = shared.ssh.exec(&host, command, Duration::from_secs(timeout)).await;
    let millis = Some(started.elapsed().as_millis() as i64);
    if let Some(message) = done.error {
        log(SshOutcome::Failed, None, millis);
        return error(message);
    }
    if done.timed_out {
        log(SshOutcome::Failed, None, millis);
        return error(format!("The command did not finish within {timeout} seconds and was stopped."));
    }
    // ssh itself exits 255 when it cannot connect or the key does not match.
    let connection_failed = done.exit_code == Some(255);
    log(if connection_failed { SshOutcome::Failed } else { SshOutcome::Ran }, done.exit_code, millis);
    let mut text = format!("exit code: {}", done.exit_code.map_or("none".into(), |code| code.to_string()));
    if !done.stdout.is_empty() {
        text.push_str(&format!("\n--- stdout ---\n{}", done.stdout.trim_end()));
    }
    if !done.stderr.is_empty() {
        text.push_str(&format!("\n--- stderr ---\n{}", done.stderr.trim_end()));
    }
    if done.truncated {
        text.push_str("\n(the output was cut and the command stopped)");
    }
    Reply { text, is_error: connection_failed }
}
