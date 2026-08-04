//! Exact repository, revision, workflow, DAG, action, input, and command
//! contract for the private Messaging Intel continuity mirror.
//!
//! A reserved identity mismatch is terminal. It must never fall back to the
//! generic Node classifier because that would execute a different repository
//! revision under a privileged fixed profile.

use std::collections::BTreeMap;

use serde_yaml::{Mapping, Value};

pub const MSGINT_REPOSITORY: &str = "messaging-intel/msgint-connectors";
pub const MSGINT_REVISION: &str = "a9cc977d78347ec0efdbe8e6766967f80d425882";
pub const MSGINT_WORKFLOW_PATH: &str = ".github/workflows/gha-clone-operator-config.yml";
pub const MSGINT_WORKFLOW_NAME: &str = "Messaging Intel GHA clone operator verification";
pub const MSGINT_CHECKOUT_ACTION: &str =
    "actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1";
pub const MSGINT_SETUP_NODE_ACTION: &str =
    "actions/setup-node@820762786026740c76f36085b0efc47a31fe5020";

const OPERATOR_COMMANDS: &[&str] = &[
    "npm ci --ignore-scripts",
    "npm run check",
    "npm run test:operator-config",
    "npm audit --audit-level=high",
];
const REPOSITORY_COMMANDS: &[&str] = &["npm ci --ignore-scripts", "npm test"];

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ContractMatch {
    NotApplicable,
    Match(BTreeMap<String, String>),
    Reject(Vec<String>),
}

pub fn classify_msgint_workflow(
    repository: &str,
    revision: &str,
    workflow_path: &str,
    root: &Mapping,
) -> ContractMatch {
    let repository_reserved = repository == MSGINT_REPOSITORY;
    let path_reserved = workflow_path == MSGINT_WORKFLOW_PATH;
    if !repository_reserved && !path_reserved {
        return ContractMatch::NotApplicable;
    }

    let mut reasons = Vec::new();
    if !repository_reserved || !path_reserved {
        return ContractMatch::Reject(vec![
            "reserved Messaging Intel repository/workflow identity mismatch".into(),
        ]);
    }
    if revision != MSGINT_REVISION {
        reasons.push(format!(
            "reserved Messaging Intel workflow requires reviewed revision {MSGINT_REVISION}"
        ));
    }
    if contains_secret_expression(&Value::Mapping(root.clone())) {
        reasons.push("secret-bearing expressions are unsupported".into());
    }
    if !exact_keys(root, &["name", "on", "jobs"]) {
        reasons.push("workflow root must contain exactly name, on, and jobs".into());
    }
    if get(root, "name").and_then(Value::as_str) != Some(MSGINT_WORKFLOW_NAME) {
        reasons.push("workflow name differs from the reviewed contract".into());
    }
    if !workflow_dispatch_only(get(root, "on")) {
        reasons.push("trigger must be exactly workflow_dispatch without inputs".into());
    }

    let Some(jobs) = get(root, "jobs").and_then(Value::as_mapping) else {
        reasons.push("jobs must be a mapping".into());
        return ContractMatch::Reject(reasons);
    };
    if !exact_ordered_keys(jobs, &["operator_config", "repository_tests"]) {
        reasons.push("job set or order differs from the reviewed two-job DAG".into());
    }
    validate_job(
        jobs,
        "operator_config",
        None,
        OPERATOR_COMMANDS,
        &mut reasons,
    );
    validate_job(
        jobs,
        "repository_tests",
        Some("operator_config"),
        REPOSITORY_COMMANDS,
        &mut reasons,
    );

    if !reasons.is_empty() {
        return ContractMatch::Reject(reasons);
    }
    ContractMatch::Match(BTreeMap::from([
        ("operator_config".into(), "node-hardened-verify".into()),
        ("repository_tests".into(), "node-hardened-test".into()),
    ]))
}

