//! Architecture contracts for the fabrication service.
//!
//! These tests intentionally inspect source layout: modularity is a property of
//! the crate's composition, not only of its runtime behavior.

const MAIN: &str = include_str!("../src/main.rs");
const LIB: &str = include_str!("../src/lib.rs");
const CONFIG: &str = include_str!("../src/config.rs");
const METRICS: &str = include_str!("../src/metrics.rs");
const OBSERVABILITY: &str = include_str!("../src/observability.rs");
const PERSISTENCE: &str = include_str!("../src/persistence.rs");
const SECRETS: &str = include_str!("../src/secrets.rs");
const MANIFEST: &str = include_str!("../Cargo.toml");
const README: &str = include_str!("../readme.md");

fn nonempty_line_count(source: &str) -> usize {
    source
        .lines()
        .filter(|line| !line.trim().is_empty())
        .count()
}

fn has_direct_dependency(manifest: &str, dependency: &str) -> bool {
    let assignment = format!("{dependency} =");
    manifest
        .lines()
        .any(|line| line.trim_start().starts_with(&assignment))
}

#[test]
fn binary_entrypoint_stays_thin() {
    assert!(
        nonempty_line_count(MAIN) <= 5,
        "main.rs should only initialize Tokio and delegate to the library"
    );
    assert!(MAIN.contains("dd_fabrication_server::run().await"));

    for implementation_detail in ["Router::new", "sea_orm", "async_nats", "dd_telemetry"] {
        assert!(
            !MAIN.contains(implementation_detail),
            "main.rs must not own {implementation_detail}"
        );
    }
}

#[test]
fn runtime_concerns_have_dedicated_modules() {
    for module in [
        "config",
        "metrics",
        "observability",
        "persistence",
        "secrets",
    ] {
        assert!(
            LIB.lines()
                .any(|line| line.trim() == format!("mod {module};")),
            "lib.rs must compose the {module} module"
        );
    }

    assert!(CONFIG.contains("struct ServiceConfig"));
    assert!(METRICS.contains("struct Metrics"));
    assert!(OBSERVABILITY.contains("dd_telemetry::init"));
    assert!(PERSISTENCE.contains("enum Persistence"));
    assert!(SECRETS.contains("struct SecretOverlay"));
    assert!(LIB.contains("pub async fn run()"));
}

#[test]
fn persistence_uses_seaorm_without_a_direct_sqlx_dependency() {
    assert!(has_direct_dependency(MANIFEST, "sea-orm"));
    assert!(PERSISTENCE.contains("use sea_orm::"));
    assert!(!has_direct_dependency(MANIFEST, "sqlx"));

    for (name, source) in [
        ("main.rs", MAIN),
        ("lib.rs", LIB),
        ("config.rs", CONFIG),
        ("persistence.rs", PERSISTENCE),
    ] {
        assert!(
            !source.contains("sqlx::"),
            "{name} must not bypass SeaORM with SQLx"
        );
    }
}

#[test]
fn observability_is_composed_at_service_boundaries() {
    assert!(LIB.contains("dd_telemetry::http_trace_layer"));
    assert!(LIB.contains("messaging.publish"));
    assert!(LIB.contains("messaging.process"));
    assert!(LIB.contains(".route(\"/metrics\""));
    assert!(LIB.contains("dd_fabrication_server_persistence_enabled"));
    assert!(OBSERVABILITY.contains("persistence.enabled"));

    for deployment_component in ["OpenTelemetry", "Loki", "Prometheus"] {
        assert!(
            README.contains(deployment_component),
            "README must document the {deployment_component} integration"
        );
    }
}

#[test]
fn canonical_repository_and_submodule_boundary_is_documented() {
    assert!(README.contains("This repository is canonical"));
    assert!(README.contains("~/codes/ores/k8s-cluster/remote/deployments/fabrication-server-rs"));
    assert!(README.contains("secondary git submodule"));
}
