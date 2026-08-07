use std::{fs, path::PathBuf};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repository remote directory")
}

fn read(relative: &str) -> String {
    fs::read_to_string(root().join(relative)).unwrap_or_else(|error| {
        panic!("failed to read {relative}: {error}");
    })
}

const PACKAGE: &str = "argocd/ci-runners/gha-executor-router";

#[test]
fn gitops_package_is_complete_but_not_activated() {
    let kustomization = read(&format!("{PACKAGE}/kustomization.yaml"));
    for file in [
        "namespace.yaml",
        "configmap.yaml",
        "externalsecrets.yaml",
        "deployment.yaml",
        "service.yaml",
        "networkpolicy.yaml",
    ] {
        assert!(
            kustomization.contains(&format!("  - {file}")),
            "missing {file}"
        );
    }

    let deployment = read(&format!("{PACKAGE}/deployment.yaml"));
    for required in [
        "replicas: 0",
        "GHA_EXECUTOR_ROUTER_AUTH_PATH",
        "GHA_EXECUTOR_ROUTER_IMAGE_DIGEST",
        "automountServiceAccountToken: false",
        "runAsNonRoot: true",
        "allowPrivilegeEscalation: false",
        "readOnlyRootFilesystem: true",
        "drop: [\"ALL\"]",
        "seccompProfile:",
        "path: /readyz",
        "path: /healthz",
    ] {
        assert!(deployment.contains(required), "missing {required}");
    }
    assert!(!deployment.contains("replicas: 1"));
    assert!(!deployment.contains("privileged: true"));
    assert!(!deployment.contains("hostPath:"));
    assert!(!deployment.contains("docker.sock"));
}

#[test]
fn provider_order_and_credentials_are_explicit_and_separate() {
    let config = read(&format!("{PACKAGE}/configmap.yaml"));
    assert!(config.contains("GHA_EXECUTOR_ROUTER_EXECUTION_ENABLED: \"false\""));
    let aws = config.find("\"id\": \"aws-primary\"").expect("AWS route");
    let hetzner = config
        .find("\"id\": \"hetzner-secondary\"")
        .expect("Hetzner route");
    assert!(aws < hetzner, "AWS must be attempted before Hetzner");
    assert!(config.contains("dd-build-server.default.svc.cluster.local:8100"));
    assert!(config.contains("https://build-server.hetzner.example.invalid"));
    assert!(config.contains("/var/run/gha-executor-router/aws/auth"));
    assert!(config.contains("/var/run/gha-executor-router/hetzner/auth"));
    assert!(!config.contains("authSecret"));
    assert!(!config.contains("x-build-server-auth"));

    let secrets = read(&format!("{PACKAGE}/externalsecrets.yaml"));
    for backing_key in [
        "dd/ci/gha-executor-router/inbound",
        "dd/ci/gha-executor-router/aws",
        "dd/ci/gha-executor-router/hetzner",
    ] {
        assert!(secrets.contains(backing_key), "missing {backing_key}");
    }
    assert_eq!(secrets.matches("kind: ExternalSecret").count(), 3);
    assert_eq!(secrets.matches("creationPolicy: Owner").count(), 3);
}

#[test]
fn network_policy_allows_only_the_reviewed_continuity_edges() {
    let policy = read(&format!("{PACKAGE}/networkpolicy.yaml"));
    for required in [
        "app: dd-gha-clone-server",
        "app: dd-build-server",
        "port: 8126",
        "port: 8100",
        "port: 443",
        "k8s-app: kube-dns",
        "169.254.0.0/16",
        "100.64.0.0/10",
    ] {
        assert!(policy.contains(required), "missing {required}");
    }
    assert!(!policy.contains("0.0.0.0/0\n      ports:\n        - protocol: TCP\n          port: 8100"));
}

#[test]
fn router_source_preserves_the_non_duplication_and_secret_boundary() {
    let source = read("deployments/gha-clone-server-rs/src/bin/gha-executor-router.rs");
    for required in [
        "RedirectPolicy::none()",
        "request_digest",
        "duplicate_requests",
        "ambiguous_acceptances",
        "pinned_poll_failures",
        "StatusCode::TOO_MANY_REQUESTS",
        "status.is_server_error()",
        "status.is_client_error()",
        "x-build-server-auth",
        "auth_path",
        "read_secret_file",
    ] {
        assert!(source.contains(required), "missing source contract {required}");
    }
    assert!(!source.contains("Command::new"));
    assert!(!source.contains("/bin/bash"));
    assert!(!source.contains("caller_provider"));
    assert!(!source.contains("caller_url"));
}

#[test]
fn runbook_states_activation_and_post_acceptance_limits() {
    let doc = read(&format!("{PACKAGE}/README.md"));
    for required in [
        "not a GitHub Actions control-plane clone",
        "Provider failover is allowed only before",
        "poll is pinned to the accepting provider",
        "Cross-provider resumption after acceptance is not enabled",
        "Keep `replicas: 0`",
        "Fiducia-fenced claim",
        "Do not force a post-acceptance job onto the other provider",
    ] {
        assert!(doc.contains(required), "missing runbook contract {required}");
    }
}

#[test]
fn new_router_slice_contains_no_committed_github_token_or_private_key() {
    let files = [
        "deployments/gha-clone-server-rs/src/bin/gha-executor-router.rs",
        "deployments/gha-clone-server-rs/tests/executor_router.rs",
        "deployments/gha-clone-server-rs/tests/executor_router_gitops.rs",
        "argocd/ci-runners/gha-executor-router/configmap.yaml",
        "argocd/ci-runners/gha-executor-router/externalsecrets.yaml",
        "argocd/ci-runners/gha-executor-router/deployment.yaml",
        "argocd/ci-runners/gha-executor-router/README.md",
    ];
    for file in files {
        let text = read(file);
        assert!(!text.contains("ghp_"), "classic PAT marker in {file}");
        assert!(!text.contains("github_pat_"), "fine-grained PAT marker in {file}");
        assert!(!text.contains("BEGIN PRIVATE KEY"), "private key in {file}");
        assert!(!text.contains("BEGIN RSA PRIVATE KEY"), "RSA key in {file}");
    }
}
