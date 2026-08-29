//! dReal analyzer step.
//!
//! Runs `dreal --precision <ε> <file.smt2>` over every SMT-LIB file under
//! `proofs/dreal/`. dReal exits 0 when the query is `unsat` (property holds
//! up to δ) and non-zero on `delta-sat` (potential counterexample). Skips
//! when `dreal` is not on `PATH`.

use std::path::PathBuf;
use std::time::Duration;

use async_trait::async_trait;
use tracing::warn;

use crate::analysis::runner::{self, ProcessStatus};
use crate::analysis::tools::check_tool_present;
use crate::analysis::workspace::Workspace;
use crate::analysis::{Analyzer, StepReport, StepStatus};

const MAX_QUERY_FILES: usize = 128;

#[derive(Debug, Clone)]
pub struct DRealAnalyzer {
    pub queries_dir: PathBuf,
    pub precision: f64,
    pub timeout: Duration,
}

#[async_trait]
impl Analyzer for DRealAnalyzer {
    fn name(&self) -> &str {
        "dreal"
    }

    async fn run(&self, ws: &Workspace) -> StepReport {
        if !check_tool_present("dreal", &["--version"]).await {
            return StepReport {
                name: self.name().to_string(),
                status: StepStatus::Skipped,
                duration: Duration::from_secs(0),
                stdout_tail: String::new(),
                stderr_tail: String::new(),
                summary: "dreal not installed; skipping (see proofs/dreal/README.md)".into(),
            };
        }

        let dir = ws.root().join(&self.queries_dir);
        let mut entries = match tokio::fs::read_dir(&dir).await {
            Ok(e) => e,
            Err(err) => {
                return StepReport {
                    name: self.name().to_string(),
                    status: StepStatus::Skipped,
                    duration: Duration::from_secs(0),
                    stdout_tail: String::new(),
                    stderr_tail: format!("{err}"),
                    summary: format!("dreal queries dir not found: {}", dir.display()),
                };
            }
        };

        let precision = format!("{}", self.precision);
        let started = std::time::Instant::now();
        let mut combined_stdout = String::new();
        let mut combined_stderr = String::new();
        let mut total_passed = 0usize;
        let mut total_failed = 0usize;
        let mut files_seen = 0usize;

        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("smt2") {
                continue;
            }
            // Skip the shared axiom file (it's not a self-contained query).
            if path.file_name().and_then(|n| n.to_str()) == Some("axioms_exp_log.smt2") {
                continue;
            }
            files_seen += 1;
            if files_seen > MAX_QUERY_FILES {
                total_failed += 1;
                runner::append_aggregated_tail(
                    &mut combined_stderr,
                    &format!("dreal query limit exceeded (max {MAX_QUERY_FILES})\n"),
                );
                break;
            }
            if !matches!(entry.file_type().await, Ok(file_type) if file_type.is_file() && !file_type.is_symlink())
            {
                total_failed += 1;
                runner::append_aggregated_tail(
                    &mut combined_stderr,
                    &format!("refusing non-regular dreal query: {}\n", path.display()),
                );
                continue;
            }
            let path_str = path.display().to_string();
            let args: Vec<&str> = vec!["--precision", precision.as_str(), path_str.as_str()];
            let remaining = self.timeout.saturating_sub(started.elapsed());
            if remaining.is_zero() {
                total_failed += 1;
                runner::append_aggregated_tail(
                    &mut combined_stderr,
                    "dreal analyzer exhausted its total time budget\n",
                );
                break;
            }

            let outcome = runner::run("dreal", &args, ws.root(), &[], remaining).await;

            runner::append_aggregated_tail(
                &mut combined_stdout,
                &format!("== {path_str}\n{}\n", outcome.stdout_tail),
            );

            if !outcome.stderr_tail.is_empty() {
                runner::append_aggregated_tail(
                    &mut combined_stderr,
                    &format!("== {path_str}\n{}\n", outcome.stderr_tail),
                );
            }

            // dReal returns 0 on `unsat`/`sat` answers; the "result" is in
            // stdout. We parse: `unsat` ⇒ property holds; `delta-sat` ⇒
            // potential counterexample (treat as failure).
            let stdout_lower = outcome.stdout_tail.to_ascii_lowercase();
            let is_unsat = stdout_lower.lines().any(|l| l.trim() == "unsat");
            let is_delta_sat = stdout_lower.contains("delta-sat");

            match (outcome.status, is_unsat, is_delta_sat) {
                (ProcessStatus::Exited { code: 0 }, true, false) => total_passed += 1,
                (ProcessStatus::Exited { code: 0 }, false, true) => {
                    warn!(file = %path_str, "dreal returned delta-sat (potential counterexample)");
                    total_failed += 1;
                }
                _ => total_failed += 1,
            }
        }

        let status = if total_failed == 0 && total_passed > 0 {
            StepStatus::Passed
        } else if total_passed == 0 && total_failed == 0 {
            StepStatus::Skipped
        } else {
            StepStatus::Failed
        };

        let summary = format!("dreal: {total_passed} passed, {total_failed} failed");

        StepReport {
            name: self.name().to_string(),
            status,
            duration: started.elapsed(),
            stdout_tail: combined_stdout,
            stderr_tail: combined_stderr,
            summary,
        }
    }
}
