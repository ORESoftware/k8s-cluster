//! Regression tests for the service's modular architecture.

use std::{fs, future::Future, path::PathBuf};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(relative: &str) -> String {
    fs::read_to_string(root().join(relative))
        .unwrap_or_else(|error| panic!("failed to read {relative}: {error}"))
}

fn rust_sources() -> String {
    let mut paths = fs::read_dir(root().join("src"))
        .expect("src directory must exist")
        .map(|entry| entry.expect("src entry must be readable").path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "rs"))
        .collect::<Vec<_>>();
    paths.sort();
    paths
        .into_iter()
        .map(|path| fs::read_to_string(path).expect("Rust source must be readable"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn assert_send_unit_future<F>(future: F)
where
    F: Future<Output = ()> + Send + 'static,
{
    drop(future);
}

#[test]
fn binary_entrypoint_stays_thin() {
    let main = read("src/main.rs");
    let meaningful_lines = main.lines().filter(|line| !line.trim().is_empty()).count();
    assert!(
        meaningful_lines <= 8,
        "main.rs grew to {meaningful_lines} lines"
    );
    assert!(main.contains("dd_sound_recorder_rs::run().await"));
    for forbidden in ["mod ", "Router::new", "Database::connect", "fn app("] {
        assert!(
            !main.contains(forbidden),
            "main.rs must not contain application concern {forbidden:?}"
        );
    }
}

#[test]
fn library_keeps_process_infrastructure_in_named_modules() {
    let library = read("src/lib.rs");
    for declaration in ["mod database;", "mod service;", "mod telemetry;"] {
        assert!(
            library.contains(declaration),
            "missing module declaration {declaration}"
        );
    }
    assert!(library.contains("pub use service::run;"));
    assert!(read("src/service.rs").contains("pub async fn run()"));
}

#[test]
fn exported_run_is_a_sendable_library_future() {
    assert_send_unit_future(dd_sound_recorder_rs::run());
}

#[test]
fn tokio_process_entrypoint_exists_only_in_the_binary() {
    let sources = rust_sources();
    assert_eq!(
        sources.matches("#[tokio::main]").count(),
        1,
        "the Tokio process macro must not leak into library modules"
    );
    assert!(read("src/main.rs").contains("#[tokio::main]"));
}

#[test]
fn connection_and_exporter_setup_stay_out_of_domain_service() {
    let service = read("src/service.rs");
    for forbidden in [
        "Database::connect",
        "ConnectOptions::new",
        "tracing_subscriber::",
        "opentelemetry_otlp::",
    ] {
        assert!(
            !service.contains(forbidden),
            "domain service took ownership of infrastructure setup {forbidden}"
        );
    }
    assert!(read("src/database.rs").contains("Database::connect"));
    assert!(read("src/telemetry.rs").contains("opentelemetry_otlp::"));
}

#[test]
fn telemetry_remains_a_dependency_leaf() {
    let telemetry = read("src/telemetry.rs");
    for forbidden in [
        "crate::database",
        "crate::service",
        "sea_orm::",
        "Router::new",
        "AppState",
    ] {
        assert!(
            !telemetry.contains(forbidden),
            "telemetry depends on application concern {forbidden}"
        );
    }
}

#[test]
fn application_persistence_is_seaorm_only() {
    let manifest = read("Cargo.toml");
    assert!(manifest.contains("sea-orm ="));
    for direct_dependency in ["sqlx =", "tokio-postgres =", "bb8-postgres ="] {
        assert!(
            !manifest
                .lines()
                .any(|line| line.trim_start().starts_with(direct_dependency)),
            "forbidden direct persistence dependency {direct_dependency}"
        );
    }

    let sources = rust_sources();
    for forbidden_api in ["sqlx::", "tokio_postgres", "bb8_postgres"] {
        assert!(
            !sources.contains(forbidden_api),
            "application source imports forbidden persistence API {forbidden_api}"
        );
    }
    let database = read("src/database.rs");
    for seaorm_boundary in [
        "Database::connect",
        "ConnectionTrait",
        "TransactionTrait",
        ".connect_lazy(true)",
    ] {
        assert!(
            database.contains(seaorm_boundary),
            "SeaORM boundary lost {seaorm_boundary}"
        );
    }
}

#[test]
fn telemetry_keeps_otel_loki_and_cardinality_guards() {
    let telemetry = read("src/telemetry.rs");
    for contract in [
        "OTEL_EXPORTER_OTLP_ENDPOINT",
        ".json()",
        "with_writer(std::io::stderr)",
        "record_http_request",
        "http.server.request.duration",
        "sensitive_attribute_key",
    ] {
        assert!(telemetry.contains(contract), "telemetry lost {contract}");
    }
    let service = read("src/service.rs");
    assert!(service.contains("MatchedPath"));
    assert!(service.contains("crate::telemetry::record_http_request"));
}
