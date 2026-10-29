use std::io::ErrorKind;
use std::time::Duration;
use tokio::process::{Child, Command};

const MAX_OUTPUT: usize = 64 * 1024;

pub struct Finished {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

pub enum RunError {
    Spawn,
    TimedOut,
}

/// Linux refuses to exec a file some process still has open for writing (ETXTBSY), which is how a CLI that is updating
/// itself looks for a moment.
pub async fn spawn(cmd: &mut Command) -> std::io::Result<Child> {
    for _ in 0..20 {
        match cmd.spawn() {
            Err(error) if error.kind() == ErrorKind::ExecutableFileBusy => tokio::time::sleep(Duration::from_millis(10)).await,
            other => return other,
        }
    }
    cmd.spawn()
}

pub async fn run(mut cmd: Command, timeout: Duration) -> Result<Finished, RunError> {
    cmd.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped()).kill_on_drop(true);
    let child = spawn(&mut cmd).await.map_err(|_| RunError::Spawn)?;
    match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Err(_) => Err(RunError::TimedOut),
        Ok(Err(_)) => Err(RunError::Spawn),
        Ok(Ok(out)) => Ok(Finished {
            success: out.status.success(),
            stdout: capped(out.stdout),
            stderr: capped(out.stderr),
        }),
    }
}

fn capped(mut bytes: Vec<u8>) -> String {
    bytes.truncate(MAX_OUTPUT);
    String::from_utf8_lossy(&bytes).into_owned()
}
