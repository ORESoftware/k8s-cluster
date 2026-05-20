//! Analysis pipeline.
//!
//! The pipeline takes a [`workspace::Workspace`] (a freshly checked-out PR
//! head commit on disk) and runs a sequence of [`Analyzer`]s against it.
//! Each analyzer produces a [`StepReport`]; the aggregated [`Report`] is what
//! gets reported back to GitHub.
//!
//! The current default pipeline runs `cargo check` and `cargo test` on the
//! contract crate. Future formal-methods steps (Kani, Verus, raw Z3, ...)
//! plug in as additional [`Analyzer`] implementations registered in
//! [`pipeline::Pipeline::from_config`].

pub mod cargo;
pub mod certora;
pub mod dreal;
pub mod kani;
pub mod pipeline;
pub mod runner;
pub mod tools;
pub mod verus;
pub mod workspace;

use std::time::Duration;

use async_trait::async_trait;
use serde::Serialize;

use crate::analysis::workspace::Workspace;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StepStatus {
    Passed,
    Failed,
    Skipped,
}

#[derive(Debug, Clone, Serialize)]
pub struct StepReport {
    pub name: String,
    pub status: StepStatus,
    pub duration: Duration,
    /// Trimmed tail of process stdout, useful for posting back in a comment.
    pub stdout_tail: String,
    /// Trimmed tail of process stderr.
    pub stderr_tail: String,
    /// First-line human summary, suitable for a check-run description.
    pub summary: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Report {
    pub steps: Vec<StepReport>,
}

impl Report {
    pub fn overall(&self) -> StepStatus {
        if self.steps.iter().any(|s| s.status == StepStatus::Failed) {
            StepStatus::Failed
        } else if self.steps.iter().all(|s| s.status == StepStatus::Skipped) {
            StepStatus::Skipped
        } else {
            StepStatus::Passed
        }
    }

    pub fn description(&self) -> String {
        match self.overall() {
            StepStatus::Passed => {
                let passed = self
                    .steps
                    .iter()
                    .filter(|s| s.status == StepStatus::Passed)
                    .count();
                format!("{passed} step(s) passed")
            }
            StepStatus::Failed => self
                .steps
                .iter()
                .find(|s| s.status == StepStatus::Failed)
                .map(|s| format!("{} failed: {}", s.name, s.summary))
                .unwrap_or_else(|| "analysis failed".into()),
            StepStatus::Skipped => "no analyzers ran".into(),
        }
    }
}

#[async_trait]
pub trait Analyzer: Send + Sync + std::fmt::Debug {
    fn name(&self) -> &str;
    async fn run(&self, ws: &Workspace) -> StepReport;
}