fn validate_job(
    jobs: &Mapping,
    id: &str,
    needs: Option<&str>,
    commands: &[&str],
    reasons: &mut Vec<String>,
) {
    let Some(job) = get(jobs, id).and_then(Value::as_mapping) else {
        reasons.push(format!("{id}: missing or not a mapping"));
        return;
    };
    let expected: &[&str] = if needs.is_some() {
        &["needs", "runs-on", "steps"]
    } else {
        &["runs-on", "steps"]
    };
    if !exact_keys(job, expected) {
        reasons.push(format!("{id}: job keys differ from the reviewed contract"));
    }
    if get(job, "runs-on").and_then(Value::as_str) != Some("ubuntu-latest") {
        reasons.push(format!("{id}: runner differs from ubuntu-latest"));
    }
    if let Some(expected_needs) = needs {
        if get(job, "needs").and_then(Value::as_str) != Some(expected_needs) {
            reasons.push(format!("{id}: dependency differs from {expected_needs}"));
        }
    }

    let Some(steps) = get(job, "steps").and_then(Value::as_sequence) else {
        reasons.push(format!("{id}: steps must be a sequence"));
        return;
    };
    if steps.len() != 3 {
        reasons.push(format!("{id}: expected exactly three reviewed steps"));
        return;
    }
    if !exact_checkout(&steps[0]) {
        reasons.push(format!(
            "{id}: checkout step must use the exact 40-hex commit SHA and persist-credentials=false"
        ));
    }
    if !exact_setup_node(&steps[1]) {
        reasons.push(format!(
            "{id}: setup-node step must use the exact 40-hex commit SHA and reviewed Node/cache inputs"
        ));
    }
    if !exact_run(&steps[2], commands) {
        reasons.push(format!(
            "{id}: exact reviewed command sequence differs or contains extra commands"
        ));
    }
}

fn exact_checkout(value: &Value) -> bool {
    let Some(step) = value.as_mapping() else {
        return false;
    };
    if !exact_keys(step, &["uses", "with"])
        || get(step, "uses").and_then(Value::as_str) != Some(MSGINT_CHECKOUT_ACTION)
    {
        return false;
    }
    let Some(with) = get(step, "with").and_then(Value::as_mapping) else {
        return false;
    };
    exact_keys(with, &["persist-credentials"])
        && get(with, "persist-credentials").and_then(Value::as_bool) == Some(false)
}

fn exact_setup_node(value: &Value) -> bool {
    let Some(step) = value.as_mapping() else {
        return false;
    };
    if !exact_keys(step, &["uses", "with"])
        || get(step, "uses").and_then(Value::as_str) != Some(MSGINT_SETUP_NODE_ACTION)
    {
        return false;
    }
    let Some(with) = get(step, "with").and_then(Value::as_mapping) else {
        return false;
    };
    exact_keys(with, &["node-version", "cache"])
        && get(with, "node-version").and_then(Value::as_str) == Some("22.23.1")
        && get(with, "cache").and_then(Value::as_str) == Some("npm")
}

fn exact_run(value: &Value, expected: &[&str]) -> bool {
    let Some(step) = value.as_mapping() else {
        return false;
    };
    if !exact_keys(step, &["run"]) {
        return false;
    }
    let Some(script) = get(step, "run").and_then(Value::as_str) else {
        return false;
    };
    let normalized = script.replace("\r\n", "\n");
    let actual = normalized
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();
    actual == expected
}

fn workflow_dispatch_only(value: Option<&Value>) -> bool {
    let Some(on) = value.and_then(Value::as_mapping) else {
        return false;
    };
    if !exact_keys(on, &["workflow_dispatch"]) {
        return false;
    }
    matches!(get(on, "workflow_dispatch"), Some(Value::Null))
        || get(on, "workflow_dispatch")
            .and_then(Value::as_mapping)
            .is_some_and(Mapping::is_empty)
}

fn contains_secret_expression(value: &Value) -> bool {
    let compact = serde_yaml::to_string(value)
        .unwrap_or_default()
        .to_ascii_lowercase()
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect::<String>();
    compact.contains("${{secrets")
        || compact.contains("tojson(secrets)")
        || compact.contains("fromjson(secrets)")
        || compact.contains("github.token")
        || compact.contains("github['token']")
        || compact.contains("github[\"token\"]")
        || compact.contains("actions_id_token_request")
}

fn exact_keys(mapping: &Mapping, expected: &[&str]) -> bool {
    mapping.len() == expected.len() && expected.iter().all(|key| get(mapping, key).is_some())
}

fn exact_ordered_keys(mapping: &Mapping, expected: &[&str]) -> bool {
    mapping.len() == expected.len()
        && mapping
            .keys()
            .filter_map(Value::as_str)
            .eq(expected.iter().copied())
}

