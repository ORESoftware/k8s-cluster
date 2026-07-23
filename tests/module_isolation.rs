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

fn source_owners(fragment: &str) -> Vec<String> {
    rust_sources()
        .into_iter()
        .filter(|path| read_source(path).contains(fragment))
        .map(|path| relative_source_name(&path))
        .collect()
}

fn module_files(directory: &str) -> Vec<String> {
    let directory = source_root().join(directory);
    let mut files: Vec<String> = fs::read_dir(&directory)
        .unwrap_or_else(|error| panic!("read module directory {}: {error}", directory.display()))
        .filter_map(|entry| {
            let path = entry.expect("read module entry").path();
            (path.extension().is_some_and(|extension| extension == "rs")).then(|| {
                path.file_name()
                    .expect("module file name")
                    .to_string_lossy()
                    .into_owned()
            })
        })
        .collect();
    files.sort();
    files
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
        "stores.rs",
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

/// SeaORM stays inside the persistence layer, which is exactly two files.
///
/// `persistence.rs` owns the pool and `stores.rs` owns the two tables behind
/// the `JobStore`/`LearningStore` traits. Nothing else may name `sea_orm::` —
/// the point of those traits is that the ~120 planning call sites talk to a
/// store, not to an ORM, and a third entry in this list means that boundary
/// has been crossed rather than extended.
#[test]
fn seaorm_access_is_confined_to_the_persistence_layer() {
    let users: Vec<String> = rust_sources()
        .into_iter()
        .filter(|path| read_source(path).contains("sea_orm::"))
        .map(|path| relative_source_name(&path))
        .collect();

    assert_eq!(users, ["persistence.rs", "stores.rs"]);
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
    for module in [
        "config.rs",
        "persistence.rs",
        "secrets.rs",
        "shared_auth.rs",
        "stores.rs",
    ] {
        let source = read_source(&root.join(module));
        assert!(
            source.contains("#[cfg(test)]"),
            "{module} needs local tests"
        );
        assert!(source.contains("mod tests"), "{module} needs a test module");
    }
}

#[test]
fn protocol_implementations_have_exactly_one_source_owner() {
    for (fragment, expected_owner) in [
        ("dd_telemetry::init(", "observability.rs"),
        ("use shared_auth_lib::", "shared_auth.rs"),
        (
            "impl FromRequestParts<AppState> for Operator",
            "shared_auth.rs",
        ),
        ("async_nats::ConnectOptions::new(", "messaging.rs"),
        ("WebSocketUpgrade", "transport/websocket.rs"),
        ("use maud::", "transport/views.rs"),
        (
            ".route(\"/printing/preflight\"",
            "additive_printing/http.rs",
        ),
        (".route(\"/ws/html\"", "transport/mod.rs"),
        (".route(\"/ws/json\"", "transport/mod.rs"),
    ] {
        assert_eq!(
            source_owners(fragment),
            [expected_owner],
            "{fragment} must have one deliberate module owner"
        );
    }

    assert_eq!(
        source_owners("Database::connect("),
        ["persistence.rs", "stores.rs"],
        "database connections stay inside the two-file SeaORM persistence boundary"
    );
}

#[test]
fn shared_transport_and_realtime_layers_do_not_depend_on_fab_application_state() {
    let realtime = read_source(&source_root().join("realtime.rs"));
    for forbidden in [
        "axum::",
        "async_nats::",
        "maud::",
        "sea_orm::",
        "AppState",
        "WebState",
    ] {
        assert!(
            !realtime.contains(forbidden),
            "realtime.rs must remain transport-neutral and cannot depend on {forbidden}"
        );
    }

    for path in rust_sources()
        .into_iter()
        .filter(|path| relative_source_name(path).starts_with("transport/"))
    {
        let name = relative_source_name(&path);
        let source = read_source(&path);
        for forbidden in [
            "AppState",
            "WebState",
            "plan_http",
            "JobStore",
            "LearningStore",
            "additive_printing",
        ] {
            assert!(
                !source.contains(forbidden),
                "{name} must remain reusable by both processes and cannot depend on {forbidden}"
            );
        }
    }
}

#[test]
fn web_process_does_not_absorb_fab_domain_implementation() {
    for path in rust_sources()
        .into_iter()
        .filter(|path| relative_source_name(path).starts_with("web_server/"))
    {
        let name = relative_source_name(&path);
        let source = read_source(&path);
        for forbidden in [
            "AppState",
            "authenticated_router",
            "plan_http",
            "additive_printing::",
            "InMemoryJobStore",
            "LearningStore",
        ] {
            assert!(
                !source.contains(forbidden),
                "{name} crossed the web/fab service boundary through {forbidden}"
            );
        }
    }
}

#[test]
fn feature_directories_have_an_explicit_complete_module_inventory() {
    assert_eq!(
        module_files("additive_printing"),
        ["analysis.rs", "http.rs", "mod.rs", "model.rs"]
    );
    assert_eq!(
        module_files("transport"),
        ["mod.rs", "nats.rs", "tcp.rs", "views.rs", "websocket.rs"]
    );
    assert_eq!(
        module_files("web_server"),
        [
            "backend.rs",
            "config.rs",
            "http.rs",
            "mod.rs",
            "supabase.rs"
        ]
    );
}

#[test]
fn extracted_behavioral_modules_keep_tests_beside_the_behavior() {
    let root = source_root();
    for module in [
        "http.rs",
        "realtime.rs",
        "transport/mod.rs",
        "transport/nats.rs",
        "transport/tcp.rs",
        "transport/views.rs",
        "transport/websocket.rs",
        "web_server/mod.rs",
        "web_server/backend.rs",
        "web_server/config.rs",
        "web_server/http.rs",
        "web_server/supabase.rs",
        "additive_printing/analysis.rs",
        "additive_printing/http.rs",
    ] {
        let source = read_source(&root.join(module));
        assert!(
            source.contains("#[cfg(test)]"),
            "{module} needs local tests"
        );
        assert!(source.contains("mod tests"), "{module} needs a test module");
    }
}
