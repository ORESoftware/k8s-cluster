use std::{fs, path::PathBuf};

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(path: &str) -> String {
    let full = manifest_dir().join(path);
    fs::read_to_string(&full).unwrap_or_else(|error| panic!("read {}: {error}", full.display()))
}

#[test]
fn standalone_workflows_are_repository_local_and_immutable() {
    let ci = read(".github/workflows/ci.yml");
    let meta = read(".github/workflows/gha-clone-server-meta.yml");
    let self_test = read("tests/meta_self_test.rs");

    for workflow in [&ci, &meta] {
        assert!(workflow.contains("actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1"));
        assert!(
            workflow.contains("dtolnay/rust-toolchain@4be7066ada62dd38de10e7b70166bc74ed198c30")
        );
        assert!(workflow.contains("toolchain: '1.90.0'"));
        assert!(!workflow.contains("ubuntu-latest"));
    }

    for command in [
        "cargo fmt --all -- --check",
        "cargo clippy --locked --all-targets -- -D warnings",
        "cargo test --locked --all-targets",
    ] {
        assert!(ci.contains(command), "standalone CI is missing {command}");
        assert!(meta.contains(command), "meta fixture is missing {command}");
    }

    assert!(ci.contains("--target clone-server"));
    assert!(ci.contains("--target executor-router"));
    assert!(ci.contains("gitleaks/gitleaks-action@e0c47f4f8be36e29cdc102c57e68cb5cbf0e8d1e"));
    assert!(ci.contains("GHA_CLONE_EXECUTION_ENABLED=false"));
    assert!(ci.contains("GHA_CLONE_WEBHOOK_EXECUTION_ENABLED=false"));

    assert!(!self_test.contains("../../../.github/workflows"));
    assert!(self_test.contains(".join(WORKFLOW_PATH)"));
    assert!(self_test.contains("gha-indie-worker/gha-clone-server.rs"));
}

#[test]
fn standalone_source_provenance_is_explicit_and_fail_closed() {
    let provenance = read("SOURCE-PROVENANCE.md");
    for marker in [
        "repository: ORESoftware/k8s-cluster",
        "path: remote/deployments/gha-clone-server-rs",
        "standalone target: gha-indie-worker/gha-clone-server.rs",
        "full immutable 40-hex commit",
        "compare the target tree against the extracted source",
        "must never force-push",
    ] {
        assert!(
            provenance.contains(marker),
            "standalone provenance is missing {marker:?}"
        );
    }
}

#[test]
fn nested_workflows_parse_as_yaml_and_expose_expected_jobs() {
    for (path, expected_job) in [
        (".github/workflows/ci.yml", "rust-and-image"),
        (
            ".github/workflows/gha-clone-server-meta.yml",
            "gha-clone-self-test",
        ),
    ] {
        let value: serde_yaml::Value = serde_yaml::from_str(&read(path))
            .unwrap_or_else(|error| panic!("parse {path}: {error}"));
        let jobs = value
            .get("jobs")
            .and_then(serde_yaml::Value::as_mapping)
            .unwrap_or_else(|| panic!("{path} has no jobs mapping"));
        assert!(
            jobs.contains_key(serde_yaml::Value::String(expected_job.to_string())),
            "{path} is missing job {expected_job}"
        );
    }
}
