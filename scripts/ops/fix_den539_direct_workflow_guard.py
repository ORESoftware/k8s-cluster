#!/usr/bin/env python3
"""Enforce configured repository/workflow pairs on direct plan and run APIs."""

from __future__ import annotations

import subprocess
from pathlib import Path

PATH = Path("remote/deployments/gha-clone-server-rs/src/main.rs")
EXPECTED_BLOB = "fd7d8adfe717f2f6cfaa2d6e89f8c4bea2345477"


def replace_once(source: str, old: str, new: str, label: str) -> str:
    if source.count(old) != 1:
        raise SystemExit(f"{label} anchor was not unique")
    return source.replace(old, new, 1)


def main() -> None:
    source = PATH.read_text(encoding="utf-8")
    if "fn require_allowed_workflow(" in source:
        print("direct workflow-path guard is already installed")
        return

    observed = subprocess.check_output(
        ["git", "hash-object", str(PATH)], text=True
    ).strip()
    if observed != EXPECTED_BLOB:
        raise SystemExit(
            f"refusing drifted {PATH}: expected {EXPECTED_BLOB}, observed {observed}"
        )

    plan_anchor = '''    if let Err(response) = require_allowed_repository(&request.repository, &state) {
        return response;
    }
    match build_plan(&request, &state.config.limits) {
'''
    plan_replacement = '''    if let Err(response) = require_allowed_repository(&request.repository, &state) {
        return response;
    }
    if let Err(response) =
        require_allowed_workflow(&request.repository, &request.workflow_path, &state)
    {
        return response;
    }
    match build_plan(&request, &state.config.limits) {
'''
    source = replace_once(source, plan_anchor, plan_replacement, "plan workflow guard")

    run_anchor = '''    if let Err(response) = require_allowed_repository(&request.plan.repository, &state) {
        return response;
    }
    if !state.config.execution_enabled {
'''
    run_replacement = '''    if let Err(response) = require_allowed_repository(&request.plan.repository, &state) {
        return response;
    }
    if let Err(response) = require_allowed_workflow(
        &request.plan.repository,
        &request.plan.workflow_path,
        &state,
    ) {
        return response;
    }
    if !state.config.execution_enabled {
'''
    source = replace_once(source, run_anchor, run_replacement, "run workflow guard")

    helper_anchor = '''fn digest_eq(left: &str, right: &str) -> bool {
'''
    helper = '''#[allow(clippy::result_large_err)] // Preserve direct Axum rejection responses without heap boxing.
fn require_allowed_workflow(
    repository: &str,
    workflow_path: &str,
    state: &AppState,
) -> Result<(), Response> {
    let Some(configured_paths) = state.config.workflow_rules.get(repository) else {
        // Preserve existing direct API behavior for legacy repositories that
        // have a repository allowlist entry but no explicit workflow rules.
        return Ok(());
    };
    if configured_paths.iter().any(|path| path == workflow_path) {
        Ok(())
    } else {
        Err((
            StatusCode::FORBIDDEN,
            Json(json!({
                "error": "workflow path is not allowlisted",
                "repository": repository,
                "workflowPath": workflow_path
            })),
        )
            .into_response())
    }
}

'''
    source = replace_once(source, helper_anchor, helper + helper_anchor, "workflow guard helper")
    PATH.write_text(source, encoding="utf-8")
    print("configured workflow-path rules now guard direct plan and run endpoints")


if __name__ == "__main__":
    main()
