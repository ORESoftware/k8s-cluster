use std::collections::BTreeSet;

use gha_clone_server::{build_plan, PlanRequest, PlannerLimits, WorkflowPlan};

const REVISION: &str = "0123456789abcdef0123456789abcdef01234567";
const CHECKOUT_ACTION: &str =
    "actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1";
const SETUP_NODE_ACTION: &str =
    "actions/setup-node@820762786026740c76f36085b0efc47a31fe5020";
const RUST_TOOLCHAIN_ACTION: &str =
    "dtolnay/rust-toolchain@4be7066ada62dd38de10e7b70166bc74ed198c30";
const API_WORKFLOW: &str = include_str!("../fixtures/streempilot-api-ci-mirror.yml");
const WEB_WORKFLOW: &str = include_str!("../fixtures/streempilot-web-ci-mirror.yml");
const INTERFACES_WORKFLOW: &str = include_str!("../fixtures/streempilot-interfaces-ci-mirror.yml");

fn plan(repository: &str, workflow: &str) -> WorkflowPlan {
    build_plan(
        &PlanRequest {
            repository: repository.to_string(),
            revision: REVISION.to_string(),
            workflow_path: ".github/workflows/ci-mirror.yml".to_string(),
            workflow_yaml: workflow.to_string(),
        },
        &PlannerLimits::default(),
    )
    .unwrap_or_else(|errors| panic!("mirror plan should compile: {}", errors.join("\n")))
}

fn job<'a>(plan: &'a WorkflowPlan, id: &str) -> &'a gha_clone_server::JobPlan {
    plan.jobs
        .iter()
        .find(|job| job.id == id)
        .unwrap_or_else(|| panic!("missing job {id:?}"))
}

#[test]
fn api_mirror_compiles_to_one_fixed_rust_profile() {
    let plan = plan("StreemPilot/streempilot-api-server.rs", API_WORKFLOW);
    assert!(plan.immutable_revision);
    assert!(plan.arc_fully_covered);
    assert!(plan.independent_executable);
    assert_eq!(plan.topological_order, ["rust"]);
    let rust = job(&plan, "rust");
    assert_eq!(rust.arc_lane, "sonus-ci");
    assert_eq!(rust.independent_profile.as_deref(), Some("rust-verify"));
    assert!(rust.independent_reasons.is_empty());
}

#[test]
fn web_mirror_keeps_core_rust_verification_independent() {
    let plan = plan("StreemPilot/streempilot-web-server.rs", WEB_WORKFLOW);
    assert!(plan.independent_executable);
    assert_eq!(plan.topological_order, ["rust"]);
    assert_eq!(
        job(&plan, "rust").independent_profile.as_deref(),
        Some("rust-verify")
    );
    assert!(!WEB_WORKFLOW.contains("playwright"));
    assert!(!WEB_WORKFLOW.contains("upload-artifact"));
}

#[test]
fn interfaces_mirror_preserves_contract_before_generated_rust_order() {
    let plan = plan("StreemPilot/streempilot-interfaces", INTERFACES_WORKFLOW);
    assert!(plan.independent_executable);
    assert_eq!(plan.topological_order, ["contracts", "rust-bindings"]);
    assert_eq!(
        job(&plan, "contracts").independent_profile.as_deref(),
        Some("node-verify")
    );
    let rust = job(&plan, "rust-bindings");
    assert_eq!(rust.needs, ["contracts"]);
    assert_eq!(rust.independent_profile.as_deref(), Some("rust-verify"));
}

#[test]
fn every_streempilot_fixture_is_static_secret_free_and_immutable() {
    for workflow in [API_WORKFLOW, WEB_WORKFLOW, INTERFACES_WORKFLOW] {
        for forbidden in [
            "${{",
            "secrets.",
            "permissions:",
            "strategy:",
            "services:",
            "container:",
            "environment:",
            "working-directory:",
            "continue-on-error:",
            "timeout-minutes:",
            "@v1",
            "@v2",
            "@v3",
            "@v4",
            "@main",
            "@master",
            "@stable",
        ] {
            assert!(
                !workflow.contains(forbidden),
                "mirror fixture unexpectedly contains {forbidden:?}"
            );
        }
        assert!(workflow.contains("workflow_dispatch:"));
        assert!(workflow.contains(CHECKOUT_ACTION));
    }
}

#[test]
fn mutable_revisions_can_be_inspected_but_never_dispatched() {
    for (repository, workflow) in [
        ("StreemPilot/streempilot-api-server.rs", API_WORKFLOW),
        ("StreemPilot/streempilot-web-server.rs", WEB_WORKFLOW),
        ("StreemPilot/streempilot-interfaces", INTERFACES_WORKFLOW),
    ] {
        let plan = build_plan(
            &PlanRequest {
                repository: repository.to_string(),
                revision: "main".to_string(),
                workflow_path: ".github/workflows/ci-mirror.yml".to_string(),
                workflow_yaml: workflow.to_string(),
            },
            &PlannerLimits::default(),
        )
        .expect("mutable refs remain plan-only");
        assert!(!plan.immutable_revision);
        assert!(!plan.independent_executable);
        assert!(plan
            .warnings
            .iter()
            .any(|warning| warning.contains("exact 40-hex commit SHA")));
    }
}

#[test]
fn plan_identity_is_stable_and_repository_scoped() {
    let first = plan("StreemPilot/streempilot-api-server.rs", API_WORKFLOW);
    let retry = plan("StreemPilot/streempilot-api-server.rs", API_WORKFLOW);
    let web = plan("StreemPilot/streempilot-web-server.rs", WEB_WORKFLOW);
    assert_eq!(first.plan_id, retry.plan_id);
    assert_ne!(first.plan_id, web.plan_id);
    assert_eq!(first.plan_id.len(), 64);
}

#[test]
fn mirror_jobs_use_only_reviewed_immutable_setup_actions() {
    let allowed = [CHECKOUT_ACTION, SETUP_NODE_ACTION, RUST_TOOLCHAIN_ACTION]
        .into_iter()
        .collect::<BTreeSet<_>>();

    for workflow in [API_WORKFLOW, WEB_WORKFLOW, INTERFACES_WORKFLOW] {
        for line in workflow.lines().map(str::trim) {
            if let Some(action) = line.strip_prefix("- uses: ") {
                assert!(allowed.contains(action), "unreviewed action {action:?}");
                let (_, revision) = action
                    .rsplit_once('@')
                    .unwrap_or_else(|| panic!("action has no revision: {action:?}"));
                assert_eq!(revision.len(), 40, "action is not pinned: {action:?}");
                assert!(
                    revision
                        .bytes()
                        .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()),
                    "action revision is not lowercase hexadecimal: {action:?}"
                );
            }
        }
    }
}

#[test]
fn interfaces_mirror_retains_generated_freshness_typescript_and_rust_evidence() {
    assert!(INTERFACES_WORKFLOW.contains("npm test"));
    assert!(INTERFACES_WORKFLOW.contains("npm run check:typescript"));
    assert!(INTERFACES_WORKFLOW.contains("generated/rust/Cargo.toml"));
    assert!(INTERFACES_WORKFLOW.contains("cargo check"));
    assert!(INTERFACES_WORKFLOW.contains("cargo test"));
    assert!(!INTERFACES_WORKFLOW.contains("dart analyze"));
}
