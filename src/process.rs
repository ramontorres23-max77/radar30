//! Bounded subprocess execution: every outside program runs with stdin closed
//! (or a provided payload), captured output, and a hard wall-clock kill.
//! Modeled after `rust-anything`'s process discipline so a hung `yt-dlp` or
//! `security` call can never stall a research run or the MCP server.

use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

#[derive(Debug)]
pub enum ProcError {
    Missing(String),
    Timeout(String),
    Failed(String),
}

impl std::fmt::Display for ProcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProcError::Missing(d) => write!(f, "program not found: {d}"),
            ProcError::Timeout(d) => write!(f, "timed out: {d}"),
            ProcError::Failed(d) => write!(f, "failed: {d}"),
        }
    }
}

impl std::error::Error for ProcError {}

fn base_command(program: &str, args: &[&str]) -> Command {
    let mut cmd = Command::new(program);
    cmd.args(args).stdout(Stdio::piped()).stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    cmd
}

/// Run `program args` with stdin closed, capturing stdout, killing it at `timeout`.
pub fn run_capture(program: &str, args: &[&str], timeout: Duration) -> Result<String, ProcError> {
    run_inner(program, args, None, timeout)
}

/// Run `program args` feeding `input` to stdin, capturing stdout, killing at `timeout`.
pub fn run_with_stdin(
    program: &str,
    args: &[&str],
    input: &str,
    timeout: Duration,
) -> Result<String, ProcError> {
    run_inner(program, args, Some(input), timeout)
}

fn run_inner(
    program: &str,
    args: &[&str],
    input: Option<&str>,
    timeout: Duration,
) -> Result<String, ProcError> {
    let mut cmd = base_command(program, args);
    cmd.stdin(if input.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    });

    let mut child = cmd
        .spawn()
        .map_err(|e| ProcError::Missing(format!("{program}: {e}")))?;

    if let Some(payload) = input
        && let Some(mut stdin) = child.stdin.take()
    {
        let _ = stdin.write_all(payload.as_bytes());
        let _ = stdin.flush();
    }

    // Drain stdout on a helper thread so a full pipe buffer can never deadlock
    // the polling loop below; the parent keeps `child` so it can kill on timeout.
    let stdout = child.stdout.take();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        use std::io::Read;
        let mut buf = String::new();
        if let Some(mut out) = stdout {
            let _ = out.read_to_string(&mut buf);
        }
        let _ = tx.send(buf);
    });

    let deadline = std::time::Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let captured = rx.recv_timeout(Duration::from_secs(2)).unwrap_or_default();
                if status.success() {
                    return Ok(captured);
                }
                return Err(ProcError::Failed(format!(
                    "{program} exited {status}: {}",
                    captured.trim().chars().take(200).collect::<String>()
                )));
            }
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(ProcError::Timeout(format!(
                        "{program} still running after {}s",
                        timeout.as_secs()
                    )));
                }
                std::thread::sleep(Duration::from_millis(40));
            }
            Err(e) => return Err(ProcError::Failed(format!("{program}: {e}"))),
        }
    }
}

/// Whether `program` is on PATH.
pub fn on_path(program: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|paths| std::env::split_paths(&paths).any(|dir| dir.join(program).is_file()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captures_stdout_and_reports_missing_program() {
        let out = run_capture("echo", &["radar30"], Duration::from_secs(5)).unwrap();
        assert_eq!(out.trim(), "radar30");
        assert!(matches!(
            run_capture(
                "definitely-not-a-real-binary-xyz",
                &[],
                Duration::from_secs(1)
            ),
            Err(ProcError::Missing(_))
        ));
    }

    #[test]
    fn stdin_payload_is_delivered() {
        let out = run_with_stdin("cat", &[], "hello-stdin", Duration::from_secs(5)).unwrap();
        assert_eq!(out, "hello-stdin");
    }
}
