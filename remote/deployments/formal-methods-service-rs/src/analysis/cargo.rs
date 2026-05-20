//! Cargo-based analyzers used until the formal-methods passes land in
//! `apps/contract-service`. They establish the baseline that the contract
//! Rust workspace at the PR head commit compiles and its existing unit tests
//! still pass.
//!
//! Each analyzer is intentionally narrow so we can plug additional formal
//! steps (Kani, Verus, raw Z3) in alongside without rewriting this code.

use std::path::PathBuf;
use std::time::Duration;

use async_trait::async_trait;

use crate::analysis::runner::{self, ProcessStatus};
use crate::analysis::workspace::Workspace;
use crate::analysis::{Analyzer, StepReport, StepStatus};

#[derive(Debug, Clone)]
pub struct CargoCheckAnalyzer {
    pub manifest_path: PathBuf,
    pub timeout: Duration,
}

#[async_trait]
impl Analyzer for CargoCheckAnalyzer {
    fn name(&self) -> &str {
        "cargo-check"
    }

    async fn run(&self, ws: &Workspace) -> StepReport {
        let manifest = ws.root().join(&self.manifest_path);
        let manifest_str = manifest.display().to_string();
        let args = vec![
            "check",
            "--manifest-path",
            manifest_str.as_str(),
            "--all-targets",
            "--locked",
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
            ProcessStatus::Exited { code: 0 } => "cargo check succeeded".into(),
            ProcessStatus::Exited { code } => format!("cargo check exited {code}"),
            ProcessStatus::Signalled => "cargo check killed by signal".into(),
            ProcessStatus::TimedOut => "cargo check timed out".into(),
            ProcessStatus::SpawnError => "failed to launch cargo".into(),
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

#[derive(Debug, Clone)]
pub struct CargoTestAnalyzer {
    pub manifest_path: PathBuf,
    pub package: Option<String>,
    pub features: Option<String>,
    pub timeout: Duration,
}

#[async_trait]
impl Analyzer for CargoTestAnalyzer {
    fn name(&self) -> &str {
        "cargo-test"
    }

    async fn run(&self, ws: &Workspace) -> StepReport {
        let manifest = ws.root().join(&self.manifest_path);
        let manifest_str = manifest.display().to_string();
        let mut args: Vec<String> = vec![
            "test".into(),
            "--manifest-path".into(),
            manifest_str,
            "--locked".into(),
        ];
        if let Some(pkg) = &self.package {
            args.push("-p".into());
            args.push(pkg.clone());
        }
        if let Some(features) = &self.features {
            args.push("--features".into());
            args.push(features.clone());
        }
        let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();

        let outcome = runner::run(
            "cargo",
            &arg_refs,
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
            ProcessStatus::Exited { code: 0 } => "cargo test succeeded".into(),
            ProcessStatus::Exited { code } => format!("cargo test exited {code}"),
            ProcessStatus::Signalled => "cargo test killed by signal".into(),
            ProcessStatus::TimedOut => "cargo test timed out".into(),
            ProcessStatus::SpawnError => "failed to launch cargo".into(),
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

/// Property-based test step. Today proptest runs as part of `cargo test`;
/// this analyzer scopes the run to the proptest integration test target so
/// it can be reported independently from the rest of the unit tests.
#[derive(Debug, Clone)]
pub struct ProptestAnalyzer {
    pub manifest_path: PathBuf,
    pub package: String,
    pub test_target: String,
    pub timeout: Duration,
}

#[async_trait]
impl Analyzer for ProptestAnalyzer {
    fn name(&self) -> &str {
        "proptest"
    }

    async fn run(&self, ws: &Workspace) -> StepReport {
        let manifest = ws.root().join(&self.manifest_path);
        let manifest_str = manifest.display().to_string();
        let args: Vec<&str> = vec![
            "test",
            "--manifest-path",
            manifest_str.as_str(),
            "-p",
            self.package.as_str(),
            "--test",
            self.test_target.as_str(),
            "--locked",
        ];

        let outcome = runner::run(
            "cargo",
            &args,
            ws.root(),
            &[
                ("CARGO_TERM_COLOR", "never"),
                // proptest writes regressions next to the test source. We
                // don't want that in CI; disable persistence.
                ("PROPTEST_DISABLE_FAILURE_PERSISTENCE", "1"),
            ],
            self.timeout,
        )
        .await;

        let status = match outcome.status {
            ProcessStatus::Exited { code: 0 } => StepStatus::Passed,
            _ => StepStatus::Failed,
        };

        let summary = match outcome.status {
            ProcessStatus::Exited { code: 0 } => "proptest properties pass".into(),
            ProcessStatus::Exited { code } => format!("proptest exited {code}"),
            ProcessStatus::Signalled => "proptest killed by signal".into(),
            ProcessStatus::TimedOut => "proptest timed out".into(),
            ProcessStatus::SpawnError => "failed to launch cargo".into(),
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
