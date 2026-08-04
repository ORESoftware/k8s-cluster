use gha_clone_server::{build_plan, PlanRequest, PlannerLimits, WorkflowPlan};

const REPOSITORY: &str = "3FA-app/3fa-interfaces";
const REVISION: &str = "baea54bad288a36e36f6f484c1b5f2313bddfba8";
const WORKFLOW_PATH: &str = ".github/workflows/gha-clone-contracts.yml";
const WORKFLOW: &str = include_str!("../fixtures/threefa-interfaces-contracts.yml");

fn compile(workflow: &str) -> WorkflowPlan {
    build_plan(
        &PlanRequest {
            repository: REPOSITORY.to_string(),
            revision: REVISION.to_string(),
            workflow_path: WORKFLOW_PATH.to_string(),
            workflow_yaml: workflow.to_string(),
        },
        &PlannerLimits::default(),
    )
    .unwrap_or_else(|errors| panic!("3FA workflow should compile: {}", errors.join("\n")))
}

fn job<'a>(plan: &'a WorkflowPlan, id: &str) -> &'a gha_clone_server::JobPlan {
    plan.jobs
        .iter()
        .find(|job| job.id == id)
        .unwrap_or_else(|| panic!("missing job {id:?}"))
}

#[test]
fn exact_threefa_fixture_compiles_to_node_then_generated_rust() {
    let plan = compile(WORKFLOW);
    assert!(plan.immutable_revision);
    assert!(plan.arc_fully_covered);
    assert!(plan.independent_executable);
    assert_eq!(plan.repository, REPOSITORY);
    assert_eq!(plan.revision, REVISION);
    assert_eq!(plan.workflow_path, WORKFLOW_PATH);
    assert_eq!(plan.topological_order, ["node_contracts", "generated_rust"]);

    let node = job(&plan, "node_contracts");
    assert_eq!(node.arc_lane, "sonus-ci");
    assert_eq!(
        node.independent_profile.as_deref(),
        Some("node-hardened-test")
    );
    assert!(node.independent_reasons.is_empty());

    let rust = job(&plan, "generated_rust");
    assert_eq!(rust.needs, ["node_contracts"]);
    assert_eq!(rust.arc_lane, "sonus-ci");
    assert_eq!(
        rust.independent_profile.as_deref(),
        Some("rust-generated-verify")
    );
    assert!(rust.independent_reasons.is_empty());
}

#[test]
fn generated_rust_sequence_is_exact_ordered_and_non_extensible() {
    let reordered = WORKFLOW.replacen(
        "          cargo clippy --locked --manifest-path generated/rust/Cargo.toml --all-targets -- -D warnings\n          cargo test --locked --manifest-path generated/rust/Cargo.toml --all-targets\n",
        "          cargo test --locked --manifest-path generated/rust/Cargo.toml --all-targets\n          cargo clippy --locked --manifest-path generated/rust/Cargo.toml --all-targets -- -D warnings\n",
        1,
    );
    let extra = WORKFLOW.replacen(
        "          cargo test --locked --manifest-path generated/rust/Cargo.toml --all-targets\n",
        "          cargo test --locked --manifest-path generated/rust/Cargo.toml --all-targets\n          cargo publish --manifest-path generated/rust/Cargo.toml\n",
        1,
    );

    for (label, workflow) in [("reordered", reordered), ("extra", extra)] {
        let plan = compile(&workflow);
        assert!(
            !plan.independent_executable,
            "{label} workflow was executable"
        );
        let rust = job(&plan, "generated_rust");
        assert!(!rust.independent_supported);
        assert_eq!(rust.independent_profile, None);
        assert!(rust
            .independent_reasons
            .iter()
            .any(|reason| reason.contains("exact reviewed command sequence")));
    }
}

#[test]
fn hardened_node_sequence_is_exact_and_non_extensible() {
    let extra = WORKFLOW.replacen(
        "          npm test\n",
        "          npm test\n          npm audit --audit-level=high\n",
        1,
    );
    let plan = compile(&extra);
    assert!(!plan.independent_executable);
    let node = job(&plan, "node_contracts");
    assert!(!node.independent_supported);
    assert_eq!(node.independent_profile, None);
    assert!(node
        .independent_reasons
        .iter()
        .any(|reason| reason.contains("exact reviewed command sequence")));
}

#[test]
fn setup_action_sequence_is_exact_pinned_and_non_extensible() {
    let mutable_node = WORKFLOW.replacen(
        "actions/setup-node@820762786026740c76f36085b0efc47a31fe5020",
        "actions/setup-node@main",
        1,
    );
    let mutable_rust = WORKFLOW.replacen(
        "dtolnay/rust-toolchain@4be7066ada62dd38de10e7b70166bc74ed198c30",
        "dtolnay/rust-toolchain@stable",
        1,
    );
    let extra_action = WORKFLOW.replacen(
        "      - run: |\n          npm ci --ignore-scripts",
        "      - uses: owner/extra-action@0123456789abcdef0123456789abcdef01234567\n      - run: |\n          npm ci --ignore-scripts",
        1,
    );

    for (job_id, workflow) in [
        ("node_contracts", mutable_node),
        ("generated_rust", mutable_rust),
        ("node_contracts", extra_action),
    ] {
        let plan = compile(&workflow);
        assert!(!plan.independent_executable);
        let planned_job = job(&plan, job_id);
        assert!(!planned_job.independent_supported);
        assert_eq!(planned_job.independent_profile, None);
        assert!(planned_job
            .independent_reasons
            .iter()
            .any(|reason| { reason.contains("exact reviewed pinned action sequence") }));
    }
}

#[test]
fn mutable_revision_can_be_classified_but_not_executed() {
    let branch_plan = build_plan(
        &PlanRequest {
            repository: REPOSITORY.to_string(),
            revision: "main".to_string(),
            workflow_path: WORKFLOW_PATH.to_string(),
            workflow_yaml: WORKFLOW.to_string(),
        },
        &PlannerLimits::default(),
    )
    .expect("mutable revision can be classified but not executed");
    assert!(!branch_plan.immutable_revision);
    assert!(!branch_plan.independent_executable);
    assert!(branch_plan
        .warnings
        .iter()
        .any(|warning| warning.contains("exact 40-hex commit SHA")));
}

#[test]
fn fixture_is_static_secret_free_and_uses_only_pinned_setup_actions() {
    for forbidden in [
        "${{",
        "secrets.",
        "permissions:",
        "environment:",
        "services:",
        "container:",
        "strategy:",
        "@main",
        "@master",
        "@stable",
        "cargo publish",
        "npm install",
    ] {
        assert!(
            !WORKFLOW.contains(forbidden),
            "fixture contains {forbidden:?}"
        );
    }
    for action in [
        "actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1",
        "actions/setup-node@820762786026740c76f36085b0efc47a31fe5020",
        "dtolnay/rust-toolchain@4be7066ada62dd38de10e7b70166bc74ed198c30",
    ] {
        assert!(WORKFLOW.contains(action), "fixture is missing {action}");
    }
    assert!(WORKFLOW.contains("npm ci --ignore-scripts"));
    assert!(WORKFLOW.contains("generated/rust/Cargo.toml"));
}
