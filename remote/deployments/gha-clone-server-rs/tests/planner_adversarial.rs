use std::collections::BTreeSet;

use gha_clone_server::{
    build_plan, capabilities, is_full_commit_sha, JobPlan, PlanRequest, PlannerLimits, WorkflowPlan,
};

const IMMUTABLE_SHA: &str = "0123456789abcdef0123456789abcdef01234567";

fn request(yaml: impl Into<String>) -> PlanRequest {
    PlanRequest {
        repository: "sonus-auris/sonus-auris-interfaces".to_string(),
        revision: IMMUTABLE_SHA.to_string(),
        workflow_path: ".github/workflows/ci.yml".to_string(),
        workflow_yaml: yaml.into(),
    }
}

fn plan(yaml: &str) -> WorkflowPlan {
    build_plan(&request(yaml), &PlannerLimits::default()).expect("workflow should plan")
}

fn errors(request: &PlanRequest, limits: &PlannerLimits) -> String {
    build_plan(request, limits)
        .expect_err("workflow should be rejected")
        .join("\n")
}

fn job<'a>(plan: &'a WorkflowPlan, id: &str) -> &'a JobPlan {
    plan.jobs
        .iter()
        .find(|job| job.id == id)
        .unwrap_or_else(|| panic!("missing job {id:?}"))
}

#[test]
fn capability_contract_is_unique_bounded_and_fail_closed() {
    let limits = PlannerLimits {
        max_workflow_bytes: 4096,
        max_jobs: 7,
        max_steps_per_job: 11,
    };
    let capabilities = capabilities(&limits);

    assert_eq!(capabilities.service, "gha-clone-server");
    assert_eq!(capabilities.plan_schema_version, "gha-clone-plan.v1");
    assert_eq!(capabilities.limits.max_workflow_bytes, 4096);
    assert_eq!(capabilities.limits.max_jobs, 7);
    assert_eq!(capabilities.limits.max_steps_per_job, 11);

    let profiles = capabilities
        .independent_profiles
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    assert_eq!(profiles.len(), capabilities.independent_profiles.len());
    assert!(profiles.contains("rust-verify"));
    assert!(profiles.contains("node-verify"));
    assert!(profiles.contains("python-verify"));

    let labels = capabilities
        .architecture
        .native_arc_labels
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    assert_eq!(
        labels.len(),
        capabilities.architecture.native_arc_labels.len()
    );
    assert!(labels.contains("sonus-ci"));
    assert!(labels.contains("sonus-ci-dind"));
    assert!(labels.contains("sonus-android-kvm"));
    assert!(!capabilities.explicitly_unsupported.is_empty());
}

#[test]
fn plan_ids_are_deterministic_and_domain_separated() {
    let base = request(
        r#"
jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - run: cargo test
"#,
    );
    let first = build_plan(&base, &PlannerLimits::default()).unwrap();
    let second = build_plan(&base, &PlannerLimits::default()).unwrap();
    assert_eq!(first.plan_id, second.plan_id);
    assert_eq!(first.plan_id.len(), 64);

    let mut ids = BTreeSet::from([first.plan_id.clone()]);
    for mut variant in [base.clone(), base.clone(), base.clone(), base.clone()] {
        match ids.len() {
            1 => variant.repository = "sonus-auris/another-repository".to_string(),
            2 => variant.revision = "f".repeat(40),
            3 => variant.workflow_path = ".github/workflows/other.yml".to_string(),
            4 => variant
                .workflow_yaml
                .push_str("\n# distinct source bytes\n"),
            _ => unreachable!(),
        }
        let variant_plan = build_plan(&variant, &PlannerLimits::default()).unwrap();
        assert!(ids.insert(variant_plan.plan_id));
    }
    assert_eq!(ids.len(), 5);
}

