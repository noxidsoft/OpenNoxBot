//! Runs the SSH runner against a throwaway `sshd` on a loopback port, with a temporary host key and client key.
use omb_wire::{HostPolicy, SshHost};
use omb_ssh::SshRunner;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;
use tokio::process::{Child, Command};

struct Server {
    _child: Child,
    port: u16,
}

async fn keygen(path: &Path) {
    let done = Command::new("ssh-keygen").args(["-q", "-t", "ed25519", "-N", ""]).arg("-f").arg(path).status().await.unwrap();
    assert!(done.success());
}

async fn start(root: &Path) -> Option<Server> {
    if !Path::new("/usr/bin/sshd").exists() {
        eprintln!("skipped: no /usr/bin/sshd");
        return None;
    }
    let (host_key, client_key) = (root.join("host_key"), root.join("client_key"));
    keygen(&host_key).await;
    keygen(&client_key).await;
    std::fs::copy(root.join("client_key.pub"), root.join("authorized_keys")).unwrap();
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let config = format!(
        "Port {port}\nListenAddress 127.0.0.1\nHostKey {}\nPidFile {}\nAuthorizedKeysFile {}\nPasswordAuthentication no\nKbdInteractiveAuthentication no\nPubkeyAuthentication yes\nUsePAM no\nStrictModes no\nLogLevel ERROR\n",
        host_key.display(),
        root.join("sshd.pid").display(),
        root.join("authorized_keys").display()
    );
    std::fs::write(root.join("sshd_config"), config).unwrap();
    let child = Command::new("/usr/bin/sshd").args(["-D", "-e", "-f"]).arg(root.join("sshd_config")).stdout(Stdio::null()).stderr(Stdio::null()).kill_on_drop(true).spawn().unwrap();
    for _ in 0..50 {
        if tokio::net::TcpStream::connect(("127.0.0.1", port)).await.is_ok() {
            return Some(Server { _child: child, port });
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("sshd did not start");
}

fn host(port: u16, target: &str) -> SshHost {
    SshHost { id: 1, name: "test".into(), target: target.into(), user: None, port: Some(port), policy: HostPolicy::Ask }
}

#[tokio::test]
async fn pins_the_servers_key_and_then_runs_commands_safely() {
    let root = tempfile::tempdir().unwrap();
    let Some(server) = start(root.path()).await else { return };
    let runner = SshRunner::new(root.path().join("known_hosts"), root.path().join("ctl")).with_extra_args(vec![
        "-F".into(),
        "/dev/null".into(),
        "-i".into(),
        root.path().join("client_key").into(),
        "-o".into(),
        "IdentitiesOnly=yes".into(),
    ]);
    let host = host(server.port, "127.0.0.1");
    let ten = Duration::from_secs(10);

    assert!(!runner.is_trusted(&host).await);
    let refused = runner.exec(&host, "echo hello", ten).await;
    assert_eq!(refused.exit_code, Some(255), "{}", refused.stderr);
    assert!(refused.stderr.contains("Host key verification failed") || refused.stderr.contains("No ED25519 host key is known"), "{}", refused.stderr);

    let keys = runner.scan(&host).await.unwrap();
    let real = Command::new("ssh-keygen").arg("-lf").arg(root.path().join("host_key.pub")).output().await.unwrap();
    let real = String::from_utf8_lossy(&real.stdout).split_whitespace().nth(1).unwrap().to_owned();
    assert!(keys.iter().any(|key| key.fingerprint.starts_with(&real)), "{keys:?} vs {real}");
    assert!(!runner.is_trusted(&host).await, "scanning pins nothing");

    runner.pin(&keys).unwrap();
    runner.pin(&keys).unwrap();
    assert!(runner.is_trusted(&host).await);
    let lines = std::fs::read_to_string(runner.known_hosts()).unwrap().lines().count();
    assert_eq!(lines, keys.len(), "pinning twice adds nothing");

    let ran = runner.exec(&host, "echo hello; echo oops >&2; exit 3", ten).await;
    assert_eq!((ran.exit_code, ran.stdout.trim(), ran.stderr.contains("oops")), (Some(3), "hello", true), "{}", ran.stderr);

    let marker = root.path().join("injected");
    let _ = runner.exec(&self::host(server.port, &format!("-oProxyCommand=touch {}", marker.display())), "true", ten).await;
    assert!(!marker.exists(), "a target is never read as an ssh option");

    let slow = runner.exec(&host, "sleep 30", Duration::from_secs(1)).await;
    assert!(slow.timed_out);
}