fn get<'a>(mapping: &'a Mapping, key: &str) -> Option<&'a Value> {
    mapping.get(Value::String(key.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const REVIEWED: &str = r#"name: Messaging Intel GHA clone operator verification
on:
  workflow_dispatch:
jobs:
  operator_config:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1
        with:
          persist-credentials: false
      - uses: actions/setup-node@820762786026740c76f36085b0efc47a31fe5020
        with:
          node-version: "22.23.1"
          cache: npm
      - run: |
          npm ci --ignore-scripts
          npm run check
          npm run test:operator-config
          npm audit --audit-level=high
  repository_tests:
    needs: operator_config
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1
        with:
          persist-credentials: false
      - uses: actions/setup-node@820762786026740c76f36085b0efc47a31fe5020
        with:
          node-version: "22.23.1"
          cache: npm
      - run: |
          npm ci --ignore-scripts
          npm test
"#;

    fn root(source: &str) -> Mapping {
        serde_yaml::from_str::<Value>(source)
            .expect("valid YAML")
            .as_mapping()
            .expect("workflow mapping")
            .clone()
    }

    fn classify(source: &str) -> ContractMatch {
        classify_msgint_workflow(
            MSGINT_REPOSITORY,
            MSGINT_REVISION,
            MSGINT_WORKFLOW_PATH,
            &root(source),
        )
    }

    fn changed(old: &str, new: &str) -> String {
        assert!(REVIEWED.contains(old), "mutation anchor must exist");
        REVIEWED.replacen(old, new, 1)
    }

    #[test]
    fn accepts_only_the_reviewed_identity_and_profiles() {
        let ContractMatch::Match(profiles) = classify(REVIEWED) else {
            panic!("reviewed workflow should match");
        };
        assert_eq!(profiles.len(), 2);
        assert_eq!(
            profiles.get("operator_config").map(String::as_str),
            Some("node-hardened-verify")
        );
        assert_eq!(
            profiles.get("repository_tests").map(String::as_str),
            Some("node-hardened-test")
        );
    }

    #[test]
    fn reserved_repository_path_and_revision_mismatches_are_terminal() {
        assert!(matches!(
            classify_msgint_workflow(
                MSGINT_REPOSITORY,
                MSGINT_REVISION,
                ".github/workflows/other.yml",
                &root(REVIEWED)
            ),
            ContractMatch::Reject(_)
        ));
        assert!(matches!(
            classify_msgint_workflow(
                "lookalike/msgint-connectors",
                MSGINT_REVISION,
                MSGINT_WORKFLOW_PATH,
                &root(REVIEWED)
            ),
            ContractMatch::Reject(_)
        ));
        assert!(matches!(
            classify_msgint_workflow(
                MSGINT_REPOSITORY,
                "0000000000000000000000000000000000000000",
                MSGINT_WORKFLOW_PATH,
                &root(REVIEWED)
            ),
            ContractMatch::Reject(_)
        ));
        assert_eq!(
            classify_msgint_workflow(
                "other/repository",
                MSGINT_REVISION,
                ".github/workflows/other.yml",
                &root(REVIEWED)
            ),
            ContractMatch::NotApplicable
        );
    }

    #[test]
    fn rejects_structural_action_input_and_command_lookalikes() {
        let cases = [
            changed(
                "name: Messaging Intel GHA clone operator verification",
                "name: Lookalike verification",
            ),
            changed("jobs:\n", "permissions: read-all\njobs:\n"),
            changed("  workflow_dispatch:\n", "  push:\n"),
            changed(
                "  workflow_dispatch:\n",
                "  workflow_dispatch:\n    inputs:\n      run:\n        type: boolean\n",
            ),
            changed(
                "    runs-on: ubuntu-latest\n    steps:\n",
                "    runs-on: self-hosted\n    steps:\n",
            ),
            changed(
                "    runs-on: ubuntu-latest\n    steps:\n",
                "    runs-on: ubuntu-latest\n    env:\n      NODE_ENV: test\n    steps:\n",
            ),
            changed(
                "actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1",
                "actions/checkout@0000000000000000000000000000000000000000",
            ),
            changed("persist-credentials: false", "persist-credentials: true"),
            changed("node-version: \"22.23.1\"", "node-version: \"22\""),
            changed(
                "          cache: npm\n",
                "          cache: npm\n          registry-url: https://evil.invalid\n",
            ),
            changed(
                "          npm audit --audit-level=high\n",
                "          npm audit --audit-level=high\n          npm publish\n",
            ),
            changed(
                "          npm run check\n          npm run test:operator-config\n",
                "          npm run test:operator-config\n          npm run check\n",
            ),
            changed(
                "          npm run test:operator-config\n",
                "          echo npm run test:operator-config\n",
            ),
            changed(
                "          npm run test:operator-config\n",
                "          npm run test:operator-config\n      - run: echo extra\n",
            ),
            changed(
                "          cache: npm\n",
                "          cache: npm\n          token: ${{ secrets['PROD_TOKEN'] }}\n",
            ),
            changed(
                "      - run: |\n          npm ci --ignore-scripts\n",
                "      - run: |\n          npm ci --ignore-scripts\n        shell: bash\n",
            ),
        ];

        for candidate in cases {
            assert!(
                matches!(classify(&candidate), ContractMatch::Reject(_)),
                "lookalike workflow was accepted:\n{candidate}"
            );
        }
    }
}
