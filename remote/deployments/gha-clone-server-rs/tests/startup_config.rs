use std::{collections::BTreeMap, process::Command};

const SERVER_BINARY: &str = env!("CARGO_BIN_EXE_gha-clone-server");

const CONFIG_ENV_VARS: &[&str] = &[
    "HOST",
    "PORT",
    "GHA_CLONE_AUTH_SECRET",
    "GHA_CLONE_GITHUB_WEBHOOK_SECRET",
    "GHA_CLONE_GITHUB_TOKEN",
    "GHA_CLONE_BUILD_SERVER_URL",
    "GHA_CLONE_BUILD_SERVER_AUTH",
    "GHA_CLONE_ALLOWED_REPOSITORIES",
    "GHA_CLONE_WORKFLOW_RULES_JSON",
    "GHA_CLONE_EXECUTION_ENABLED",
    "GHA_CLONE_WEBHOOK_EXECUTION_ENABLED",
    "GHA_CLONE_MAX_WORKFLOW_BYTES",
    "GHA_CLONE_MAX_JOBS",
    "GHA_CLONE_MAX_STEPS_PER_JOB",
    "GHA_CLONE_BUILD_POLL_SECONDS",
    "GHA_CLONE_BUILD_TIMEOUT_SECONDS",
    "GHA_CLONE_MAX_RUNS",
];

fn run_with_env(overrides: BTreeMap<&str, &str>) -> (i32, String) {
    let mut command = Command::new(SERVER_BINARY);
    for &name in CONFIG_ENV_VARS {
        command.env_remove(name);
    }
    command.env("RUST_LOG", "error");
    for (name, value) in overrides {
        command.env(name, value);
    }

    let output = command.output().expect("execute gha-clone-server");
    let code = output.status.code().unwrap_or(-1);
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    (code, stderr)
}

fn assert_configuration_error(overrides: BTreeMap<&str, &str>, expected: &str) {
    let (code, stderr) = run_with_env(overrides);
    assert_eq!(code, 2, "unexpected exit code; stderr: {stderr}");
    assert!(
        stderr.contains("gha-clone-server: configuration error:"),
        "missing configuration-error prefix: {stderr}"
    );
    assert!(
        stderr.contains(expected),
        "expected {expected:?} in stderr: {stderr}"
    );
}

#[test]
fn invalid_boolean_flags_fail_before_network_startup() {
    for name in [
        "GHA_CLONE_EXECUTION_ENABLED",
        "GHA_CLONE_WEBHOOK_EXECUTION_ENABLED",
    ] {
        assert_configuration_error(
            BTreeMap::from([(name, "sometimes")]),
            &format!("{name} must be true or false"),
        );
    }
}

#[test]
fn invalid_port_values_fail_before_network_startup() {
    for value in ["not-a-port", "70000", "-1"] {
        assert_configuration_error(
            BTreeMap::from([("PORT", value)]),
            "PORT is invalid:",
        );
    }
}

#[test]
fn invalid_unsigned_limits_fail_before_network_startup() {
    for name in [
        "GHA_CLONE_MAX_WORKFLOW_BYTES",
        "GHA_CLONE_MAX_JOBS",
        "GHA_CLONE_MAX_STEPS_PER_JOB",
        "GHA_CLONE_BUILD_POLL_SECONDS",
        "GHA_CLONE_BUILD_TIMEOUT_SECONDS",
        "GHA_CLONE_MAX_RUNS",
    ] {
        assert_configuration_error(
            BTreeMap::from([(name, "not-a-number")]),
            &format!("{name} is invalid:"),
        );
    }
}

#[test]
fn malformed_workflow_rule_json_fails_before_network_startup() {
    for value in [
        "{",
        "[]",
        r#"{"owner/repo":".github/workflows/ci.yml"}"#,
        r#"{"owner/repo":[1]}"#,
    ] {
        assert_configuration_error(
            BTreeMap::from([("GHA_CLONE_WORKFLOW_RULES_JSON", value)]),
            "GHA_CLONE_WORKFLOW_RULES_JSON is invalid:",
        );
    }
}

#[test]
fn workflow_rules_cannot_escape_the_repository_allowlist() {
    assert_configuration_error(
        BTreeMap::from([
            (
                "GHA_CLONE_WORKFLOW_RULES_JSON",
                r#"{"owner/repo":[".github/workflows/ci.yml"]}"#,
            ),
            ("GHA_CLONE_ALLOWED_REPOSITORIES", "other/repo"),
        ]),
        "workflow rule repository \"owner/repo\" is absent from GHA_CLONE_ALLOWED_REPOSITORIES",
    );
}

#[test]
fn every_unallowlisted_rule_is_reported_deterministically() {
    assert_configuration_error(
        BTreeMap::from([
            (
                "GHA_CLONE_WORKFLOW_RULES_JSON",
                r#"{"z-owner/z-repo":[".github/workflows/z.yml"],"a-owner/a-repo":[".github/workflows/a.yml"]}"#,
            ),
            ("GHA_CLONE_ALLOWED_REPOSITORIES", "allowed/repo"),
        ]),
        "workflow rule repository \"a-owner/a-repo\" is absent",
    );
}
