use std::time::Duration;
use tokio::process::Command;

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

pub async fn run(mut cmd: Command, timeout: Duration) -> Result<Finished, RunError> {
    cmd.stdin(std::process::Stdio::null()).kill_on_drop(true);
    match tokio::time::timeout(timeout, cmd.output()).await {
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
