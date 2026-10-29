use crate::host::Host;
use crate::run::{Finished, RunError, run};
use omb_wire::AuthState;
use std::path::{Path, PathBuf};
use tokio::process::Command;

/// Only PATH and HOME reach a probed binary, never inherited credentials.
pub async fn version(bin: &Path, host: &Host) -> Option<String> {
    let mut cmd = Command::new(bin);
    cmd.arg("--version").env_clear().env("PATH", &host.path).env("HOME", &host.home);
    let done = run(cmd, host.timeout).await.ok()?;
    let line = done.stdout.lines().next()?.trim();
    (done.success && !line.is_empty()).then(|| line.to_owned())
}

fn login_command(bin: &Path, args: &[&str], host: &Host) -> Command {
    let mut cmd = Command::new(bin);
    cmd.args(args).env("PATH", &host.path).env("HOME", &host.home);
    match &host.codex_home {
        Some(dir) => cmd.env("CODEX_HOME", dir),
        None => cmd.env_remove("CODEX_HOME"),
    };
    cmd
}

/// `claude auth status` is authoritative: credential storage varies by platform and a file's presence accepts stale logins.
/// A check that times out says nothing, so it is `Unknown`.
pub async fn claude(bin: &Path, host: &Host) -> (AuthState, Option<String>) {
    match run(login_command(bin, &["auth", "status", "--json"], host), host.timeout).await {
        Ok(done) => parse_claude_status(&done.stdout),
        Err(RunError::TimedOut | RunError::Spawn) => (AuthState::Unknown, None),
    }
}

fn parse_claude_status(stdout: &str) -> (AuthState, Option<String>) {
    let Ok(status) = serde_json::from_str::<serde_json::Value>(stdout) else {
        return (AuthState::SignedOut, None);
    };
    if status.get("loggedIn") != Some(&serde_json::Value::Bool(true)) {
        return (AuthState::SignedOut, None);
    }
    let email = status.get("email").and_then(|e| e.as_str()).map(str::trim).filter(|e| valid_email(e));
    (AuthState::SignedIn, email.map(str::to_owned))
}

fn valid_email(email: &str) -> bool {
    email.len() <= 254
        && !email.chars().any(|c| c.is_control() || c.is_whitespace())
        && email.split_once('@').is_some_and(|(local, domain)| {
            !local.is_empty() && !domain.contains('@') && domain.contains('.') && !domain.starts_with('.') && !domain.ends_with('.')
        })
}

/// Codex keeps its login in `<CODEX_HOME>/auth.json` unless config.toml moves it to the keyring (codex-cli 0.160.1).
pub async fn codex(bin: &Path, host: &Host) -> AuthState {
    let auth = codex_home(host).map(|home| home.join("auth.json"));
    if auth.as_deref().is_some_and(readable_json) {
        return AuthState::SignedIn;
    }
    let done = run(login_command(bin, &["login", "status"], host), host.timeout).await;
    match done.as_ref().map(codex_status) {
        Ok(AuthState::SignedOut) if auth.as_deref().is_some_and(readable_json) => AuthState::SignedIn,
        Ok(state) => state,
        Err(_) => AuthState::Unknown,
    }
}

/// codex-cli 0.160.1 says "Logged in using …", "Not logged in", or "Error checking login status: …" for an auth.json it cannot read.
fn codex_status(done: &Finished) -> AuthState {
    let said = format!("{}\n{}", done.stdout, done.stderr);
    if done.success && says(&said, "logged in") {
        AuthState::SignedIn
    } else if !done.success && (says(&said, "not logged in") || says(&said, "error checking login status")) {
        AuthState::SignedOut
    } else {
        AuthState::Unknown
    }
}

fn says(text: &str, phrase: &str) -> bool {
    text.lines().any(|line| {
        line.trim_start()
            .to_lowercase()
            .strip_prefix(phrase)
            .is_some_and(|rest| rest.chars().next().is_none_or(|c| !c.is_alphanumeric() && c != '_'))
    })
}

fn codex_home(host: &Host) -> Option<PathBuf> {
    let dir = match &host.codex_home {
        Some(dir) => PathBuf::from(dir),
        None => host.home.join(".codex"),
    };
    dir.is_absolute().then_some(dir)
}

fn readable_json(path: &Path) -> bool {
    std::fs::read_to_string(path).is_ok_and(|text| serde_json::from_str::<serde_json::Value>(&text).is_ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_status_requires_logged_in_true() {
        assert_eq!(parse_claude_status(r#"{"loggedIn":true,"email":"a@b.co"}"#), (AuthState::SignedIn, Some("a@b.co".into())));
        assert_eq!(parse_claude_status(r#"{"loggedIn":false}"#), (AuthState::SignedOut, None));
        assert_eq!(parse_claude_status("not json"), (AuthState::SignedOut, None));
    }

    #[test]
    fn claude_email_is_dropped_when_malformed() {
        assert_eq!(parse_claude_status(r#"{"loggedIn":true,"email":"a b@c.co"}"#), (AuthState::SignedIn, None));
        assert_eq!(parse_claude_status(r#"{"loggedIn":true,"email":"x\u0007@c.co"}"#), (AuthState::SignedIn, None));
    }

    #[test]
    fn codex_phrases_match_at_line_start_only() {
        assert!(says("Logged in using ChatGPT", "logged in"));
        assert!(!says("Not logged in", "logged in"));
        assert!(!says("loggedinfoo", "logged in"));
        assert!(says("warn\nNot logged in\n", "not logged in"));
    }
}
