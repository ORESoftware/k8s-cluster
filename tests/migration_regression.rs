//! Regression contracts that remain valid before and after declarative migrations.

use std::{fs, path::Path};

const MANIFEST: &str = include_str!("../Cargo.toml");
const LIB: &str = include_str!("../src/lib.rs");
const PERSISTENCE: &str = include_str!("../src/persistence.rs");
const REALTIME: &str = include_str!("../src/realtime.rs");

fn has_direct_dependency(dependency: &str) -> bool {
    let assignment = format!("{dependency} =");
    MANIFEST
        .lines()
        .any(|line| line.trim_start().starts_with(&assignment))
}

#[test]
fn fabrication_never_borrows_the_build_server_database_contract() {
    let source_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    for entry in walk(&source_root) {
        let source = fs::read_to_string(&entry).expect("read Rust source");
        assert!(
            !source.contains("dd_build_server"),
            "{} must not use the build-server bounded context",
            entry.display()
        );
    }
    assert!(PERSISTENCE.contains("dd_fabrication_server/schema.sql"));
}

#[test]
fn application_startup_connects_but_never_migrates() {
    assert!(PERSISTENCE.contains("Database::connect"));
    assert!(PERSISTENCE.contains("DatabaseConnection"));
    assert!(PERSISTENCE.contains("SUPABASE_DATABASE_URL"));
    assert!(!has_direct_dependency("sqlx"));

    for migration_operation in [
        "Migrator::up",
        "Schema::create_table",
        "Schema::drop_table",
        "execute_unprepared",
        "CREATE TABLE",
        "ALTER TABLE",
        "DROP TABLE",
    ] {
        assert!(!PERSISTENCE.contains(migration_operation));
        assert!(!LIB.contains(migration_operation));
    }
}

#[test]
fn realtime_contract_is_versioned_and_payload_forward_compatible() {
    assert!(REALTIME.contains("dd.fabrication.realtime.v1"));
    assert!(REALTIME.contains("payload: Value"));
    assert!(REALTIME.contains("#[serde(rename_all = \"camelCase\")]"));
    assert!(REALTIME.contains("unknown_payload_fields"));
}

fn walk(directory: &Path) -> Vec<std::path::PathBuf> {
    let mut files = Vec::new();
    for entry in fs::read_dir(directory).expect("read source directory") {
        let path = entry.expect("read source entry").path();
        if path.is_dir() {
            files.extend(walk(&path));
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            files.push(path);
        }
    }
    files
}
