//! Process runner used by analyzers. Captures stdout/stderr, applies a
//! timeout, and trims output tails to keep memory bounded.

use std::path::Path;
use std::process::Stdio;
use std::time::{Duration, Instant};

use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;
use tokio::task::JoinHandle;
use tokio::time::timeout;
use tracing::warn;

use crate::github::redact_url;

/// Maximum bytes of stdout/stderr we retain for reporting. Beyond this we
/// keep the tail (the part most useful for diagnosing failures).
const MAX_CAPTURED_BYTES: usize = 16 * 1024;
const MAX_AGGREGATED_BYTES: usize = 128 * 1024;
const PIPE_DRAIN_TIMEOUT: Duration = Duration::from_secs(2);

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
    let command_str = redact_url(&format_command(program, args));

    let analyzer_home = cwd.join(".formal-methods-home");
    let cargo_home = analyzer_home.join(".cargo");
    let cargo_target = cwd.join(".formal-methods-target");
    if let Err(err) =
        std::fs::create_dir_all(&cargo_home).and_then(|_| std::fs::create_dir_all(&cargo_target))
    {
        return ProcessOutcome {
            status: ProcessStatus::SpawnError,
            stdout_tail: String::new(),
            stderr_tail: format!("failed to create isolated analyzer directories: {err}"),
            duration: started.elapsed(),
            command: command_str,
        };
    }

    let mut cmd = Command::new(program);
    cmd.args(args)
        .current_dir(cwd)
        .env_clear()
        .env("HOME", &analyzer_home)
        .env("CARGO_HOME", &cargo_home)
        .env("CARGO_TARGET_DIR", &cargo_target)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    cmd.process_group(0);

    for key in [
        "PATH",
        "RUSTUP_HOME",
        "RUSTC",
        "SSL_CERT_FILE",
        "SSL_CERT_DIR",
        "LANG",
        "LC_ALL",
    ] {
        if let Some(value) = std::env::var_os(key) {
            cmd.env(key, value);
        }
    }
    for (k, v) in extra_envs {
        cmd.env(k, v);
    }

    let mut child = match cmd.spawn() {
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

    let stdout_task = child.stdout.take().map(spawn_tail_capture);
    let stderr_task = child.stderr.take().map(spawn_tail_capture);

    let status = match timeout(deadline, child.wait()).await {
        Ok(Ok(exit_status)) => {
            if let Some(code) = exit_status.code() {
                ProcessStatus::Exited { code }
            } else {
                ProcessStatus::Signalled
            }
        }
        Ok(Err(err)) => {
            warn!(error = %err, command = %command_str, "failed while waiting for process");
            ProcessStatus::SpawnError
        }
        Err(_) => {
            #[cfg(unix)]
            if let Some(process_id) = child.id() {
                // The analyzers (notably Cargo) spawn subprocess trees. Each
                // command has its own process group, so the timeout can stop
                // descendants as well as the direct child.
                unsafe {
                    libc::kill(-(process_id as i32), libc::SIGKILL);
                }
            }
            let _ = child.start_kill();
            let _ = child.wait().await;
            ProcessStatus::TimedOut
        }
    };

    let stdout = join_tail_capture(stdout_task).await;
    let stderr = join_tail_capture(stderr_task).await;
    let stderr_tail = if status == ProcessStatus::TimedOut && stderr.is_empty() {
        format!("timed out after {}s", deadline.as_secs_f64())
    } else if status == ProcessStatus::SpawnError && stderr.is_empty() {
        "failed while waiting for process".to_string()
    } else {
        tail_lossy(&stderr)
    };

    ProcessOutcome {
        status,
        stdout_tail: tail_lossy(&stdout),
        stderr_tail,
        duration: started.elapsed(),
        command: command_str,
    }
}

fn spawn_tail_capture<R>(reader: R) -> JoinHandle<Vec<u8>>
where
    R: AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(capture_tail(reader))
}

async fn join_tail_capture(task: Option<JoinHandle<Vec<u8>>>) -> Vec<u8> {
    match task {
        Some(mut task) => match timeout(PIPE_DRAIN_TIMEOUT, &mut task).await {
            Ok(result) => result.unwrap_or_default(),
            Err(_) => {
                task.abort();
                Vec::new()
            }
        },
        None => Vec::new(),
    }
}

async fn capture_tail(mut reader: impl AsyncRead + Unpin) -> Vec<u8> {
    // Retain one extra byte so `tail_lossy` can distinguish an exactly-full
    // stream from a truncated one and include its marker.
    let retained_limit = MAX_CAPTURED_BYTES + 1;
    let mut retained = Vec::with_capacity(retained_limit);
    let mut chunk = [0_u8; 8192];
    loop {
        let read = match reader.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(read) => read,
        };
        if read >= retained_limit {
            retained.clear();
            retained.extend_from_slice(&chunk[read - retained_limit..read]);
            continue;
        }
        let overflow = retained
            .len()
            .saturating_add(read)
            .saturating_sub(retained_limit);
        if overflow > 0 {
            retained.drain(..overflow);
        }
        retained.extend_from_slice(&chunk[..read]);
    }
    retained
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

pub(crate) fn append_aggregated_tail(target: &mut String, value: &str) {
    target.push_str(value);
    if target.len() <= MAX_AGGREGATED_BYTES {
        return;
    }
    let mut start = target.len() - MAX_AGGREGATED_BYTES;
    while !target.is_char_boundary(start) {
        start += 1;
    }
    let tail = target[start..].to_string();
    target.clear();
    target.push_str("...[aggregate output truncated]\n");
    target.push_str(&tail);
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

    #[cfg(unix)]
    #[tokio::test]
    async fn timeout_terminates_subprocess_group_without_hanging_on_pipes() {
        let cwd = std::env::temp_dir();
        let outcome = run(
            "sh",
            &["-c", "sleep 30 & wait"],
            &cwd,
            &[],
            Duration::from_millis(100),
        )
        .await;
        assert_eq!(outcome.status, ProcessStatus::TimedOut);
        assert!(outcome.duration < Duration::from_secs(5), "{outcome:?}");
    }

    #[tokio::test]
    async fn run_bounds_child_output_while_draining_pipes() {
        let cwd = std::env::temp_dir();
        let outcome = run(
            "sh",
            &["-c", "yes x | head -c 1048576"],
            &cwd,
            &[],
            Duration::from_secs(5),
        )
        .await;
        assert!(outcome.status.is_success(), "{outcome:?}");
        assert!(outcome.stdout_tail.len() <= MAX_CAPTURED_BYTES + 32);
        assert!(outcome.stdout_tail.starts_with("...[truncated]"));
    }

    #[tokio::test]
    async fn run_does_not_inherit_service_secrets() {
        let cwd = std::env::temp_dir();
        let outcome = run(
            "sh",
            &["-c", "test -z \"${GITHUB_TOKEN-}\" && printf isolated"],
            &cwd,
            &[],
            Duration::from_secs(5),
        )
        .await;
        assert!(outcome.status.is_success(), "{outcome:?}");
        assert_eq!(outcome.stdout_tail, "isolated");
    }
}
