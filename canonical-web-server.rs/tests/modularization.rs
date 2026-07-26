//! Cross-module invariants that complement the focused architecture contracts.

use std::{
    fs,
    path::{Path, PathBuf},
};

use canonical_web_server::{database, db::migration::Migrator};
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};
use sea_orm_migration::MigratorTrait;

fn manifest_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(relative: &str) -> String {
    let path = manifest_root().join(relative);
    fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()))
}

fn collect_rust_sources(directory: &Path, sources: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(directory)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", directory.display()))
    {
        let path = entry
            .expect("source directory entry must be readable")
            .path();
        if path.is_dir() {
            collect_rust_sources(&path, sources);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            sources.push(path);
        }
    }
}

fn rust_sources() -> Vec<PathBuf> {
    let mut sources = Vec::new();
    collect_rust_sources(&manifest_root().join("src"), &mut sources);
    sources.sort();
    sources
}

fn source_name(path: &Path) -> String {
    path.strip_prefix(manifest_root())
        .expect("source must be under the crate root")
        .to_string_lossy()
        .into_owned()
}

fn owners(marker: &str) -> Vec<String> {
    rust_sources()
        .into_iter()
        .filter(|path| {
            fs::read_to_string(path)
                .expect("Rust source must be readable")
                .contains(marker)
        })
        .map(|path| source_name(&path))
        .collect()
}

fn assert_single_owner(marker: &str, expected: &str) {
    assert_eq!(
        owners(marker),
        [expected],
        "{marker:?} must have exactly one architectural owner"
    );
}

#[test]
fn every_top_level_module_is_registered_exactly_once() {
    let src = manifest_root().join("src");
    let mut discovered = fs::read_dir(&src)
        .expect("src directory must be readable")
        .filter_map(|entry| {
            let path = entry.expect("src entry must be readable").path();
            if path.is_dir() && path.join("mod.rs").is_file() {
                return path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned());
            }
            if path.extension().is_some_and(|extension| extension == "rs") {
                let stem = path.file_stem()?.to_string_lossy();
                if stem != "lib" && stem != "main" {
                    return Some(stem.into_owned());
                }
            }
            None
        })
        .collect::<Vec<_>>();
    discovered.sort();

    let mut declared = read("src/lib.rs")
        .lines()
        .filter_map(|line| {
            line.trim()
                .strip_prefix("pub mod ")
                .and_then(|module| module.strip_suffix(';'))
                .map(str::to_owned)
        })
        .collect::<Vec<_>>();
    declared.sort();

    assert_eq!(declared, discovered, "lib.rs and src/ modules diverged");
}

#[test]
fn runtime_side_effects_keep_exactly_one_owner() {
    for (marker, owner) in [
        ("telemetry::init", "src/main.rs"),
        ("Config::from_env", "src/command.rs"),
        ("MigrationConfig::from_env", "src/command.rs"),
        ("crate::db::connect_database", "src/database.rs"),
        ("Migrator::up", "src/database.rs"),
        ("database::connect", "src/app.rs"),
        ("app::build_state", "src/server.rs"),
        ("app::build_app", "src/server.rs"),
        ("SocketAddr::from(([0, 0, 0, 0], port))", "src/server.rs"),
        ("with_graceful_shutdown(shutdown_signal())", "src/server.rs"),
    ] {
        assert_single_owner(marker, owner);
    }
}

#[test]
fn infrastructure_dependencies_only_point_inward() {
    let telemetry = read("src/telemetry.rs");
    assert!(
        !telemetry.contains("crate::"),
        "telemetry must remain reusable and independent from application modules"
    );

    let database = read("src/database.rs");
    for forbidden in [
        "auth::",
        "routes::",
        "server::",
        "telemetry::",
        "ws::",
        "axum::",
    ] {
        assert!(
            !database.contains(forbidden),
            "database infrastructure depends upward on {forbidden}"
        );
    }

    let command = read("src/command.rs");
    for forbidden in ["axum::", "sea_orm", "tokio::", "auth::", "routes::"] {
        assert!(
            !command.contains(forbidden),
            "command dispatch owns runtime implementation detail {forbidden}"
        );
    }
}

#[test]
fn postgres_driver_escape_is_confined_to_the_seaorm_reexport() {
    assert_single_owner("sqlx::", "src/ws/mod.rs");
    for path in rust_sources() {
        for (index, line) in fs::read_to_string(&path)
            .expect("Rust source must be readable")
            .lines()
            .enumerate()
        {
            if line.contains("sqlx::") {
                assert!(
                    line.contains("sea_orm"),
                    "{}:{} bypasses SeaORM: {line}",
                    source_name(&path),
                    index + 1
                );
            }
        }
    }
}

async fn application_tables(database: &DatabaseConnection) -> Vec<String> {
    database
        .query_all_raw(Statement::from_string(
            DatabaseBackend::Sqlite,
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
        ))
        .await
        .expect("SQLite catalog query must succeed")
        .into_iter()
        .map(|row| row.try_get("", "name").expect("table name must be text"))
        .collect()
}

#[tokio::test]
async fn runtime_connection_and_schema_migration_are_independent() {
    let database = database::connect("sqlite::memory:", 1)
        .await
        .expect("runtime connection must open");

    assert_eq!(
        application_tables(&database).await,
        Vec::<String>::new(),
        "opening the runtime pool must never apply migrations"
    );

    Migrator::up(&database, None)
        .await
        .expect("explicit migration must succeed");
    let tables = application_tables(&database).await;
    for expected in ["audit_engagement", "sync_record", "web_session"] {
        assert!(
            tables.iter().any(|table| table == expected),
            "explicit migration did not create {expected}; created {tables:?}"
        );
    }

    database.close().await.expect("database must close cleanly");
}
