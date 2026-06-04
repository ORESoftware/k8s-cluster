//! Kani analyzer step.
//!
//! Runs `cargo kani` against a specific package on the PR head commit. When
//! the `cargo-kani` binary is not installed (the usual case on local
//! laptops and on environments not yet configured for FM work), the step
//! gracefully skips with an explanatory message.

use std::path::PathBuf;
use std::time::Duration;

use async_trait::async_trait;

use crate::analysis::runner::{self, ProcessStatus};
use crate::analysis::tools::check_tool_present;
use crate::analysis::workspace::Workspace;
use crate::analysis::{Analyzer, StepReport, StepStatus};

#[derive(Debug, Clone)]
pub struct KaniAnalyzer {
    pub manifest_path: PathBuf,
    pub package: String,
    pub timeout: Duration,
}

#[async_trait]
impl Analyzer for KaniAnalyzer {
    fn name(&self) -> &str {
        "kani"
    }

    async fn run(&self, ws: &Workspace) -> StepReport {
        if !check_tool_present("cargo", &["kani", "--version"]).await {
            return StepReport {
                name: self.name().to_string(),
                status: StepStatus::Skipped,
                duration: Duration::from_secs(0),
                stdout_tail: String::new(),
                stderr_tail: String::new(),
                summary: "cargo-kani not installed; skipping (see README for install steps)".into(),
            };
        }

        let manifest = ws.root().join(&self.manifest_path);
        let manifest_str = manifest.display().to_string();
        let args: Vec<&str> = vec![
            "kani",
            "--manifest-path",
            manifest_str.as_str(),
            "-p",
            self.package.as_str(),
        ];

        let outcome = runner::run(
            "cargo",
            &args,
            ws.root(),
            &[("CARGO_TERM_COLOR", "never")],
            self.timeout,
        )
        .await;

        let status = match outcome.status {
            ProcessStatus::Exited { code: 0 } => StepStatus::Passed,
            _ => StepStatus::Failed,
        };
        let summary = match outcome.status {
            ProcessStatus::Exited { code: 0 } => "all Kani harnesses verified".into(),
            ProcessStatus::Exited { code } => format!("cargo kani exited {code}"),
            ProcessStatus::Signalled => "cargo kani killed by signal".into(),
            ProcessStatus::TimedOut => "cargo kani timed out".into(),
            ProcessStatus::SpawnError => "failed to launch cargo kani".into(),
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
