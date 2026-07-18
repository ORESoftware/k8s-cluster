//! Whole-tree isolation checks for the fabrication service modules.

use std::fs;
use std::path::{Path, PathBuf};

fn source_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

fn read_source(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
}

fn collect_rust_sources(directory: &Path, files: &mut Vec<PathBuf>) {
    let entries = fs::read_dir(directory)
        .unwrap_or_else(|error| panic!("read directory {}: {error}", directory.display()));
    for entry in entries {
        let path = entry.expect("read source directory entry").path();
        if path.is_dir() {
            collect_rust_sources(&path, files);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            files.push(path);
        }
    }
}

fn rust_sources() -> Vec<PathBuf> {
    let mut files = Vec::new();
    collect_rust_sources(&source_root(), &mut files);
    files.sort();
    files
}

fn relative_source_name(path: &Path) -> String {
    path.strip_prefix(source_root())
        .expect("source lives below src")
        .display()
        .to_string()
}

#[test]
fn every_declared_top_level_module_has_its_own_source() {
    let root = source_root();
    let library = read_source(&root.join("lib.rs"));
    let modules: Vec<&str> = library
        .lines()
        .filter_map(|line| {
            line.trim()
                .strip_prefix("mod ")
                .and_then(|declaration| declaration.strip_suffix(';'))
        })
        .collect();

    assert!(modules.len() >= 50, "expected the full module catalog");
    for module in modules {
        let file_module = root.join(format!("{module}.rs"));
        let directory_module = root.join(module).join("mod.rs");
        assert!(
            file_module.is_file() || directory_module.is_file(),
            "module {module} must live in {module}.rs or {module}/mod.rs"
        );
    }
}

#[test]
fn infrastructure_modules_remain_transport_agnostic() {
    let root = source_root();
    for module in [
        "config.rs",
        "metrics.rs",
        "observability.rs",
        "persistence.rs",
        "secrets.rs",
    ] {
        let source = read_source(&root.join(module));
        for transport_detail in ["axum::", "Router::", "IntoResponse", "StatusCode"] {
            assert!(
                !source.contains(transport_detail),
                "{module} must not depend on HTTP transport detail {transport_detail}"
            );
        }
    }
}

#[test]
fn seaorm_access_is_confined_to_the_persistence_module() {
    let users: Vec<String> = rust_sources()
        .into_iter()
        .filter(|path| read_source(path).contains("sea_orm::"))
        .map(|path| relative_source_name(&path))
        .collect();

    assert_eq!(users, ["persistence.rs"]);
}

#[test]
fn no_rust_source_bypasses_seaorm_with_sqlx() {
    for path in rust_sources() {
        assert!(
            !read_source(&path).contains("sqlx::"),
            "{} must not use SQLx directly",
            relative_source_name(&path)
        );
    }
}

#[test]
fn executable_and_telemetry_initialization_have_single_owners() {
    let mut entrypoints = Vec::new();
    let mut telemetry_initializers = Vec::new();

    for path in rust_sources() {
        let source = read_source(&path);
        if source.lines().any(|line| {
            let line = line.trim_start();
            line.starts_with("fn main(") || line.starts_with("async fn main(")
        }) {
            entrypoints.push(relative_source_name(&path));
        }
        if source.contains("dd_telemetry::init(") {
            telemetry_initializers.push(relative_source_name(&path));
        }
    }

    assert_eq!(entrypoints, ["main.rs"]);
    assert_eq!(telemetry_initializers, ["observability.rs"]);
}

#[test]
fn stateful_infrastructure_modules_keep_local_unit_tests() {
    let root = source_root();
    for module in ["config.rs", "persistence.rs", "secrets.rs"] {
        let source = read_source(&root.join(module));
        assert!(
            source.contains("#[cfg(test)]"),
            "{module} needs local tests"
        );
        assert!(source.contains("mod tests"), "{module} needs a test module");
    }
}