#[test]
fn invalid_request_metadata_accumulates_all_early_failures() {
    let mut invalid = request("x\0");
    invalid.repository = "owner/repo/extra".to_string();
    invalid.workflow_path = "../ci.txt".to_string();
    let limits = PlannerLimits {
        max_workflow_bytes: 1,
        ..PlannerLimits::default()
    };

    let errors = errors(&invalid, &limits);
    assert!(errors.contains("repository must be an owner/name"));
    assert!(errors.contains("workflowPath must stay under .github/workflows"));
    assert!(errors.contains("byte limit"));
    assert!(errors.contains("NUL bytes"));
}

#[test]
fn malformed_documents_and_job_shapes_are_rejected() {
    for (yaml, expected) in [
        ("[", "not valid YAML"),
        ("- one\n- two\n", "workflow document must be a YAML mapping"),
        ("name: missing-jobs\n", "workflow.jobs must be a mapping"),
        ("jobs: []\n", "workflow.jobs must be a mapping"),
        ("jobs: {}\n", "workflow.jobs must contain at least one job"),
        (
            "jobs:\n  1:\n    runs-on: ubuntu-latest\n    steps: [{ run: cargo test }]\n",
            "every workflow job ID must be a string",
        ),
        (
            "jobs:\n  bad.id:\n    runs-on: ubuntu-latest\n    steps: [{ run: cargo test }]\n",
            "job ID must use letters, numbers",
        ),
        ("jobs:\n  test: cargo test\n", "job must be a mapping"),
    ] {
        let message = errors(&request(yaml), &PlannerLimits::default());
        assert!(
            message.contains(expected),
            "expected {expected:?} in rejection for {yaml:?}; got {message:?}"
        );
    }
}

#[test]
fn planner_limits_accept_the_boundary_and_reject_the_next_item() {
    let limits = PlannerLimits {
        max_workflow_bytes: 16 * 1024,
        max_jobs: 2,
        max_steps_per_job: 2,
    };

    let at_limit = request(
        r#"
jobs:
  first:
    runs-on: ubuntu-latest
    steps:
      - run: cargo fmt --check
      - run: cargo test
  second:
    runs-on: ubuntu-latest
    steps:
      - run: npm ci
      - run: npm test
"#,
    );
    let accepted = build_plan(&at_limit, &limits).expect("exact limits should be accepted");
    assert_eq!(accepted.jobs.len(), 2);

    let too_many_jobs = request(
        r#"
jobs:
  first:
    runs-on: ubuntu-latest
    steps: [{ run: cargo test }]
  second:
    runs-on: ubuntu-latest
    steps: [{ run: npm test }]
  third:
    runs-on: ubuntu-latest
    steps: [{ run: pytest }]
"#,
    );
    assert!(errors(&too_many_jobs, &limits).contains("maximum is 2"));

    let too_many_steps = request(
        r#"
jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - run: cargo fmt --check
      - run: cargo clippy
      - run: cargo test
"#,
    );
    assert!(errors(&too_many_steps, &limits).contains("has 3 steps; maximum is 2"));
}

#[test]
fn malformed_runner_dependency_and_step_lists_are_specific() {
    let malformed = request(
        r#"
jobs:
  root:
    runs-on:
      label: ubuntu-latest
    steps: not-a-sequence
  child:
    needs: [root, 42]
    runs-on: [self-hosted, 7]
    steps:
      - scalar-step
"#,
    );
    let errors = errors(&malformed, &PlannerLimits::default());
    assert!(errors.contains("jobs.root.runs-on: expected a string or string sequence"));
    assert!(errors.contains("jobs.root.runs-on: at least one runner label is required"));
    assert!(errors.contains("jobs.root.steps must be a sequence"));
    assert!(errors.contains("jobs.child.needs: every item must be a string"));
    assert!(errors.contains("jobs.child.runs-on: every item must be a string"));
    assert!(errors.contains("jobs.child.steps[0]: step must be a mapping"));
}

#[test]
fn dependency_and_runner_lists_are_sorted_and_deduplicated() {
    let plan = plan(
        r#"
jobs:
  root:
    runs-on: ubuntu-latest
    steps: [{ run: cargo test }]
  child:
    needs: [root, root]
    runs-on: [self-hosted, linux, self-hosted, linux]
    steps: [{ run: npm test }]
"#,
    );

    assert_eq!(plan.topological_order, vec!["root", "child"]);
    assert_eq!(job(&plan, "child").needs, vec!["root"]);
    assert_eq!(job(&plan, "child").runs_on, vec!["linux", "self-hosted"]);
}

