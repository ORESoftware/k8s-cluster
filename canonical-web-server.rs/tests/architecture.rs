//! Structural contracts for keeping process bootstrap and runtime concerns modular.

use std::{fs, path::PathBuf};

fn manifest_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(relative: &str) -> String {
    let path = manifest_root().join(relative);
    fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()))
}

fn non_blank_lines(source: &str) -> usize {
    source
        .lines()
        .filter(|line| !line.trim().is_empty())
        .count()
}

#[test]
fn binary_and_library_roots_remain_declarative() {
    let main = read("src/main.rs");
    let library = read("src/lib.rs");

    assert!(
        non_blank_lines(&main) <= 24,
        "main.rs must remain bootstrap-only; move new behavior into a module"
    );
    assert!(
        non_blank_lines(&library) <= 30,
        "lib.rs must remain a module map and re-export surface"
    );

    for forbidden in [
        "axum::",
        "sea_orm",
        "Router",
        "TcpListener",
        "ConnectOptions",
        "Migrator",
    ] {
        assert!(
            !main.contains(forbidden),
            "main.rs crossed a runtime boundary by referencing {forbidden}"
        );
    }
    for forbidden in ["pub struct", "pub async fn", "pub fn", "impl "] {
        assert!(
            !library.contains(forbidden),
            "lib.rs contains implementation code ({forbidden}); move it into a focused module"
        );
    }

    assert!(main.contains("telemetry::init"));
    assert!(main.contains("command::run"));
    for module in ["app", "command", "database", "server", "telemetry"] {
        assert!(
            library.contains(&format!("pub mod {module};")),
            "lib.rs must expose the {module} boundary"
        );
    }
}

#[test]
fn runtime_responsibilities_stay_in_their_modules() {
    let app = read("src/app.rs");
    let command = read("src/command.rs");
    let database = read("src/database.rs");
    let server = read("src/server.rs");

    for required in [
        "pub struct AppState",
        "pub async fn build_state",
        "pub fn build_app",
    ] {
        assert!(app.contains(required), "app.rs is missing {required}");
    }
    assert!(app.contains("telemetry::instrument_http"));
    assert!(!app.contains("TcpListener"));
    assert!(!app.contains("Migrator::up"));

    assert!(command.contains("Some(\"migrate\")"));
    assert!(command.contains("Some(\"serve\")"));
    assert!(command.contains("database::run_migrations"));
    for forbidden in ["TcpListener", "ConnectOptions", "Router"] {
        assert!(
            !command.contains(forbidden),
            "command.rs must not own {forbidden}"
        );
    }

    for required in ["crate::db::connect_database", "Migrator::up"] {
        assert!(
            database.contains(required),
            "database.rs is missing {required}"
        );
    }
    for forbidden in ["Router", "TcpListener", "SupabaseAuth"] {
        assert!(
            !database.contains(forbidden),
            "database.rs must not own {forbidden}"
        );
    }

    for required in ["TcpListener::bind", "spawn_postgres_backplane"] {
        assert!(server.contains(required), "server.rs is missing {required}");
    }
    assert!(
        !server.contains("spawn_revocation_worker"),
        "the customer web process must not run the no-ingress revoker"
    );
    for forbidden in ["ConnectOptions", "Migrator::up", "SupabaseAuth::new"] {
        assert!(
            !server.contains(forbidden),
            "server.rs must not own {forbidden}"
        );
    }
}

#[test]
fn database_contract_is_seaorm_only_and_never_migrates_on_serve() {
    let cargo = read("Cargo.toml");
    let config = read("crates/canonical-config/src/lib.rs");
    let database = read("src/database.rs");
    let websocket = read("src/ws/mod.rs");

    let has_direct_sqlx_dependency = cargo.lines().any(|line| {
        line.split_once('=')
            .is_some_and(|(key, _)| key.trim().trim_matches(['\'', '"']) == "sqlx")
    });
    assert!(
        !has_direct_sqlx_dependency,
        "depend on SeaORM, never directly on sqlx"
    );
    assert!(cargo.contains("sea-orm ="));
    assert!(database.contains("crate::db::connect_database"));
    assert!(!database.contains("ConnectOptions"));
    assert!(!database.contains("Database::connect"));
    assert!(read("crates/canonical-store/src/lib.rs").contains("use sea_orm::"));

    // PostgreSQL LISTEN/NOTIFY is the one sanctioned driver escape, and it must
    // remain reached through SeaORM's re-export rather than a direct dependency.
    assert!(websocket.contains("sqlx::postgres::PgListener"));
    assert!(websocket.contains("sea_orm"));
    assert!(!websocket.contains("use sqlx::"));

    assert!(!config.contains("AUTO_MIGRATE"));
    assert!(!config.contains("auto_migrate"));
    assert!(database.contains("pub async fn run_migrations"));
    assert!(read("services/canonical-session-revoker/src/main.rs").contains("revoker.spawn()"));
}

