//! Multi-VCS backend for dd-git-rs.
//!
//! Each supported version-control system (git, mercurial, subversion, fossil)
//! is driven through its own CLI. Every command is executed with an explicit
//! argument vector via `tokio::process::Command` — never through a shell — so
//! caller-supplied values (refs, revisions) cannot inject options or commands.
//! All user-facing identifiers are validated before they reach a command line,
//! output is capped, and every invocation runs under a timeout.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use serde::Serialize;
use serde_json::{json, Value};
use tokio::io::AsyncReadExt;
use tokio::process::Command;

/// Hard ceiling on captured stdout/stderr per stream. Protects the process from
/// pathological repositories without truncating ordinary inspection output.
pub const DEFAULT_MAX_OUTPUT_BYTES: usize = 4 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VcsKind {
    Git,
    Hg,
    Svn,
    Fossil,
}

impl VcsKind {
    pub const ALL: [VcsKind; 4] = [VcsKind::Git, VcsKind::Hg, VcsKind::Svn, VcsKind::Fossil];

    pub fn parse(value: &str) -> Option<VcsKind> {
        match value.trim().to_ascii_lowercase().as_str() {
            "git" => Some(VcsKind::Git),
            "hg" | "mercurial" => Some(VcsKind::Hg),
            "svn" | "subversion" => Some(VcsKind::Svn),
            "fossil" => Some(VcsKind::Fossil),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            VcsKind::Git => "git",
            VcsKind::Hg => "hg",
            VcsKind::Svn => "svn",
            VcsKind::Fossil => "fossil",
        }
    }

    /// The CLI binary that drives this VCS.
    pub fn binary(&self) -> &'static str {
        match self {
            VcsKind::Git => "git",
            VcsKind::Hg => "hg",
            VcsKind::Svn => "svn",
            VcsKind::Fossil => "fossil",
        }
    }

    /// Human label for descriptors and docs.
    pub fn label(&self) -> &'static str {
        match self {
            VcsKind::Git => "Git",
            VcsKind::Hg => "Mercurial",
            VcsKind::Svn => "Subversion",
            VcsKind::Fossil => "Fossil",
        }
    }

    /// Whether the mirror destination is a single file (fossil) rather than a
    /// directory. Affects how the server lays out on-disk storage.
    pub fn mirror_is_file(&self) -> bool {
        matches!(self, VcsKind::Fossil)
    }

    /// Accepted remote-URL schemes for this VCS. `file://` is intentionally
    /// excluded here — it is a local-file-disclosure vector and is only added
    /// back when `GIT_RS_ALLOW_FILE_URLS=true`.
    pub fn allowed_url_prefixes(&self) -> &'static [&'static str] {
        match self {
            VcsKind::Git => &["https://", "http://", "git://", "ssh://", "git@"],
            VcsKind::Hg => &["https://", "http://", "ssh://"],
            VcsKind::Svn => &["https://", "http://", "svn://", "svn+ssh://"],
            VcsKind::Fossil => &["https://", "http://", "ssh://"],
        }
    }
}

#[derive(Debug)]
pub enum VcsError {
    Spawn(String),
    Timeout(u64),
    NonZero { code: Option<i32>, stderr: String },
}

impl std::fmt::Display for VcsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VcsError::Spawn(message) => write!(f, "spawn failed: {message}"),
            VcsError::Timeout(seconds) => write!(f, "command timed out after {seconds}s"),
            VcsError::NonZero { code, stderr } => {
                let code = code.map(|c| c.to_string()).unwrap_or_else(|| "signal".to_string());
                write!(f, "command exited {code}: {}", stderr.trim())
            }
        }
    }
}

/// A bounded, structured result of running one VCS command.
pub struct CmdOutput {
    pub success: bool,
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub truncated: bool,
}

