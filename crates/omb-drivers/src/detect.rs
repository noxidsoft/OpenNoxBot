use crate::host::Host;
use crate::login;
use omb_wire::{AuthState, EngineId, EngineStatus};

pub async fn detect_all(host: &Host) -> Vec<EngineStatus> {
    let (claude, codex) = tokio::join!(cli_engine(EngineId::Claude, "claude", host), cli_engine(EngineId::Codex, "codex", host));
    vec![claude, codex, api_key_engine(EngineId::OpenaiApi, host.openai_api_key), api_key_engine(EngineId::Jev, host.jev_api_key)]
}

async fn cli_engine(id: EngineId, name: &str, host: &Host) -> EngineStatus {
    let Some(bin) = host.resolve(name) else {
        return EngineStatus { id, available: false, version: None, auth: AuthState::SignedOut, account: None };
    };
    let (version, (auth, account)) = tokio::join!(login::version(&bin, host), async {
        match id {
            EngineId::Claude => login::claude(&bin, host).await,
            _ => (login::codex(&bin, host).await, None),
        }
    });
    EngineStatus { id, available: true, version, auth, account }
}

fn api_key_engine(id: EngineId, has_key: bool) -> EngineStatus {
    let auth = if has_key { AuthState::SignedIn } else { AuthState::SignedOut };
    EngineStatus { id, available: true, version: None, auth, account: None }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{fake, host};
    use std::time::Duration;

    fn find(all: &[EngineStatus], id: EngineId) -> &EngineStatus {
        all.iter().find(|e| e.id == id).unwrap()
    }

    #[tokio::test]
    async fn missing_clis_are_unavailable() {
        let tmp = tempfile::tempdir().unwrap();
        let all = detect_all(&host(tmp.path())).await;
        let claude = find(&all, EngineId::Claude);
        assert!(!claude.available && claude.version.is_none() && claude.auth == AuthState::SignedOut);
    }

    #[tokio::test]
    async fn signed_in_claude_reports_version_and_account() {
        let tmp = tempfile::tempdir().unwrap();
        let host = host(tmp.path());
        fake(
            &tmp.path().join("bin"),
            "claude",
            r#"case "$1" in --version) echo "2.1.0 (Claude Code)";; auth) echo '{"loggedIn":true,"email":"a@b.co"}';; esac"#,
        );
        let all = detect_all(&host).await;
        let claude = find(&all, EngineId::Claude);
        assert_eq!(claude.version.as_deref(), Some("2.1.0 (Claude Code)"));
        assert_eq!((claude.auth, claude.account.as_deref()), (AuthState::SignedIn, Some("a@b.co")));
    }

    #[tokio::test]
    async fn a_hanging_auth_check_is_unknown_not_signed_out() {
        let tmp = tempfile::tempdir().unwrap();
        let mut host = host(tmp.path());
        host.timeout = Duration::from_millis(400);
        fake(&tmp.path().join("bin"), "claude", r#"[ "$1" = auth ] && /bin/sleep 30; echo 1.0"#);
        let all = detect_all(&host).await;
        assert_eq!(find(&all, EngineId::Claude).auth, AuthState::Unknown);
    }

    #[tokio::test]
    async fn codex_with_auth_json_is_signed_in_without_asking_the_cli() {
        let tmp = tempfile::tempdir().unwrap();
        let host = host(tmp.path());
        fake(&tmp.path().join("bin"), "codex", "echo 'Not logged in'; exit 1");
        std::fs::create_dir_all(tmp.path().join("home/.codex")).unwrap();
        std::fs::write(tmp.path().join("home/.codex/auth.json"), "{}").unwrap();
        assert_eq!(find(&detect_all(&host).await, EngineId::Codex).auth, AuthState::SignedIn);
    }

    #[tokio::test]
    async fn codex_login_status_decides_when_there_is_no_auth_json() {
        let tmp = tempfile::tempdir().unwrap();
        let host = host(tmp.path());
        let bin = tmp.path().join("bin");
        fake(&bin, "codex", "echo 'Not logged in'; exit 1");
        assert_eq!(find(&detect_all(&host).await, EngineId::Codex).auth, AuthState::SignedOut);
        fake(&bin, "codex", "echo 'Logged in using ChatGPT'");
        assert_eq!(find(&detect_all(&host).await, EngineId::Codex).auth, AuthState::SignedIn);
    }

    #[tokio::test]
    async fn api_engines_follow_their_keys() {
        let tmp = tempfile::tempdir().unwrap();
        let mut host = host(tmp.path());
        let all = detect_all(&host).await;
        assert_eq!(find(&all, EngineId::OpenaiApi).auth, AuthState::SignedOut);
        assert_eq!(find(&all, EngineId::Jev).auth, AuthState::SignedOut);
        host.openai_api_key = true;
        host.jev_api_key = true;
        let all = detect_all(&host).await;
        assert_eq!(find(&all, EngineId::OpenaiApi).auth, AuthState::SignedIn);
        assert_eq!(find(&all, EngineId::Jev).auth, AuthState::SignedIn);
    }
}
