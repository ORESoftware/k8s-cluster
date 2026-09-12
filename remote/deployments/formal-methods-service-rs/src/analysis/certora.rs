//! Certora analyzer step.
//!
//! Disabled by default. Treats every invocation as `Skipped` with a clear
//! message until either:
//!
//!   * `FORMAL_METHODS_CERTORA_ENABLED` is set to `true`, AND
//!   * `certoraRun` is on `PATH` (the Certora CLI), AND
//!   * a `proofs/certora/conf/` directory exists in the PR worktree.
//!
//! When all three hold, the step runs each `.conf` file under
//! `proofs/certora/conf/` through `certoraRun` and aggregates the results.

use std::path::PathBuf;
use std::time::Duration;

use async_trait::async_trait;

use crate::analysis::runner::{self, ProcessStatus};
use crate::analysis::tools::check_tool_present;
use crate::analysis::workspace::Workspace;
use crate::analysis::{Analyzer, StepReport, StepStatus};

#[derive(Debug, Clone)]
pub struct CertoraAnalyzer {
    pub enabled: bool,
    pub conf_dir: PathBuf,
    pub timeout: Duration,
}

#[async_trait]
impl Analyzer for CertoraAnalyzer {
    fn name(&self) -> &str {
        "certora"
    }

    async fn run(&self, ws: &Workspace) -> StepReport {
        if !self.enabled {
            return skipped("Certora step disabled (set FORMAL_METHODS_CERTORA_ENABLED=true)");
        }
        if !check_tool_present("certoraRun", &["--version"]).await {
            return skipped("certoraRun not installed; skipping");
        }

        let dir = ws.root().join(&self.conf_dir);
        let mut entries = match tokio::fs::read_dir(&dir).await {
            Ok(e) => e,
            Err(_) => return skipped(&format!("no Certora confs at {}", dir.display())),
        };

        let started = std::time::Instant::now();
        let mut stdout_combined = String::new();
        let mut stderr_combined = String::new();
        let mut passed = 0usize;
        let mut failed = 0usize;

        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("conf") {
                continue;
            }
            let path_str = path.display().to_string();
            let args: Vec<&str> = vec![path_str.as_str()];
            let outcome = runner::run("certoraRun", &args, ws.root(), &[], self.timeout).await;
            stdout_combined.push_str(&format!("== {path_str}\n"));
            stdout_combined.push_str(&outcome.stdout_tail);
            stdout_combined.push('\n');
            if !outcome.stderr_tail.is_empty() {
                stderr_combined.push_str(&format!("== {path_str}\n"));
                stderr_combined.push_str(&outcome.stderr_tail);
                stderr_combined.push('\n');
            }
            match outcome.status {
                ProcessStatus::Exited { code: 0 } => passed += 1,
                _ => failed += 1,
            }
        }

        let status = if failed == 0 && passed > 0 {
            StepStatus::Passed
        } else if passed == 0 && failed == 0 {
            StepStatus::Skipped
        } else {
            StepStatus::Failed
        };

        StepReport {
            name: "certora".into(),
            status,
            duration: started.elapsed(),
            stdout_tail: stdout_combined,
            stderr_tail: stderr_combined,
            summary: format!("certora: {passed} passed, {failed} failed"),
        }
    }
}

fn skipped(msg: &str) -> StepReport {
    StepReport {
        name: "certora".into(),
        status: StepStatus::Skipped,
        duration: Duration::from_secs(0),
        stdout_tail: String::new(),
        stderr_tail: String::new(),
        summary: msg.into(),
    }
}
