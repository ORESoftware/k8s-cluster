use gha_clone_server::{build_plan, PlanRequest, PlannerLimits};

const REVISION: &str = "952623b07fd83caa3a83ee27bdea293f6bd4372f";

fn request(workflow_yaml: &str) -> PlanRequest {
    PlanRequest {
        repository: "messaging-intel/msgint-connectors".to_string(),
        revision: REVISION.to_string(),
        workflow_path: ".github/workflows/gha-clone-operator-config.yml".to_string(),
        workflow_yaml: workflow_yaml.to_string(),
    }
}

fn assert_parse_rejected(label: &str, workflow_yaml: &str) {
    let result = build_plan(&request(workflow_yaml), &PlannerLimits::default());
    assert!(result.is_err(), "{label} unexpectedly parsed: {result:#?}");
}

fn assert_not_executable(label: &str, workflow_yaml: &str) {
    match build_plan(&request(workflow_yaml), &PlannerLimits::default()) {
        Err(_) => {}
        Ok(plan) => assert!(
            !plan.independent_executable,
            "{label} unexpectedly compiled for independent execution: {plan:#?}"
        ),
    }
}

#[test]
fn duplicate_mapping_keys_are_rejected_in_every_security_relevant_position() {
    let cases = [
        (
            "duplicate root jobs",
            r#"
jobs:
  first:
    runs-on: ubuntu-latest
    steps:
      - run: npm test
jobs:
  second:
    runs-on: ubuntu-latest
    steps:
      - run: npm test
"#,
        ),
        (
            "duplicate job id",
            r#"
jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - run: npm test
  test:
    runs-on: ubuntu-latest
    steps:
      - run: npm publish
"#,
        ),
        (
            "duplicate run",
            r#"
jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - run: npm test
        run: npm publish
"#,
        ),
        (
            "duplicate uses",
            r#"
jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1
        uses: attacker/action@0123456789abcdef0123456789abcdef01234567
      - run: npm test
"#,
        ),
        (
            "duplicate env",
            r#"
jobs:
  test:
    runs-on: ubuntu-latest
    env:
      NODE_ENV: test
    env:
      TOKEN: ${{ secrets.PROD_TOKEN }}
    steps:
      - run: npm test
"#,
        ),
        (
            "duplicate needs",
            r#"
jobs:
  root:
    runs-on: ubuntu-latest
    steps:
      - run: npm test
  test:
    needs: root
    needs: missing
    runs-on: ubuntu-latest
    steps:
      - run: npm test
"#,
        ),
    ];

    for (label, yaml) in cases {
        assert_parse_rejected(label, yaml);
    }
}

#[test]
fn merge_keys_cannot_hide_or_duplicate_executable_steps() {
    assert_not_executable(
        "step merge key",
        r#"
jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - &reviewed
        run: npm test
      - <<: *reviewed
"#,
    );

    assert_not_executable(
        "job merge key",
        r#"
jobs:
  test:
    <<: &reviewed
      runs-on: ubuntu-latest
      steps:
        - run: npm test
"#,
    );
}

#[test]
fn tags_and_multiple_documents_fail_closed() {
    assert_parse_rejected(
        "tagged jobs mapping",
        r#"
jobs: !reviewed
  test:
    runs-on: ubuntu-latest
    steps:
      - run: npm test
"#,
    );

    assert_parse_rejected(
        "multiple YAML documents",
        r#"
---
jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - run: npm test
---
jobs:
  publish:
    runs-on: ubuntu-latest
    steps:
      - run: npm publish
"#,
    );
}

#[test]
fn non_string_and_confusable_security_keys_do_not_compile() {
    assert_parse_rejected(
        "non-string job id",
        r#"
jobs:
  42:
    runs-on: ubuntu-latest
    steps:
      - run: npm test
"#,
    );

    assert_not_executable(
        "fullwidth run key",
        r#"
jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - ｒｕｎ: npm test
"#,
    );
}
