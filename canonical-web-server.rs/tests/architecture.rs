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

    for required in ["ConnectOptions", "Database::connect", "Migrator::up"] {
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

    for required in [
        "TcpListener::bind",
        "spawn_postgres_backplane",
        "spawn_revocation_worker",
    ] {
        assert!(server.contains(required), "server.rs is missing {required}");
    }
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
    let config = read("src/config.rs");
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
    assert!(database.contains("use sea_orm::"));

    // PostgreSQL LISTEN/NOTIFY is the one sanctioned driver escape, and it must
    // remain reached through SeaORM's re-export rather than a direct dependency.
    assert!(websocket.contains("sqlx::postgres::PgListener"));
    assert!(websocket.contains("sea_orm"));
    assert!(!websocket.contains("use sqlx::"));

    assert!(!config.contains("AUTO_MIGRATE"));
    assert!(!config.contains("auto_migrate"));
    assert!(database.contains("pub async fn run_migrations"));
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
