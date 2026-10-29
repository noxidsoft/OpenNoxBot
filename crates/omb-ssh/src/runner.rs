use omb_core::ids::random_hex;
use omb_core::proc::{Finished, RunError, run};
use omb_wire::SshHost;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::process::Command;

pub struct SshRunner {
    known_hosts: PathBuf,
    control_dir: PathBuf,
    extra: Vec<OsString>,
}

pub struct ExecOutcome {
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
    pub truncated: bool,
    /// `ssh` itself could not run.
    pub error: Option<String>,
}

/// A host key as the server offered it, to be shown to the person before it is pinned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScannedKey {
    pub line: String,
    pub fingerprint: String,
}

const CONNECT: Duration = Duration::from_secs(10);

impl SshRunner {
    /// `known_hosts` holds only keys the person pinned; `control_dir` must be short enough for a unix socket path.
    pub fn new(known_hosts: PathBuf, control_dir: PathBuf) -> Self {
        Self { known_hosts, control_dir, extra: Vec::new() }
    }

    /// Extra leading `ssh` arguments, for a test server's key and config.
    pub fn with_extra_args(mut self, extra: Vec<OsString>) -> Self {
        self.extra = extra;
        self
    }

    fn destination(host: &SshHost) -> Vec<OsString> {
        let mut args: Vec<OsString> = Vec::new();
        if let Some(port) = host.port {
            args.extend(["-p".into(), port.to_string().into()]);
        }
        if let Some(user) = &host.user {
            args.extend(["-l".into(), user.into()]);
        }
        args
    }

    /// Everything that makes a connection safe is set on the command line, which wins over the person's ssh config: no
    /// prompts, only pinned host keys, no forwarding of the agent, X11, ports or local commands.
    pub fn args(&self, host: &SshHost, command: &str) -> Vec<OsString> {
        let mut args: Vec<OsString> = self.extra.clone();
        args.push("-T".into());
        for option in [
            "BatchMode=yes",
            "StrictHostKeyChecking=yes",
            "GlobalKnownHostsFile=/dev/null",
            "ForwardAgent=no",
            "ForwardX11=no",
            "ClearAllForwardings=yes",
            "PermitLocalCommand=no",
            "ConnectTimeout=10",
            "ServerAliveInterval=15",
            "ServerAliveCountMax=2",
            "ControlMaster=auto",
            "ControlPersist=60",
            "LogLevel=ERROR",
        ] {
            args.extend(["-o".into(), option.into()]);
        }
        let mut known = OsString::from("UserKnownHostsFile=");
        known.push(&self.known_hosts);
        let mut control = OsString::from("ControlPath=");
        control.push(self.control_dir.join("%C"));
        args.extend(["-o".into(), known, "-o".into(), control]);
        args.extend(Self::destination(host));
        args.extend(["--".into(), (&host.target).into(), command.into()]);
        args
    }

