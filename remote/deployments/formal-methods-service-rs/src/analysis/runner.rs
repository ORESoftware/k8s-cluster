//! Process runner used by analyzers. Captures stdout/stderr, applies a
//! timeout, and trims output tails to keep memory bounded.

use std::path::Path;
use std::process::Stdio;
use std::time::{Duration, Instant};

use tokio::process::Command;
use tokio::time::timeout;
use tracing::warn;

/// Maximum bytes of stdout/stderr we retain for reporting. Beyond this we
/// keep the tail (the part most useful for diagnosing failures).
const MAX_CAPTURED_BYTES: usize = 16 * 1024;

#[derive(Debug)]
pub struct ProcessOutcome {
    pub status: ProcessStatus,
    pub stdout_tail: String,
    pub stderr_tail: String,
    pub duration: Duration,
    pub command: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessStatus {
    Exited { code: i32 },
    Signalled,
    TimedOut,
    SpawnError,
}

impl ProcessStatus {
    pub fn is_success(self) -> bool {
        matches!(self, ProcessStatus::Exited { code: 0 })
    }
}

/// Run a process, capturing output with a hard timeout.
pub async fn run(
    program: &str,
    args: &[&str],
    cwd: &Path,
    extra_envs: &[(&str, &str)],
    deadline: Duration,
) -> ProcessOutcome {
    let started = Instant::now();
    let command_str = format_command(program, args);

    let mut cmd = Command::new(program);
    cmd.args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    for (k, v) in extra_envs {
        cmd.env(k, v);
    }

    let spawn = match cmd.spawn() {
        Ok(child) => child,
        Err(err) => {
            warn!(error = %err, command = %command_str, "failed to spawn process");
            return ProcessOutcome {
                status: ProcessStatus::SpawnError,
                stdout_tail: String::new(),
                stderr_tail: format!("spawn error: {err}"),
                duration: started.elapsed(),
                command: command_str,
            };
        }
    };

    let wait = spawn.wait_with_output();
    match timeout(deadline, wait).await {
        Ok(Ok(output)) => {
            let status = if let Some(code) = output.status.code() {
                ProcessStatus::Exited { code }
            } else {
                ProcessStatus::Signalled
            };
            ProcessOutcome {
                status,
                stdout_tail: tail_lossy(&output.stdout),
                stderr_tail: tail_lossy(&output.stderr),
                duration: started.elapsed(),
                command: command_str,
            }
        }
        Ok(Err(err)) => ProcessOutcome {
            status: ProcessStatus::SpawnError,
            stdout_tail: String::new(),
            stderr_tail: format!("wait error: {err}"),
            duration: started.elapsed(),
            command: command_str,
        },
        Err(_) => ProcessOutcome {
            status: ProcessStatus::TimedOut,
            stdout_tail: String::new(),
            stderr_tail: format!("timed out after {}s", deadline.as_secs()),
            duration: started.elapsed(),
            command: command_str,
        },
    }
}

fn format_command(program: &str, args: &[&str]) -> String {
    let mut out =
        String::with_capacity(program.len() + args.iter().map(|a| a.len() + 1).sum::<usize>());
    out.push_str(program);
    for a in args {
        out.push(' ');
        out.push_str(a);
    }
    out
}

fn tail_lossy(bytes: &[u8]) -> String {
    if bytes.len() <= MAX_CAPTURED_BYTES {
        return String::from_utf8_lossy(bytes).into_owned();
    }
    // Slice from the end; align to a UTF-8 boundary by walking forward to the
    // next valid char start.
    let start = bytes.len() - MAX_CAPTURED_BYTES;
    let mut aligned = start;
    while aligned < bytes.len() && (bytes[aligned] & 0b1100_0000) == 0b1000_0000 {
        aligned += 1;
    }
    let mut s = String::from("...[truncated]\n");
    s.push_str(&String::from_utf8_lossy(&bytes[aligned..]));
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tail_short_input_is_lossless() {
        let s = tail_lossy(b"hello");
        assert_eq!(s, "hello");
    }

    #[test]
    fn tail_long_input_is_truncated() {
        let buf = vec![b'a'; MAX_CAPTURED_BYTES + 100];
        let s = tail_lossy(&buf);
        assert!(s.starts_with("...[truncated]"));
        // truncated marker + at least MAX_CAPTURED_BYTES of payload
        assert!(s.len() >= MAX_CAPTURED_BYTES);
    }

    #[tokio::test]
    async fn run_captures_stdout() {
        let cwd = std::env::temp_dir();
        let outcome = run("echo", &["hello"], &cwd, &[], Duration::from_secs(5)).await;
        assert!(outcome.status.is_success(), "{:?}", outcome);
        assert!(outcome.stdout_tail.contains("hello"));
    }

    #[tokio::test]
    async fn run_reports_nonzero_exit() {
        let cwd = std::env::temp_dir();
        let outcome = run("sh", &["-c", "exit 7"], &cwd, &[], Duration::from_secs(5)).await;
        assert_eq!(outcome.status, ProcessStatus::Exited { code: 7 });
    }

    #[tokio::test]
    async fn run_times_out() {
        let cwd = std::env::temp_dir();
        let outcome = run(
            "sh",
            &["-c", "sleep 5"],
            &cwd,
            &[],
            Duration::from_millis(100),
        )
        .await;
        assert_eq!(outcome.status, ProcessStatus::TimedOut);
    }
}
