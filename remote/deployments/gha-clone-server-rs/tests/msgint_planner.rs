use gha_clone_server::{build_plan, PlanRequest, PlannerLimits, WorkflowPlan};

const REPOSITORY: &str = "messaging-intel/msgint-connectors";
const REVISION: &str = "a9cc977d78347ec0efdbe8e6766967f80d425882";
const WORKFLOW_PATH: &str = ".github/workflows/gha-clone-operator-config.yml";
const WORKFLOW: &str = include_str!("../fixtures/msgint-operator-config.yml");

fn request(repository: &str, revision: &str, path: &str, workflow: &str) -> PlanRequest {
    PlanRequest {
        repository: repository.to_string(),
        revision: revision.to_string(),
        workflow_path: path.to_string(),
        workflow_yaml: workflow.to_string(),
    }
}

fn compile(workflow: &str) -> WorkflowPlan {
    build_plan(
        &request(REPOSITORY, REVISION, WORKFLOW_PATH, workflow),
        &PlannerLimits::default(),
    )
    .unwrap_or_else(|errors| {
        panic!(
            "Messaging Intel workflow should parse: {}",
            errors.join("\n")
        )
    })
}

fn job<'a>(plan: &'a WorkflowPlan, id: &str) -> &'a gha_clone_server::JobPlan {
    plan.jobs
        .iter()
        .find(|job| job.id == id)
        .unwrap_or_else(|| panic!("missing job {id:?}"))
}

#[test]
fn exact_fixture_compiles_to_hardened_operator_then_repository_profiles() {
    let plan = compile(WORKFLOW);
    assert!(plan.immutable_revision);
    assert!(plan.independent_executable);
    assert_eq!(plan.repository, REPOSITORY);
    assert_eq!(plan.revision, REVISION);
    assert_eq!(plan.workflow_path, WORKFLOW_PATH);
    assert_eq!(
        plan.topological_order,
        ["operator_config", "repository_tests"]
    );

    let operator = job(&plan, "operator_config");
    assert_eq!(
        operator.independent_profile.as_deref(),
        Some("node-hardened-verify")
    );
    assert!(operator.independent_reasons.is_empty());

    let tests = job(&plan, "repository_tests");
    assert_eq!(tests.needs, ["operator_config"]);
    assert_eq!(
        tests.independent_profile.as_deref(),
        Some("node-hardened-test")
    );
    assert!(tests.independent_reasons.is_empty());
}

#[test]
fn reserved_repository_path_and_revision_mismatches_are_terminal() {
    let cases = [
        request(
            REPOSITORY,
            REVISION,
            ".github/workflows/other.yml",
            WORKFLOW,
        ),
        request(
            "lookalike/msgint-connectors",
            REVISION,
            WORKFLOW_PATH,
            WORKFLOW,
        ),
        request(
            REPOSITORY,
            "0000000000000000000000000000000000000000",
            WORKFLOW_PATH,
            WORKFLOW,
        ),
    ];

    for candidate in cases {
        let plan = build_plan(&candidate, &PlannerLimits::default())
            .expect("reserved mismatch must classify into a terminal plan");
        assert!(!plan.independent_executable);
        assert!(plan.jobs.iter().all(|job| !job.independent_supported));
        assert!(plan
            .jobs
            .iter()
            .flat_map(|job| &job.independent_reasons)
            .any(|reason| reason.contains("reserved Messaging Intel")
                || reason.contains("reviewed revision")));
    }
}

#[test]
fn structural_action_input_command_and_secret_lookalikes_fail_closed() {
    let cases = [
        WORKFLOW.replacen(
            "name: Messaging Intel GHA clone operator verification",
            "name: Lookalike verification",
            1,
        ),
        WORKFLOW.replacen("jobs:\n", "permissions: read-all\njobs:\n", 1),
        WORKFLOW.replacen("  workflow_dispatch:\n", "  push:\n", 1),
        WORKFLOW.replacen(
            "actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1",
            "actions/checkout@main",
            1,
        ),
        WORKFLOW.replacen("persist-credentials: false", "persist-credentials: true", 1),
        WORKFLOW.replacen("node-version: \"22.23.1\"", "node-version: \"22\"", 1),
        WORKFLOW.replacen(
            "          cache: npm\n",
            "          cache: npm\n          registry-url: https://evil.invalid\n",
            1,
        ),
        WORKFLOW.replacen(
            "          npm audit --audit-level=high\n",
            "          npm audit --audit-level=high\n          npm publish\n",
            1,
        ),
        WORKFLOW.replacen(
            "          npm run check\n          npm run test:operator-config\n",
            "          npm run test:operator-config\n          npm run check\n",
            1,
        ),
        WORKFLOW.replacen(
            "          cache: npm\n",
            "          cache: npm\n          token: ${{ secrets.PROD_TOKEN }}\n",
            1,
        ),
        WORKFLOW.replacen(
            "      - run: |\n          npm ci --ignore-scripts\n",
            "      - run: |\n          npm ci --ignore-scripts\n        shell: bash\n",
            1,
        ),
    ];

    for candidate in cases {
        let plan = compile(&candidate);
        assert!(!plan.independent_executable, "lookalike was executable");
        assert!(plan.jobs.iter().all(|job| !job.independent_supported));
    }
}

#[test]
fn unrelated_workflow_retains_legacy_classifier_behavior() {
    let unrelated = request(
        "other/repository",
        REVISION,
        ".github/workflows/other.yml",
        "jobs:\n  test:\n    runs-on: ubuntu-latest\n    steps:\n      - run: npm ci && npm test\n",
    );
    let plan = build_plan(&unrelated, &PlannerLimits::default())
        .expect("unrelated workflow should retain legacy planning");
    assert_eq!(
        job(&plan, "test").independent_profile.as_deref(),
        Some("node-verify")
    );
}