    fn ensure_dirs(&self) -> std::io::Result<()> {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&self.control_dir)?;
        if let Some(parent) = self.known_hosts.parent() {
            std::fs::create_dir_all(parent)?;
        }
        Ok(())
    }

    pub async fn exec(&self, host: &SshHost, command: &str, timeout: Duration) -> ExecOutcome {
        let failed = |error: String| ExecOutcome { exit_code: None, stdout: String::new(), stderr: String::new(), timed_out: false, truncated: false, error: Some(error) };
        if let Err(error) = self.ensure_dirs() {
            return failed(format!("could not prepare ssh: {error}"));
        }
        let mut cmd = Command::new("ssh");
        cmd.args(self.args(host, command));
        match run(cmd, timeout).await {
            Ok(done) => ExecOutcome { exit_code: done.code, stdout: done.stdout, stderr: done.stderr, timed_out: false, truncated: done.truncated, error: None },
            Err(RunError::TimedOut) => ExecOutcome { timed_out: true, ..failed(String::new()) }.without_error(),
            Err(RunError::Spawn) => failed("ssh could not be started".into()),
        }
    }

    /// The address and port `ssh` really connects to, after the person's ssh config has had its say.
    async fn resolve(&self, host: &SshHost) -> Result<(String, u16), String> {
        let mut cmd = Command::new("ssh");
        cmd.args(&self.extra).args(Self::destination(host)).args(["-G", "--"]).arg(&host.target);
        let done = run(cmd, CONNECT).await.map_err(|_| "ssh could not be started".to_owned())?;
        let field = |name: &str| done.stdout.lines().find_map(|line| line.strip_prefix(name)?.strip_prefix(' ')).map(str::trim);
        let address = field("hostname").ok_or("ssh did not say where this host is")?;
        let port = field("port").and_then(|p| p.parse().ok()).unwrap_or(22);
        Ok((address.to_owned(), port))
    }

    fn known_name(address: &str, port: u16) -> String {
        if port == 22 { address.to_owned() } else { format!("[{address}]:{port}") }
    }

    pub async fn is_trusted(&self, host: &SshHost) -> bool {
        let Ok((address, port)) = self.resolve(host).await else { return false };
        if !self.known_hosts.exists() {
            return false;
        }
        let mut cmd = Command::new("ssh-keygen");
        cmd.arg("-F").arg(Self::known_name(&address, port)).arg("-f").arg(&self.known_hosts);
        run(cmd, CONNECT).await.is_ok_and(|done: Finished| done.success)
    }

    /// Asks the server for its host keys. Nothing is pinned until the person has seen the fingerprints.
    pub async fn scan(&self, host: &SshHost) -> Result<Vec<ScannedKey>, String> {
        let (address, port) = self.resolve(host).await?;
        let mut cmd = Command::new("ssh-keyscan");
        cmd.args(["-T", "5", "-p", &port.to_string(), "--"]).arg(&address);
        let done = run(cmd, CONNECT).await.map_err(|_| "ssh-keyscan could not be run".to_owned())?;
        let lines: Vec<String> = done.stdout.lines().filter(|l| !l.starts_with('#') && !l.trim().is_empty()).map(str::to_owned).collect();
        if lines.is_empty() {
            return Err(format!("{address} did not offer a host key on port {port}"));
        }
        self.ensure_dirs().map_err(|e| e.to_string())?;
        let listing = self.control_dir.join(format!("scan-{}", random_hex(8)));
        std::fs::write(&listing, lines.join("\n") + "\n").map_err(|e| e.to_string())?;
        let mut cmd = Command::new("ssh-keygen");
        cmd.arg("-lf").arg(&listing);
        let printed = run(cmd, CONNECT).await;
        let _ = std::fs::remove_file(&listing);
        let printed = printed.map_err(|_| "ssh-keygen could not be run".to_owned())?;
        let fingerprints: Vec<String> = printed
            .stdout
            .lines()
            .filter_map(|line| {
                let mut parts = line.split_whitespace();
                let (_, hash) = (parts.next()?, parts.next()?);
                Some(format!("{hash} {}", parts.next_back().unwrap_or_default()))
            })
            .collect();
        if fingerprints.len() != lines.len() {
            return Err("could not read the host key fingerprints".into());
        }
        Ok(lines.into_iter().zip(fingerprints).map(|(line, fingerprint)| ScannedKey { line, fingerprint }).collect())
    }

    /// Pins exactly the keys the person was shown.
    pub fn pin(&self, keys: &[ScannedKey]) -> Result<(), String> {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        self.ensure_dirs().map_err(|e| e.to_string())?;
        let existing = std::fs::read_to_string(&self.known_hosts).unwrap_or_default();
        let mut file = std::fs::OpenOptions::new().append(true).create(true).mode(0o600).open(&self.known_hosts).map_err(|e| e.to_string())?;
        for key in keys.iter().filter(|key| !existing.lines().any(|line| line == key.line)) {
            writeln!(file, "{}", key.line).map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    pub fn known_hosts(&self) -> &Path {
        &self.known_hosts
    }
}

impl ExecOutcome {
    fn without_error(mut self) -> Self {
        self.error = None;
        self
    }
}