/// Run `binary args...` with no shell. `cwd` is the working directory; `envs`
/// are appended to a hardened, prompt-free base environment.
pub async fn run(
    binary: &str,
    args: &[String],
    cwd: Option<&Path>,
    envs: &BTreeMap<String, String>,
    timeout: Duration,
    max_bytes: usize,
) -> Result<CmdOutput, VcsError> {
    let mut command = Command::new(binary);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if let Some(dir) = cwd {
        command.current_dir(dir);
    }
    // Hardened defaults: never prompt, never read interactive credentials, and
    // keep a stable plain output locale for the VCS CLIs that honor it.
    command
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_ASKPASS", "/bin/true")
        .env("GIT_SSH_COMMAND", "ssh -oBatchMode=yes -oStrictHostKeyChecking=accept-new")
        .env("GCM_INTERACTIVE", "never")
        .env("HGPLAIN", "1")
        .env("SVN_SSH", "ssh -oBatchMode=yes -oStrictHostKeyChecking=accept-new")
        .env("LC_ALL", "C.UTF-8")
        .env("PAGER", "cat")
        .env("GIT_PAGER", "cat");
    for (key, value) in envs {
        command.env(key, value);
    }

    let mut child = command
        .spawn()
        .map_err(|error| VcsError::Spawn(format!("{binary}: {error}")))?;

    let mut stdout_pipe = child.stdout.take();
    let mut stderr_pipe = child.stderr.take();

    let mut stdout_buf = Vec::new();
    let mut stderr_buf = Vec::new();
    let timeout_secs = timeout.as_secs();

    let collect = async {
        // Read both pipes concurrently so a chatty stderr can't deadlock stdout.
        let read_out = read_capped(&mut stdout_pipe, &mut stdout_buf, max_bytes);
        let read_err = read_capped(&mut stderr_pipe, &mut stderr_buf, max_bytes);
        tokio::join!(read_out, read_err);
        child.wait().await
    };

    let status = match tokio::time::timeout(timeout, collect).await {
        Ok(Ok(status)) => status,
        Ok(Err(error)) => return Err(VcsError::Spawn(format!("{binary}: {error}"))),
        Err(_) => {
            let _ = child.start_kill();
            return Err(VcsError::Timeout(timeout_secs));
        }
    };

    let truncated = stdout_buf.len() >= max_bytes || stderr_buf.len() >= max_bytes;
    Ok(CmdOutput {
        success: status.success(),
        code: status.code(),
        stdout: String::from_utf8_lossy(&stdout_buf).into_owned(),
        stderr: String::from_utf8_lossy(&stderr_buf).into_owned(),
        truncated,
    })
}

async fn read_capped<R>(pipe: &mut Option<R>, buf: &mut Vec<u8>, max_bytes: usize)
where
    R: tokio::io::AsyncRead + Unpin,
{
    let Some(reader) = pipe.as_mut() else {
        return;
    };
    let mut chunk = [0u8; 16 * 1024];
    loop {
        match reader.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                if buf.len() < max_bytes {
                    let room = max_bytes - buf.len();
                    buf.extend_from_slice(&chunk[..n.min(room)]);
                }
                // Keep draining past the cap so the child is never blocked on a
                // full pipe, but stop storing once we have enough.
            }
        }
    }
}

/// Convenience: turn a non-success [`CmdOutput`] into a [`VcsError`].
pub fn require_success(output: CmdOutput) -> Result<CmdOutput, VcsError> {
    if output.success {
        Ok(output)
    } else {
        Err(VcsError::NonZero {
            code: output.code,
            stderr: if output.stderr.trim().is_empty() {
                output.stdout
            } else {
                output.stderr
            },
        })
    }
}

// ---------------------------------------------------------------------------
// Command builders. Each returns the argument vector for a given operation.
// `dest` is the server-owned on-disk path; it is derived from the repo slug,
// never from caller input.
// ---------------------------------------------------------------------------

/// Probe whether a VCS binary is installed (runs `<bin> --version`).
pub fn probe_args(_kind: VcsKind) -> Vec<String> {
    vec!["--version".to_string()]
}

/// Initial mirror/clone of `remote_url` into `dest`.
pub fn mirror_args(kind: VcsKind, remote_url: &str, dest: &str) -> Vec<String> {
    match kind {
        VcsKind::Git => svec(&["clone", "--mirror", "--quiet", "--", remote_url, dest]),
        VcsKind::Hg => svec(&["clone", "-U", "--", remote_url, dest]),
        VcsKind::Svn => svec(&["checkout", "--non-interactive", "--quiet", "--", remote_url, dest]),
        VcsKind::Fossil => svec(&["clone", "--", remote_url, dest]),
    }
}

/// Re-sync an existing mirror at `dest` from its origin.
pub fn fetch_args(kind: VcsKind, dest: &str) -> Vec<String> {
    match kind {
        VcsKind::Git => svec(&["-C", dest, "remote", "update", "--prune"]),
        VcsKind::Hg => svec(&["-R", dest, "pull"]),
        VcsKind::Svn => svec(&["update", "--non-interactive", "--quiet", dest]),
        VcsKind::Fossil => svec(&["pull", "-R", dest]),
    }
}

