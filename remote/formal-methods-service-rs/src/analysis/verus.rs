//! Verus analyzer step.
//!
//! Drives `verus` against the proof crate sitting at
//! `<contract_root>/proofs/verus`. Skips gracefully when `verus` is not on
//! `PATH` (very common: Verus uses a custom rustc fork and is not
//! pre-installed in most CI environments).

use std::path::PathBuf;
use std::time::Duration;

use async_trait::async_trait;

use crate::analysis::runner::{self, ProcessStatus};
use crate::analysis::tools::check_tool_present;
use crate::analysis::workspace::Workspace;
use crate::analysis::{Analyzer, StepReport, StepStatus};

#[derive(Debug, Clone)]
pub struct VerusAnalyzer {
    /// Path (relative to the cloned PR worktree) of the Verus proof crate.
    pub proof_crate_dir: PathBuf,
    pub timeout: Duration,
}

#[async_trait]
impl Analyzer for VerusAnalyzer {
    fn name(&self) -> &str {
        "verus"
    }

    async fn run(&self, ws: &Workspace) -> StepReport {
        if !check_tool_present("verus", &["--version"]).await {
            return StepReport {
                name: self.name().to_string(),
                status: StepStatus::Skipped,
                duration: Duration::from_secs(0),
                stdout_tail: String::new(),
                stderr_tail: String::new(),
                summary: "verus not installed; skipping (see proofs/verus/README.md)".into(),
            };
        }

        let crate_dir = ws.root().join(&self.proof_crate_dir);
        // `verus` is invoked at the proof crate root; it discovers the
        // entry point via Cargo.toml.
        let outcome = runner::run("verus", &["--time", "."], &crate_dir, &[], self.timeout).await;

        let status = match outcome.status {
            ProcessStatus::Exited { code: 0 } => StepStatus::Passed,
            _ => StepStatus::Failed,
        };
        let summary = match outcome.status {
            ProcessStatus::Exited { code: 0 } => "all Verus obligations discharged".into(),
            ProcessStatus::Exited { code } => format!("verus exited {code}"),
            ProcessStatus::Signalled => "verus killed by signal".into(),
            ProcessStatus::TimedOut => "verus timed out".into(),
            ProcessStatus::SpawnError => "failed to launch verus".into(),
        };

        StepReport {
            name: self.name().to_string(),
            status,
            duration: outcome.duration,
            stdout_tail: outcome.stdout_tail,
            stderr_tail: outcome.stderr_tail,
            summary,
        }
    }
}