#[test]
fn telemetry_contract_stays_explicit_and_low_cardinality() {
    let app = read("src/app.rs");
    let telemetry = read("src/telemetry.rs");

    for required in [
        "OTEL_EXPORTER_OTLP_ENDPOINT",
        "TraceContextPropagator",
        "http.server.request.count",
        "http.server.request.duration",
        "with_writer(std::io::stdout)",
        "resource_attribute_pairs",
    ] {
        assert!(
            telemetry.contains(required),
            "telemetry.rs is missing {required}"
        );
    }
    assert!(app.contains("telemetry::instrument_http"));
    assert!(!telemetry.contains("DATABASE_URL"));
    assert!(!telemetry.contains("Authorization"));
    assert!(!telemetry.contains("Cookie"));
}

#[test]
fn public_module_seams_compile() {
    let _ = canonical_web_server::app::build_app;
    let _ = canonical_web_server::app::build_state;
    let _ = canonical_web_server::command::run;
    let _ = canonical_web_server::database::connect;
    let _ = canonical_web_server::database::run_migrations;
    let _ = canonical_web_server::server::run;
    let _ = canonical_web_server::telemetry::init;
    let _ = canonical_web_server::telemetry::instrument_http;
}

#[test]
fn revoker_manifest_cannot_depend_on_the_customer_http_surface() {
    let manifest = read("services/canonical-session-revoker/Cargo.toml");
    for forbidden in ["canonical-web-server", "axum", "maud"] {
        assert!(
            !manifest.contains(forbidden),
            "the no-ingress revoker must not depend on {forbidden}"
        );
    }
    for required in [
        "canonical-auth",
        "canonical-config",
        "canonical-session",
        "canonical-store",
    ] {
        assert!(manifest.contains(required), "revoker is missing {required}");
    }
}

#[test]
fn browser_test_auth_is_debug_only_and_absent_from_production_images() {
    let manifest = read("Cargo.toml");
    let build = read("build.rs");
    let library = read("src/lib.rs");
    let app = read("src/app.rs");
    let dockerfile = read("Dockerfile");

    assert!(manifest.contains("test-auth = []"));
    assert!(build.contains("CARGO_FEATURE_TEST_AUTH"));
    assert!(build.contains("release_profile"));
    assert!(build.contains("the test-auth feature is forbidden in release builds"));
    assert!(library.contains("#[cfg(all(feature = \"test-auth\", not(debug_assertions)))]"));
    assert!(library
        .contains("compile_error!(\"the test-auth feature is forbidden in release builds\")"));
    assert!(app.contains("#[cfg(feature = \"test-auth\")]"));
    assert!(app.contains("BrowserTestAuth::is_enabled()"));
    assert!(
        !dockerfile.contains("--features test-auth"),
        "the production image must never compile browser test authentication"
    );
}

#[test]
fn app_repository_has_an_executable_release_boundary_contract() {
    let workflow = read(".github/workflows/container-contract.yml");
    let contract = read("tests/release-boundary.mjs");

    assert!(workflow.contains("permissions:\n  contents: read"));
    assert!(workflow.matches("tests/release-boundary.mjs").count() >= 3);
    assert!(workflow.contains("run: node tests/release-boundary.mjs"));
    assert!(contract.contains("release/current-containers.env"));
    assert!(contract.contains("canonical-monorepo is the sole release publisher"));
    assert!(contract.contains("docker\\/build-push-action"));
    assert!(contract.contains("(?:oras|crane|skopeo)"));
    assert!(contract.contains("\\.github\\/workflows\\/"));
}