/// List refs (branches/tags/bookmarks). For git/hg the output is parsed into
/// structured rows; for svn/fossil it is returned as labeled text.
pub fn refs_args(kind: VcsKind, dest: &str) -> Vec<String> {
    match kind {
        VcsKind::Git => svec(&[
            "-C",
            dest,
            "for-each-ref",
            "--format=%(objectname) %(refname) %(objecttype)",
        ]),
        VcsKind::Hg => svec(&["-R", dest, "branches", "-T", "json"]),
        VcsKind::Svn => svec(&["info", "--non-interactive", dest]),
        VcsKind::Fossil => svec(&["branch", "list", "-R", dest]),
    }
}

/// Commit/changeset log. `rev` is an already-validated ref or revision.
pub fn log_args(kind: VcsKind, dest: &str, rev: Option<&str>, limit: i64) -> Vec<String> {
    let limit = limit.clamp(1, 1000).to_string();
    match kind {
        VcsKind::Git => {
            let mut args = svec(&[
                "-C",
                dest,
                "log",
                "--no-color",
                "--max-count",
                &limit,
                "--pretty=format:%H%x1f%an%x1f%ae%x1f%aI%x1f%s",
            ]);
            if let Some(rev) = rev {
                args.push(rev.to_string());
            }
            args.push("--".to_string());
            args
        }
        VcsKind::Hg => {
            let mut args = svec(&["-R", dest, "log", "-l", &limit, "-T", "json"]);
            if let Some(rev) = rev {
                args.push("-r".to_string());
                args.push(format!("reverse(::{rev})"));
            }
            args
        }
        VcsKind::Svn => {
            let mut args = svec(&["log", "--non-interactive", "-l", &limit]);
            if let Some(rev) = rev {
                args.push("-r".to_string());
                args.push(format!("{rev}:0"));
            }
            args.push(dest.to_string());
            args
        }
        VcsKind::Fossil => svec(&["timeline", "-t", "ci", "-n", &limit, "-R", dest]),
    }
}

/// Show a single commit/revision (with diff where the VCS supports it).
pub fn show_args(kind: VcsKind, dest: &str, rev: &str) -> Vec<String> {
    match kind {
        VcsKind::Git => svec(&["-C", dest, "show", "--no-color", "--stat", "-p", rev, "--"]),
        VcsKind::Hg => svec(&["-R", dest, "log", "-r", rev, "-p"]),
        VcsKind::Svn => svec(&["log", "--non-interactive", "-v", "-r", rev, dest]),
        VcsKind::Fossil => svec(&["info", rev, "-R", dest]),
    }
}

fn svec(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|s| s.to_string()).collect()
}

// ---------------------------------------------------------------------------
// Output parsers.
// ---------------------------------------------------------------------------

#[derive(Serialize)]
pub struct RefEntry {
    pub name: String,
    #[serde(rename = "type")]
    pub ref_type: String,
    pub target: String,
    #[serde(rename = "isDefault")]
    pub is_default: bool,
}

/// Parse `git for-each-ref` output into structured refs.
pub fn parse_git_refs(stdout: &str, default_branch: &str) -> Vec<RefEntry> {
    let mut refs = Vec::new();
    for line in stdout.lines() {
        let mut parts = line.splitn(3, ' ');
        let (Some(target), Some(full_ref)) = (parts.next(), parts.next()) else {
            continue;
        };
        let (ref_type, short) = if let Some(name) = full_ref.strip_prefix("refs/heads/") {
            ("branch", name)
        } else if let Some(name) = full_ref.strip_prefix("refs/tags/") {
            ("tag", name)
        } else if let Some(name) = full_ref.strip_prefix("refs/remotes/") {
            ("branch", name)
        } else {
            ("other", full_ref)
        };
        refs.push(RefEntry {
            name: short.to_string(),
            ref_type: ref_type.to_string(),
            target: target.to_string(),
            is_default: ref_type == "branch" && short == default_branch,
        });
    }
    refs
}

/// Parse `hg branches -T json` into structured refs.
pub fn parse_hg_refs(stdout: &str, default_branch: &str) -> Vec<RefEntry> {
    let mut refs = Vec::new();
    if let Ok(Value::Array(items)) = serde_json::from_str::<Value>(stdout) {
        for item in items {
            let name = item.get("branch").and_then(Value::as_str).unwrap_or("");
            if name.is_empty() {
                continue;
            }
            let node = item.get("node").and_then(Value::as_str).unwrap_or("");
            refs.push(RefEntry {
                name: name.to_string(),
                ref_type: "branch".to_string(),
                target: node.to_string(),
                is_default: name == default_branch
                    || (default_branch == "main" && name == "default"),
            });
        }
    }
    refs
}

