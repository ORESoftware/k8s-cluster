use serde::Serialize;
use std::collections::BTreeSet;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Error,
    Warning,
    Info,
}

#[derive(Debug, Clone, Serialize)]
pub struct Diagnostic {
    pub code: &'static str,
    pub severity: Severity,
    pub message: String,
    pub resolution: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionSafety {
    ReadOnly,
    MutatesProject,
    InstallsSoftware,
}

#[derive(Debug, Clone, Serialize)]
pub struct RecommendedAction {
    pub id: &'static str,
    pub title: String,
    pub command: Option<String>,
    pub rationale: String,
    pub safety: ActionSafety,
}

#[derive(Debug, Clone, Serialize)]
pub struct CliState {
    pub available: bool,
    pub version: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct PackageState {
    pub manifest_path: Option<String>,
    pub lock_path: Option<String>,
    pub modules_path: Option<String>,
    pub org: Option<String>,
    pub name: Option<String>,
    pub version: Option<String>,
    pub direct_dependency_count: usize,
    pub manifest_valid: bool,
    pub lock_valid: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Summary {
    pub errors: usize,
    pub warnings: usize,
    pub info: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProjectReport {
    pub root: String,
    pub cli: CliState,
    pub package: PackageState,
    pub summary: Summary,
    pub diagnostics: Vec<Diagnostic>,
    pub recommended_actions: Vec<RecommendedAction>,
}

pub fn diagnostic(
    code: &'static str,
    severity: Severity,
    message: impl Into<String>,
    resolution: impl Into<String>,
) -> Diagnostic {
    Diagnostic {
        code,
        severity,
        message: message.into(),
        resolution: resolution.into(),
    }
}

pub fn summarize(diagnostics: &[Diagnostic]) -> Summary {
    let mut summary = Summary {
        errors: 0,
        warnings: 0,
        info: 0,
    };

    for diagnostic in diagnostics {
        match diagnostic.severity {
            Severity::Error => summary.errors += 1,
            Severity::Warning => summary.warnings += 1,
            Severity::Info => summary.info += 1,
        }
    }
    summary
}

pub fn actions_for(diagnostics: &[Diagnostic]) -> Vec<RecommendedAction> {
    let codes: BTreeSet<_> = diagnostics
        .iter()
        .map(|diagnostic| diagnostic.code)
        .collect();
    let mut actions = Vec::new();

    if codes.contains("ZED001") {
        actions.push(action(
            "install-zed-cli",
            "Install the Zed CLI",
            Some("curl -fsSL https://zpkg.tech/install.sh | bash"),
            "Air cannot ask zed-cli for authoritative package operations until the executable is available.",
            ActionSafety::InstallsSoftware,
        ));
    }
    if codes.contains("ZED002") && !codes.contains("ZED004") {
        actions.push(action(
            "initialize-zed-package",
            "Initialize a Zed manifest",
            Some("zed init"),
            "Creates the package manifest that records package identity and dependency intent.",
            ActionSafety::MutatesProject,
        ));
    }
    if codes.contains("ZED003") || codes.contains("ZED008") {
        actions.push(action(
            "resolve-dependencies",
            "Resolve Zed dependencies",
            Some("zed install"),
            "Reconciles the manifest with the lockfile and materialized dependency tree.",
            ActionSafety::MutatesProject,
        ));
    }
    if codes.contains("ZED004") {
        actions.push(action(
            "restore-lock-only",
            "Restore an intentional lock-only workspace",
            Some("zed install --frozen --do-not-write-new-manifest"),
            "Uses exact locked artifacts without inventing direct-dependency intent.",
            ActionSafety::MutatesProject,
        ));
    }
    if codes.contains("ZED007") {
        actions.push(action(
            "materialize-locked-packages",
            "Materialize exact locked packages",
            Some("zed install --frozen"),
            "Restores `zed_modules/` without changing selected versions.",
            ActionSafety::MutatesProject,
        ));
    }
    if codes.contains("ZED005") || codes.contains("ZED006") || codes.contains("ZED009") {
        actions.push(action(
            "repair-metadata",
            "Repair package metadata",
            None,
            "The manifest or lockfile needs human review before an automated package command is safe.",
            ActionSafety::ReadOnly,
        ));
    }

    actions.push(action(
        "inspect-store",
        "Inspect the local Zed store",
        Some("zed store status"),
        "Provides read-only cache and store context useful during package troubleshooting.",
        ActionSafety::ReadOnly,
    ));
    actions
}

fn action(
    id: &'static str,
    title: impl Into<String>,
    command: Option<&str>,
    rationale: impl Into<String>,
    safety: ActionSafety,
) -> RecommendedAction {
    RecommendedAction {
        id,
        title: title.into(),
        command: command.map(ToOwned::to_owned),
        rationale: rationale.into(),
        safety,
    }
}

pub fn explain_code(code: &str) -> String {
    match code.to_ascii_uppercase().as_str() {
        "ZED000" => "No obvious package-state problem was detected. Continue to use frozen installs in CI.".into(),
        "ZED001" => "The zed CLI is missing from PATH. Install it and restart Air so the MCP subprocess receives the new environment.".into(),
        "ZED002" => "No .zpkg.toml exists. Initialize an authored package or adopt Zed through a dependency-bearing install.".into(),
        "ZED003" => "A manifest exists without a lockfile. Run zed install to resolve and record exact artifacts.".into(),
        "ZED004" => "A lockfile exists without a manifest. Restore the manifest when possible; otherwise use the explicit frozen lock-only workflow.".into(),
        "ZED005" => "The manifest is invalid TOML and must be repaired before dependency resolution is safe.".into(),
        "ZED006" => "The lockfile is invalid TOML. Restore it from version control or regenerate it from a valid manifest.".into(),
        "ZED007" => "Locked packages are not materialized. Run zed install --frozen to restore exact versions.".into(),
        "ZED008" => "The manifest is newer than the lockfile. Re-resolve and review the resulting lockfile diff.".into(),
        "ZED009" => "The package identity is incomplete. Add org, name, and version before publishing.".into(),
        _ => format!("Unknown diagnostic code `{code}`. Run zed_project_status to obtain current supported codes."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lock_only_actions_do_not_initialize_a_new_manifest() {
        let diagnostics = vec![
            diagnostic("ZED002", Severity::Warning, "missing", "restore"),
            diagnostic("ZED004", Severity::Warning, "lock only", "restore"),
        ];
        let actions = actions_for(&diagnostics);

        assert!(
            actions
                .iter()
                .any(|action| action.id == "restore-lock-only")
        );
        assert!(
            !actions
                .iter()
                .any(|action| action.id == "initialize-zed-package")
        );
    }

    #[test]
    fn explains_known_code() {
        assert!(explain_code("zed007").contains("--frozen"));
    }
}