#[test]
fn self_dependencies_fail_before_execution() {
    let message = errors(
        &request(
            r#"
jobs:
  recursive:
    needs: recursive
    runs-on: ubuntu-latest
    steps: [{ run: cargo test }]
"#,
        ),
        &PlannerLimits::default(),
    );
    assert!(message.contains("job cannot depend on itself"));
}

#[test]
fn every_primary_fixed_profile_is_reachable_from_static_workflows() {
    let plan = plan(
        r#"
jobs:
  rust:
    runs-on: ubuntu-latest
    steps: [{ run: cargo test }]
  node:
    runs-on: ubuntu-latest
    steps: [{ run: pnpm test }]
  python:
    runs-on: ubuntu-latest
    steps: [{ run: python -m pytest }]
  flutter_verify:
    runs-on: ubuntu-latest
    steps: [{ run: flutter test }]
  flutter_android:
    runs-on: ubuntu-latest
    steps: [{ run: flutter build apk --debug }]
  flutter_web:
    runs-on: ubuntu-latest
    steps: [{ run: flutter build web --release }]
  flutter_linux:
    runs-on: ubuntu-latest
    steps: [{ run: flutter build linux --release }]
  flutter_desktop:
    runs-on: ubuntu-latest
    steps: [{ run: flutter build linux --release -t lib/main_desktop.dart }]
  playwright:
    runs-on: ubuntu-latest
    steps: [{ run: npx playwright test }]
  puppeteer:
    runs-on: ubuntu-latest
    steps: [{ run: node puppeteer-smoke.js }]
"#,
    );

    for (id, profile) in [
        ("rust", "rust-verify"),
        ("node", "node-verify"),
        ("python", "python-verify"),
        ("flutter_verify", "flutter-verify"),
        ("flutter_android", "flutter-android-debug"),
        ("flutter_web", "flutter-web-release"),
        ("flutter_linux", "flutter-linux-release"),
        ("flutter_desktop", "flutter-linux-desktop-entrypoint"),
        ("playwright", "playwright"),
        ("puppeteer", "puppeteer"),
    ] {
        let job = job(&plan, id);
        assert!(
            job.independent_supported,
            "{id} was rejected: {:?}",
            job.independent_reasons
        );
        assert_eq!(job.independent_profile.as_deref(), Some(profile));
    }
    assert!(plan.independent_executable);
}

#[test]
fn arc_lane_precedence_is_deterministic() {
    let plan = plan(
        r#"
jobs:
  native:
    runs-on: windows-2025
    steps: [{ run: cargo test }]
  android_over_dind_and_browser:
    runs-on: [self-hosted, linux, kvm]
    services:
      postgres:
        image: postgres:17
    steps: [{ run: npx playwright test }]
  dind_over_browser:
    runs-on: ubuntu-latest
    services:
      postgres:
        image: postgres:17
    steps: [{ run: npx playwright test }]
  browser:
    runs-on: ubuntu-latest
    steps: [{ run: selenium test }]
  ordinary:
    runs-on: ubuntu-latest
    steps: [{ run: cargo test }]
"#,
    );

    assert_eq!(job(&plan, "native").arc_lane, "github-hosted-native");
    assert!(!job(&plan, "native").arc_compatible);
    assert_eq!(
        job(&plan, "android_over_dind_and_browser").arc_lane,
        "sonus-android-kvm"
    );
    assert_eq!(job(&plan, "dind_over_browser").arc_lane, "sonus-ci-dind");
    assert_eq!(job(&plan, "browser").arc_lane, "sonus-browser");
    assert_eq!(job(&plan, "ordinary").arc_lane, "sonus-ci");
    assert!(!plan.arc_fully_covered);
}

