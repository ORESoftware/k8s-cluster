use gha_clone_server::{build_plan, PlanRequest, PlannerLimits};

const REPOSITORY: &str = "messaging-intel/msgint-connectors";
const REVISION: &str = "a43e11cd7610806470c0af95f4cdbe3e19b143bb";
const WORKFLOW_PATH: &str = ".github/workflows/gha-clone-operator-config.yml";
const REVIEWED: &str = include_str!("fixtures/msgint-operator-config.yml");

fn request(repository: &str, revision: &str, workflow_path: &str, yaml: &str) -> PlanRequest {
    PlanRequest {
        repository: repository.to_string(),
        revision: revision.to_string(),
        workflow_path: workflow_path.to_string(),
        workflow_yaml: yaml.to_string(),
    }
}

fn reviewed(yaml: &str) -> Result<gha_clone_server::WorkflowPlan, Vec<String>> {
    build_plan(
        &request(REPOSITORY, REVISION, WORKFLOW_PATH, yaml),
        &PlannerLimits::default(),
    )
}

fn replace_once(old: &str, new: &str) -> String {
    assert!(REVIEWED.contains(old), "missing mutation anchor {old:?}");
    REVIEWED.replacen(old, new, 1)
}

#[test]
fn exact_reviewed_identity_maps_only_the_two_privileged_profiles() {
    let plan = reviewed(REVIEWED).expect("reviewed workflow should execute");
    assert!(plan.independent_executable);
    assert_eq!(
        plan.topological_order,
        ["operator_config", "repository_tests"]
    );
    assert_eq!(plan.jobs.len(), 2);
    assert_eq!(
        plan.jobs[0].independent_profile.as_deref(),
        Some("node-hardened-verify")
    );
    assert_eq!(
        plan.jobs[1].independent_profile.as_deref(),
        Some("node-hardened-test")
    );
}

#[test]
fn reserved_repository_path_and_revision_mismatches_are_terminal() {
    for request in [
        request(
            REPOSITORY,
            REVISION,
            ".github/workflows/lookalike.yml",
            REVIEWED,
        ),
        request(
            "lookalike/msgint-connectors",
            REVISION,
            WORKFLOW_PATH,
            REVIEWED,
        ),
        request(
            REPOSITORY,
            "0000000000000000000000000000000000000000",
            WORKFLOW_PATH,
            REVIEWED,
        ),
    ] {
        let plan = build_plan(&request, &PlannerLimits::default())
            .expect("reserved identity mismatch must produce a terminal plan");
        assert!(!plan.independent_executable);
        assert!(plan
            .jobs
            .iter()
            .all(|job| job.independent_profile.is_none()));
        let error = plan
            .jobs
            .iter()
            .flat_map(|job| job.independent_reasons.iter())
            .cloned()
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            error.contains("reserved Messaging Intel")
                || error.contains("requires reviewed revision"),
            "unexpected rejection: {error}"
        );
    }
}

#[test]
fn immutable_action_and_input_lookalikes_never_fall_back_to_generic_node() {
    for yaml in [
        replace_once(
            "actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1",
            "actions/checkout@0000000000000000000000000000000000000000",
        ),
        replace_once("persist-credentials: false", "persist-credentials: true"),
        replace_once("node-version: \"22.23.1\"", "node-version: \"22\""),
        replace_once(
            "          cache: npm\n",
            "          cache: npm\n          registry-url: https://evil.invalid\n",
        ),
        replace_once(
            "      - run: |\n          npm ci --ignore-scripts\n",
            "      - uses: actions/setup-python@8a5f4f9f4d7e4c9f5eead5d7f7e770585a6e9430\n      - run: |\n          npm ci --ignore-scripts\n",
        ),
    ] {
        let plan = reviewed(&yaml)
            .expect("lookalike action/input workflow must produce a terminal plan");
        assert!(!plan.independent_executable);
        assert!(plan
            .jobs
            .iter()
            .all(|job| job.independent_profile.is_none()));
        let error = plan
            .jobs
            .iter()
            .flat_map(|job| job.independent_reasons.iter())
            .cloned()
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            error.contains("reviewed contract")
                || error.contains("exact 40-hex commit SHA")
                || error.contains("exactly three reviewed steps"),
            "unexpected rejection: {error}"
        );
    }
}

#[test]
fn command_dag_environment_and_secret_lookalikes_are_rejected() {
    for yaml in [
        replace_once(
            "          npm audit --audit-level=high\n",
            "          npm audit --audit-level=high\n          npm publish\n",
        ),
        replace_once(
            "          npm run check\n          npm run test:operator-config\n",
            "          npm run test:operator-config\n          npm run check\n",
        ),
        replace_once(
            "    runs-on: ubuntu-latest\n    steps:\n",
            "    runs-on: ubuntu-latest\n    env:\n      NODE_ENV: test\n    steps:\n",
        ),
        replace_once(
            "          cache: npm\n",
            "          cache: npm\n          token: ${{ secrets['PROD_TOKEN'] }}\n",
        ),
        replace_once(
            "  repository_tests:\n    needs: operator_config\n",
            "  repository_tests:\n    needs: missing_job\n",
        ),
        replace_once(
            "          npm run test:operator-config\n",
            "          echo npm run test:operator-config\n",
        ),
    ] {
        match reviewed(&yaml) {
            Ok(plan) => assert!(
                !plan.independent_executable
                    && plan
                        .jobs
                        .iter()
                        .all(|job| job.independent_profile.is_none()),
                "lookalike workflow unexpectedly executed:\n{yaml}"
            ),
            Err(errors) => assert!(
                !errors.is_empty(),
                "structurally invalid lookalike must explain its rejection"
            ),
        }
    }
}

#[test]
fn unrelated_repository_and_path_still_use_the_generic_planner() {
    let generic = r#"jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - run: cargo test
"#;
    let plan = build_plan(
        &request(
            "sonus-auris/sonus-auris-interfaces",
            "0123456789abcdef0123456789abcdef01234567",
            ".github/workflows/ci.yml",
            generic,
        ),
        &PlannerLimits::default(),
    )
    .expect("generic workflow should still plan");
    assert!(plan.independent_executable);
    assert_eq!(
        plan.jobs[0].independent_profile.as_deref(),
        Some("rust-verify")
    );
}
