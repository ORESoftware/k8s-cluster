use gha_clone_server::{build_plan, PlanRequest, PlannerLimits};

const IMMUTABLE_SHA: &str = "0123456789abcdef0123456789abcdef01234567";

fn request(repository: &str, workflow_path: &str, yaml: &str) -> PlanRequest {
    PlanRequest {
        repository: repository.to_string(),
        revision: IMMUTABLE_SHA.to_string(),
        workflow_path: workflow_path.to_string(),
        workflow_yaml: yaml.to_string(),
    }
}

fn simple_workflow(job_id: &str) -> String {
    format!(
        r#"
jobs:
  {job_id}:
    runs-on: ubuntu-latest
    steps:
      - run: cargo test
"#
    )
}

fn rejection(repository: &str, workflow_path: &str, yaml: &str) -> String {
    build_plan(
        &request(repository, workflow_path, yaml),
        &PlannerLimits::default(),
    )
    .expect_err("input should be rejected")
    .join("\n")
}

#[test]
fn repository_components_cannot_be_dot_segments_or_url_syntax() {
    let workflow = simple_workflow("test");
    for repository in [
        "./repo",
        "../repo",
        "owner/.",
        "owner/..",
        "owner/repo?ref=main",
        "owner/repo#fragment",
        "owner/repo%2fother",
        "owner/re po",
        "/repo",
        "owner/",
        "owner/repo/extra",
    ] {
        let message = rejection(repository, ".github/workflows/ci.yml", &workflow);
        assert!(
            message.contains("repository must be an owner/name"),
            "repository {repository:?} was not rejected canonically: {message}"
        );
    }
}

#[test]
fn repository_components_are_bounded_without_rejecting_safe_github_names() {
    let workflow = simple_workflow("test");
    for repository in [
        "owner/repository",
        "owner-name/repository.name",
        "owner_name/.github",
    ] {
        build_plan(
            &request(repository, ".github/workflows/ci.yml", &workflow),
            &PlannerLimits::default(),
        )
        .unwrap_or_else(|errors| panic!("safe repository {repository:?} failed: {errors:?}"));
    }

    for repository in [
        format!("{}/repo", "a".repeat(101)),
        format!("owner/{}", "b".repeat(101)),
    ] {
        let message = rejection(&repository, ".github/workflows/ci.yml", &workflow);
        assert!(message.contains("repository must be an owner/name"));
    }
}

#[test]
fn workflow_path_is_a_single_safe_file_under_the_github_workflows_directory() {
    let workflow = simple_workflow("test");
    for workflow_path in [
        ".github/workflows/ci.yml",
        ".github/workflows/release-1.2_test.yaml",
        ".github/workflows/ci..verify.yml",
    ] {
        build_plan(
            &request("owner/repo", workflow_path, &workflow),
            &PlannerLimits::default(),
        )
        .unwrap_or_else(|errors| panic!("safe workflow path {workflow_path:?} failed: {errors:?}"));
    }
}

#[test]
fn workflow_paths_reject_subdirectories_encoded_traversal_and_url_delimiters() {
    let workflow = simple_workflow("test");
    for workflow_path in [
        ".github/workflows/subdir/ci.yml",
        ".github/workflows//ci.yml",
        ".github/workflows/./ci.yml",
        ".github/workflows/../ci.yml",
        ".github/workflows/%2e%2e/ci.yml",
        ".github/workflows/%2E%2E%2Fci.yml",
        ".github/workflows/ci?download=.yml",
        ".github/workflows/ci#fragment.yml",
        ".github/workflows/ci workflow.yml",
        ".github/workflows/café.yml",
        ".github/workflows/ci.yml/extra.yaml",
        ".github/workflows/.yml",
        ".github/workflows/..yml",
        ".github/workflows/ci\nname.yml",
        "github/workflows/ci.yml",
        "/.github/workflows/ci.yml",
        ".github/workflows/ci.txt",
    ] {
        let message = rejection("owner/repo", workflow_path, &workflow);
        assert!(
            message.contains("workflowPath must stay under .github/workflows"),
            "workflow path {workflow_path:?} was not rejected canonically: {message}"
        );
    }
}

#[test]
fn workflow_path_length_limit_is_enforced_after_canonical_shape_checks() {
    let workflow = simple_workflow("test");
    let long_name = format!(".github/workflows/{}.yml", "a".repeat(240));
    assert!(long_name.len() > 256);
    let message = rejection("owner/repo", &long_name, &workflow);
    assert!(message.contains("workflowPath must stay under .github/workflows"));
}

#[test]
fn job_ids_follow_githubs_leading_character_rule() {
    for job_id in ["test", "_test", "Build_1", "build-1"] {
        let workflow = simple_workflow(job_id);
        build_plan(
            &request("owner/repo", ".github/workflows/ci.yml", &workflow),
            &PlannerLimits::default(),
        )
        .unwrap_or_else(|errors| panic!("safe job ID {job_id:?} failed: {errors:?}"));
    }

    for job_id in ["1test", "-test", ".test"] {
        let workflow = simple_workflow(job_id);
        let message = rejection("owner/repo", ".github/workflows/ci.yml", &workflow);
        assert!(
            message.contains("job ID must start with a letter or '_'"),
            "job ID {job_id:?} was not rejected canonically: {message}"
        );
    }
}

#[test]
fn duplicate_yaml_job_keys_are_rejected_instead_of_last_write_winning() {
    let message = rejection(
        "owner/repo",
        ".github/workflows/ci.yml",
        r#"
jobs:
  test:
    runs-on: ubuntu-latest
    steps: [{ run: cargo test }]
  test:
    runs-on: ubuntu-latest
    steps: [{ run: npm test }]
"#,
    );
    assert!(
        message.contains("not valid YAML") || message.contains("duplicate"),
        "duplicate jobs were not rejected explicitly: {message}"
    );
}

#[test]
fn deterministic_structured_garbage_never_panics_the_public_planner() {
    let alphabet = [
        'a', 'Z', '0', ':', '-', '_', '[', ']', '{', '}', '\n', '\t', '$', '{', '}', '/', '.',
        '%', '?', '#', '\\', '\0', 'é', '💥',
    ];
    let mut state = 0x5eed_u64;

    for length in 0..512 {
        let mut yaml = String::with_capacity(length);
        for _ in 0..length {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            yaml.push(alphabet[(state as usize) % alphabet.len()]);
        }

        let request = request("owner/repo", ".github/workflows/ci.yml", &yaml);
        let result = std::panic::catch_unwind(|| build_plan(&request, &PlannerLimits::default()));
        assert!(result.is_ok(), "planner panicked for generated input {yaml:?}");
    }
}