#[test]
fn unsupported_workflow_and_job_semantics_disable_independent_execution() {
    let plan = plan(
        r#"
permissions:
  contents: read
concurrency: ci-main
defaults:
  run:
    shell: bash
jobs:
  reusable_like:
    uses: owner/repository/.github/workflows/reusable.yml@main
    permissions:
      contents: read
    environment: production
    secrets: inherit
    defaults:
      run:
        shell: bash
    outputs:
      result: ignored
    continue-on-error: true
    timeout-minutes: 5
    strategy:
      matrix:
        version: [20, 22]
    if: always()
    runs-on: ubuntu-latest
    services:
      postgres:
        image: postgres:17
    container: node:22
    steps: [{ run: npm test }]
  ordinary:
    runs-on: ubuntu-latest
    steps: [{ run: cargo test }]
"#,
    );

    assert!(!plan.independent_executable);
    for id in ["reusable_like", "ordinary"] {
        let reasons = job(&plan, id).independent_reasons.join("\n");
        assert!(reasons.contains("workflow-level permissions"));
        assert!(reasons.contains("workflow-level concurrency"));
        assert!(reasons.contains("workflow-level defaults"));
    }

    let reasons = job(&plan, "reusable_like").independent_reasons.join("\n");
    for expected in [
        "job-level uses",
        "job-level permissions",
        "job-level environment",
        "job-level secrets",
        "job-level defaults",
        "job-level outputs",
        "job-level continue-on-error",
        "job-level timeout-minutes",
        "service containers",
        "job containers",
        "dynamic strategy/matrix",
        "job-level if condition",
    ] {
        assert!(
            reasons.contains(expected),
            "missing reason {expected:?}: {reasons}"
        );
    }
}

#[test]
fn unsupported_step_semantics_are_never_silently_ignored() {
    let plan = plan(
        r#"
jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - run: cargo test
        if: success()
        working-directory: crates/server
        continue-on-error: true
        timeout-minutes: 5
        shell: bash
      - uses: actions/setup-node@820762786026740c76f36085b0efc47a31fe5020
        env:
          TOKEN: '${{ secrets.TOKEN }}'
        with:
          node-version: '22'
          token: '${{ secrets.OTHER_TOKEN }}'
"#,
    );

    let reasons = job(&plan, "test").independent_reasons.join("\n");
    for expected in [
        "conditional steps are unsupported",
        "working-directory is unsupported",
        "continue-on-error is unsupported",
        "timeout-minutes is unsupported",
        "shell is unsupported",
        "secret-bearing step environments are unsupported",
        "secret-bearing setup inputs are unsupported",
    ] {
        assert!(
            reasons.contains(expected),
            "missing reason {expected:?}: {reasons}"
        );
    }
    assert!(!job(&plan, "test").independent_supported);
}

#[test]
fn secret_github_token_and_oidc_contexts_fail_closed() {
    let plan = plan(
        r#"
jobs:
  job_secret:
    runs-on: ubuntu-latest
    env:
      TOKEN: '${{ secrets.PROD_TOKEN }}'
    steps: [{ run: cargo test }]
  compact_secret:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/setup-node@820762786026740c76f36085b0efc47a31fe5020
        with:
          token: '${{secrets.NPM_TOKEN}}'
      - run: npm test
  github_token:
    runs-on: ubuntu-latest
    steps:
      - run: npm test
        env:
          TOKEN: '${{ github.token }}'
  oidc:
    runs-on: ubuntu-latest
    steps:
      - run: python -m pytest
        env:
          REQUEST_URL: '${{ env.ACTIONS_ID_TOKEN_REQUEST_URL }}'
"#,
    );

    assert!(job(&plan, "job_secret")
        .independent_reasons
        .iter()
        .any(|reason| reason.contains("job environment contains a secret expression")));
    assert!(job(&plan, "compact_secret")
        .independent_reasons
        .iter()
        .any(|reason| reason.contains("secret-bearing setup inputs are unsupported")));
    for id in ["github_token", "oidc"] {
        assert!(job(&plan, id)
            .independent_reasons
            .iter()
            .any(|reason| reason.contains("secret-bearing step environments are unsupported")));
    }
    assert!(!plan.independent_executable);
}

