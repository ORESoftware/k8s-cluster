//! Helpers for detecting whether external prover binaries are installed.
//!
//! Every formal-methods step depends on a third-party CLI (kani, verus,
//! dreal, certoraRun). When the binary is absent on the runner, we want the
//! step to *skip*, not *fail*: missing tooling is not a regression in the
//! code being verified. This module gives each analyzer a single uniform
//! way to ask "is my tool installed?".

use std::time::Duration;

use crate::analysis::runner::{self, ProcessStatus};

/// Runs `<program> <args...>` with a 10s deadline and reports whether it
/// terminated successfully. Used as a presence check: e.g. `cargo kani --version`,
/// `verus --version`, `dreal --version`.
pub async fn check_tool_present(program: &str, args: &[&str]) -> bool {
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let outcome = runner::run(program, args, &cwd, &[], Duration::from_secs(10)).await;
    matches!(outcome.status, ProcessStatus::Exited { code: 0 })
}
