//! Manages a per-analysis on-disk worktree containing the PR head commit.
//!
//! Cloning is done via `git` to keep dependencies thin and to inherit any
//! ambient credential helpers. The temp directory is cleaned up on drop.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use tempfile::TempDir;
use tracing::info;

use crate::analysis::runner::{self, ProcessStatus};
use crate::github::redact_url;

pub struct Workspace {
    /// Held to keep the temp directory alive for the analysis lifetime.
    _tmp: TempDir,
    root: PathBuf,
    head_sha: String,
    repo_full_name: String,
}

impl Workspace {
    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn head_sha(&self) -> &str {
        &self.head_sha
    }

    pub fn repo_full_name(&self) -> &str {
        &self.repo_full_name
    }
}

#[derive(Debug, Clone)]
pub struct CheckoutSpec {
    pub clone_url: String,
    pub head_sha: String,
    pub head_ref: String,
    pub repo_full_name: String,
    pub workdir_root: PathBuf,
    pub clone_timeout: Duration,
}

/// Clone the given repo into a temp directory under `workdir_root` and check
/// out `head_sha`. Performs a shallow fetch of the single commit.
pub async fn checkout(spec: CheckoutSpec) -> Result<Workspace> {
    tokio::fs::create_dir_all(&spec.workdir_root)
        .await
        .with_context(|| {
            format!(
                "failed to create workdir_root: {}",
                spec.workdir_root.display()
            )
        })?;

    let tmp = TempDir::new_in(&spec.workdir_root).with_context(|| {
        format!(
            "failed to create temp workdir in {}",
            spec.workdir_root.display()
        )
    })?;
    let root = tmp.path().to_path_buf();

    info!(
        repo = %spec.repo_full_name,
        sha = %spec.head_sha,
        path = %root.display(),
        "cloning PR head into worktree"
    );

    // 1. git init (quiet to avoid spam in tracing)
    let init = runner::run("git", &["init", "--quiet"], &root, &[], spec.clone_timeout).await;
    ensure_ok(&init, "git init")?;

    // 2. add the remote
    let remote = runner::run(
        "git",
        &["remote", "add", "origin", spec.clone_url.as_str()],
        &root,
        &[],
        spec.clone_timeout,
    )
    .await;
    ensure_ok(&remote, "git remote add")?;

    // 3. fetch the single SHA (shallow). Some remotes do not accept SHA
    //    fetches without `uploadpack.allowReachableSHA1InWant`, in which case
    //    we fall back to fetching the branch ref.
    let fetch_sha = runner::run(
        "git",
        &["fetch", "--depth=1", "origin", spec.head_sha.as_str()],
        &root,
        &[],
        spec.clone_timeout,
    )
    .await;

    if !fetch_sha.status.is_success() {
        let fetch_ref = runner::run(
            "git",
            &["fetch", "--depth=1", "origin", spec.head_ref.as_str()],
            &root,
            &[],
            spec.clone_timeout,
        )
        .await;
        ensure_ok(&fetch_ref, "git fetch (ref fallback)")?;
    }

    // 4. checkout the SHA in detached HEAD
    let checkout = runner::run(
        "git",
        &["checkout", "--quiet", "--detach", spec.head_sha.as_str()],
        &root,
        &[],
        spec.clone_timeout,
    )
    .await;
    ensure_ok(&checkout, "git checkout")?;

    Ok(Workspace {
        _tmp: tmp,
        root,
        head_sha: spec.head_sha,
        repo_full_name: spec.repo_full_name,
    })
}

fn ensure_ok(outcome: &runner::ProcessOutcome, label: &str) -> Result<()> {
    match outcome.status {
        ProcessStatus::Exited { code: 0 } => Ok(()),
        other => Err(anyhow!(
            "{label} failed ({other:?}): cmd=`{}` stderr={}",
            redact_url(&outcome.command),
            redact_url(&outcome.stderr_tail)
        )),
    }
}
