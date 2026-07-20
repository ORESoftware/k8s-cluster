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
fn executable_processes_are_thin_and_telemetry_has_one_owner() {
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

    assert_eq!(entrypoints, ["bin/dd-fabrication-web-server.rs", "main.rs"]);
    assert_eq!(telemetry_initializers, ["observability.rs"]);
}

#[test]
fn each_process_has_one_health_readiness_and_metrics_adapter() {
    for handler in ["healthz", "readyz", "metrics"] {
        let definition = format!("async fn {handler}");
        let owners: Vec<String> = rust_sources()
            .into_iter()
            .filter(|path| read_source(path).contains(&definition))
            .map(|path| relative_source_name(&path))
            .collect();
        assert_eq!(
            owners,
            ["http.rs", "web_server/http.rs"],
            "each process needs one owner for {handler}"
        );
    }

    let http = read_source(&source_root().join("http.rs"));
    assert!(http.contains("#[cfg(test)]"));
    assert!(http.contains("dd_fabrication_server_persistence_enabled"));
}

#[test]
fn websocket_and_maud_server_details_stay_in_transport_modules() {
    for path in rust_sources() {
        let name = relative_source_name(&path);
        let source = read_source(&path);
        if source.contains("WebSocketUpgrade") {
            assert_eq!(name, "transport/websocket.rs");
        }
        if source.contains("use maud::") {
            assert_eq!(name, "transport/views.rs");
        }
    }
}

#[test]
fn additive_analysis_is_independent_of_frameworks_and_infrastructure() {
    let analysis = read_source(&source_root().join("additive_printing/analysis.rs"));
    for forbidden_dependency in ["axum::", "async_nats::", "sea_orm::", "reqwest::"] {
        assert!(
            !analysis.contains(forbidden_dependency),
            "additive analysis must not depend on {forbidden_dependency}"
        );
    }

    let additive = read_source(&source_root().join("additive_printing/mod.rs"));
    for module in ["analysis", "http", "model"] {
        assert!(additive.contains(&format!("mod {module};")));
    }
}

#[test]
fn runtime_sources_do_not_own_database_ddl() {
    for path in rust_sources() {
        let source = read_source(&path);
        for ddl in ["CREATE TABLE", "ALTER TABLE", "DROP TABLE"] {
            assert!(
                !source.contains(ddl),
                "{} contains runtime DDL {ddl}",
                relative_source_name(&path)
            );
        }
    }
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