#[test]
fn run_expressions_and_unknown_actions_fail_closed() {
    let plan = plan(
        r#"
jobs:
  expression:
    runs-on: ubuntu-latest
    steps:
      - run: cargo test --manifest-path '${{ matrix.path }}'
  unknown_action:
    runs-on: ubuntu-latest
    steps:
      - uses: vendor/arbitrary-action@main
      - run: npm test
"#,
    );

    assert!(job(&plan, "expression")
        .independent_reasons
        .iter()
        .any(|reason| reason.contains("expressions inside run commands")));
    assert!(job(&plan, "unknown_action")
        .independent_reasons
        .iter()
        .any(|reason| reason.contains("marketplace action")));
    assert!(!plan.independent_executable);
}

#[test]
fn allowed_setup_actions_are_case_insensitive_and_inputs_are_advisory() {
    let plan = plan(
        r#"
jobs:
  node:
    runs-on: ubuntu-latest
    steps:
      - uses: ACTIONS/CHECKOUT@3d3c42e5aac5ba805825da76410c181273ba90b1
      - uses: Actions/Setup-Node@820762786026740c76f36085b0efc47a31fe5020
        with:
          node-version: '22'
      - run: npm test
"#,
    );

    let job = job(&plan, "node");
    assert!(
        job.independent_supported,
        "unexpected rejection: {:?}",
        job.independent_reasons
    );
    assert_eq!(job.independent_profile.as_deref(), Some("node-verify"));
    assert!(job
        .independent_notes
        .iter()
        .any(|note| note.contains("fixed profile pins the actual toolchain")));
}

#[test]
fn immutable_revision_gate_requires_exactly_forty_hex_characters() {
    assert!(is_full_commit_sha(IMMUTABLE_SHA));
    assert!(is_full_commit_sha(&"A".repeat(40)));
    assert!(!is_full_commit_sha(&"a".repeat(39)));
    assert!(!is_full_commit_sha(&"a".repeat(41)));
    assert!(!is_full_commit_sha(&format!("{}g", "a".repeat(39))));

    let mut mutable = request(
        r#"
jobs:
  test:
    runs-on: ubuntu-latest
    steps: [{ run: cargo test }]
"#,
    );
    mutable.revision = "main".to_string();
    let plan = build_plan(&mutable, &PlannerLimits::default()).unwrap();
    assert!(!plan.immutable_revision);
    assert!(!plan.independent_executable);
    assert_eq!(plan.warnings.len(), 1);
}

#[test]
fn serialized_plan_contract_uses_camel_case_fields() {
    let plan = plan(
        r#"
jobs:
  test:
    runs-on: ubuntu-latest
    steps: [{ run: cargo test }]
"#,
    );
    let value = serde_json::to_value(plan).unwrap();

    for key in [
        "schemaVersion",
        "planId",
        "immutableRevision",
        "arcFullyCovered",
        "independentExecutable",
        "topologicalOrder",
    ] {
        assert!(value.get(key).is_some(), "missing serialized field {key}");
    }
    assert!(value.get("schema_version").is_none());

    let serialized_job = &value["jobs"][0];
    for key in [
        "runsOn",
        "arcCompatible",
        "arcLane",
        "independentSupported",
        "independentProfile",
        "independentReasons",
        "independentNotes",
    ] {
        assert!(serialized_job.get(key).is_some(), "missing job field {key}");
    }
}

#[test]
fn unmatched_commands_are_valid_plans_but_never_independent_runs() {
    let plan = plan(
        r#"
jobs:
  unmatched:
    runs-on: ubuntu-latest
    steps:
      - run: echo hello
"#,
    );
    let job = job(&plan, "unmatched");
    assert!(job.arc_compatible);
    assert_eq!(job.arc_lane, "sonus-ci");
    assert!(!job.independent_supported);
    assert!(job.independent_profile.is_none());
    assert!(job
        .independent_reasons
        .iter()
        .any(|reason| reason.contains("no fixed build-server profile")));
    assert!(!plan.independent_executable);
}
