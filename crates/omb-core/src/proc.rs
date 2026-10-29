use std::io::ErrorKind;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::{Child, Command};

pub const MAX_OUTPUT: usize = 64 * 1024;

pub struct Finished {
    pub success: bool,
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    /// The process was stopped because it wrote more than `MAX_OUTPUT` to one stream.
    pub truncated: bool,
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

async fn read_capped(reader: Option<impl AsyncRead + Unpin>) -> (String, bool) {
    let Some(reader) = reader else { return (String::new(), false) };
    let mut bytes = Vec::new();
    let _ = reader.take(MAX_OUTPUT as u64 + 1).read_to_end(&mut bytes).await;
    let over = bytes.len() > MAX_OUTPUT;
    bytes.truncate(MAX_OUTPUT);
    (String::from_utf8_lossy(&bytes).into_owned(), over)
}

/// Runs to completion with no stdin, keeping at most `MAX_OUTPUT` of each stream. A process that outlives `timeout` or
/// floods a stream is killed.
pub async fn run(mut cmd: Command, timeout: Duration) -> Result<Finished, RunError> {
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    let mut child = spawn(&mut cmd).await.map_err(|_| RunError::Spawn)?;
    let (stdout, stderr) = (child.stdout.take(), child.stderr.take());
    let finish = async {
        let (mut out, mut err) = (Box::pin(read_capped(stdout)), Box::pin(read_capped(stderr)));
        let (mut read_out, mut read_err) = (None, None);
        while read_out.is_none() || read_err.is_none() {
            tokio::select! {
                done = &mut out, if read_out.is_none() => {
                    if done.1 { let _ = child.start_kill(); }
                    read_out = Some(done);
                }
                done = &mut err, if read_err.is_none() => {
                    if done.1 { let _ = child.start_kill(); }
                    read_err = Some(done);
                }
            }
        }
        let ((stdout, cut_out), (stderr, cut_err)) = (read_out.unwrap_or_default(), read_err.unwrap_or_default());
        let status = child.wait().await.map_err(|_| RunError::Spawn)?;
        Ok(Finished { success: status.success(), code: status.code(), stdout, stderr, truncated: cut_out || cut_err })
    };
    tokio::time::timeout(timeout, finish).await.unwrap_or(Err(RunError::TimedOut))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sh(script: &str) -> Command {
        let mut cmd = Command::new("/bin/sh");
        cmd.args(["-c", script]);
        cmd
    }

    #[tokio::test]
    async fn collects_output_and_the_exit_code() {
        let done = run(sh("echo out; echo err >&2; exit 3"), Duration::from_secs(10)).await.ok().unwrap();
        assert_eq!((done.stdout.trim(), done.stderr.trim(), done.code, done.success, done.truncated), ("out", "err", Some(3), false, false));
    }

    #[tokio::test]
    async fn a_flood_is_cut_and_the_process_stopped() {
        let done = run(sh("/usr/bin/yes"), Duration::from_secs(10)).await.ok().unwrap();
        assert!(done.truncated && done.stdout.len() == MAX_OUTPUT);
    }

    #[tokio::test]
    async fn a_process_that_outlives_the_timeout_is_an_error() {
        assert!(matches!(run(sh("/bin/sleep 30"), Duration::from_millis(200)).await, Err(RunError::TimedOut)));
    }

    #[tokio::test]
    async fn a_missing_binary_is_a_spawn_error() {
        assert!(matches!(run(Command::new("/nonexistent/bin"), Duration::from_secs(1)).await, Err(RunError::Spawn)));
    }
}