#[derive(Serialize)]
pub struct CommitEntry {
    pub revision: String,
    pub author: String,
    pub email: String,
    pub date: String,
    pub summary: String,
}

/// Parse the `git log` `%x1f`-delimited format into structured commits.
pub fn parse_git_log(stdout: &str) -> Vec<CommitEntry> {
    let mut commits = Vec::new();
    for line in stdout.lines() {
        if line.is_empty() {
            continue;
        }
        let fields: Vec<&str> = line.split('\u{1f}').collect();
        if fields.len() < 5 {
            continue;
        }
        commits.push(CommitEntry {
            revision: fields[0].to_string(),
            author: fields[1].to_string(),
            email: fields[2].to_string(),
            date: fields[3].to_string(),
            summary: fields[4].to_string(),
        });
    }
    commits
}

/// Parse `hg log -T json` into structured commits.
pub fn parse_hg_log(stdout: &str) -> Vec<CommitEntry> {
    let mut commits = Vec::new();
    if let Ok(Value::Array(items)) = serde_json::from_str::<Value>(stdout) {
        for item in items {
            let node = item.get("node").and_then(Value::as_str).unwrap_or("");
            if node.is_empty() {
                continue;
            }
            let user = item.get("user").and_then(Value::as_str).unwrap_or("");
            let (author, email) = split_hg_user(user);
            // hg json date is `[unixtime, tzoffset]`.
            let date = item
                .get("date")
                .and_then(Value::as_array)
                .and_then(|a| a.first())
                .and_then(Value::as_f64)
                .map(|secs| {
                    chrono::DateTime::from_timestamp(secs as i64, 0)
                        .map(|dt| dt.to_rfc3339())
                        .unwrap_or_default()
                })
                .unwrap_or_default();
            let summary = item
                .get("desc")
                .and_then(Value::as_str)
                .unwrap_or("")
                .lines()
                .next()
                .unwrap_or("")
                .to_string();
            commits.push(CommitEntry {
                revision: node.to_string(),
                author,
                email,
                date,
                summary,
            });
        }
    }
    commits
}

fn split_hg_user(user: &str) -> (String, String) {
    if let (Some(open), Some(close)) = (user.find('<'), user.find('>')) {
        if open < close {
            let name = user[..open].trim().to_string();
            let email = user[open + 1..close].trim().to_string();
            return (name, email);
        }
    }
    (user.trim().to_string(), String::new())
}

/// Best-effort structured `refs` payload across all VCS kinds. Git/hg yield
/// structured rows; svn/fossil yield a `text` block plus any rows we can lift.
pub fn refs_payload(kind: VcsKind, stdout: &str, default_branch: &str) -> (Vec<RefEntry>, Value) {
    match kind {
        VcsKind::Git => {
            let refs = parse_git_refs(stdout, default_branch);
            let value = json!({ "refs": &refs });
            (refs, value)
        }
        VcsKind::Hg => {
            let refs = parse_hg_refs(stdout, default_branch);
            let value = json!({ "refs": &refs });
            (refs, value)
        }
        VcsKind::Svn => {
            // `svn info` carries the HEAD revision; surface it as a single head.
            let revision = stdout
                .lines()
                .find_map(|line| line.strip_prefix("Revision: "))
                .unwrap_or("")
                .trim()
                .to_string();
            let refs = if revision.is_empty() {
                Vec::new()
            } else {
                vec![RefEntry {
                    name: "trunk".to_string(),
                    ref_type: "head".to_string(),
                    target: revision.clone(),
                    is_default: true,
                }]
            };
            let value = json!({ "refs": &refs, "text": stdout });
            (refs, value)
        }
        VcsKind::Fossil => {
            let refs: Vec<RefEntry> = stdout
                .lines()
                .map(|line| line.trim_start_matches('*').trim())
                .filter(|line| !line.is_empty())
                .map(|name| RefEntry {
                    name: name.to_string(),
                    ref_type: "branch".to_string(),
                    target: String::new(),
                    is_default: name == default_branch,
                })
                .collect();
            let value = json!({ "refs": &refs, "text": stdout });
            (refs, value)
        }
    }
}

/// Best-effort structured `log` payload. Git/hg yield structured commits;
/// svn/fossil yield a `text` block.
pub fn log_payload(kind: VcsKind, stdout: &str) -> Value {
    match kind {
        VcsKind::Git => json!({ "commits": parse_git_log(stdout) }),
        VcsKind::Hg => json!({ "commits": parse_hg_log(stdout) }),
        VcsKind::Svn | VcsKind::Fossil => json!({ "commits": [], "text": stdout }),
    }
}
