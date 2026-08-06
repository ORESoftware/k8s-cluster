use std::{
    any::TypeId,
    collections::{BTreeMap, BTreeSet, HashSet},
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::Arc,
    time::Duration,
};

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use canonical_auth::SupabaseAuth;
use canonical_config::{Config, MigrationConfig, SessionRevokerConfig};
use canonical_store::migration::Migrator;
use canonical_web_server::{build_app, AppState};
use sea_orm_migration::MigratorTrait;
use serde_json::Value;
use tower::ServiceExt;

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn workspace_metadata() -> Value {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo"));
    let output = Command::new(cargo)
        .args(["metadata", "--locked", "--no-deps", "--format-version", "1"])
        .current_dir(workspace_root())
        .output()
        .expect("cargo metadata must run");

    assert!(
        output.status.success(),
        "cargo metadata failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("cargo metadata must emit JSON")
}

fn package_map(metadata: &Value) -> BTreeMap<String, &Value> {
    metadata["packages"]
        .as_array()
        .expect("metadata packages must be an array")
        .iter()
        .map(|package| {
            (
                package["name"]
                    .as_str()
                    .expect("package must have a name")
                    .to_owned(),
                package,
            )
        })
        .collect()
}

fn internal_dependencies(package: &Value, internal: &BTreeSet<String>) -> BTreeSet<String> {
    package["dependencies"]
        .as_array()
        .expect("package dependencies must be an array")
        .iter()
        .filter_map(|dependency| dependency["name"].as_str())
        .filter(|name| internal.contains(*name))
        .map(str::to_owned)
        .collect()
}

fn dependency_names(package: &Value) -> BTreeSet<&str> {
    package["dependencies"]
        .as_array()
        .expect("package dependencies must be an array")
        .iter()
        .filter_map(|dependency| dependency["name"].as_str())
        .collect()
}

fn rust_sources_below(path: &Path, sources: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(path).unwrap_or_else(|error| {
        panic!("failed to read {}: {error}", path.display());
    }) {
        let entry = entry.expect("source directory entry must be readable");
        let path = entry.path();
        if path.is_dir() {
            rust_sources_below(&path, sources);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            sources.push(path);
        }
    }
}

fn all_workspace_sources() -> Vec<PathBuf> {
    let root = workspace_root();
    let mut sources = Vec::new();
    for directory in ["src", "crates", "services"] {
        rust_sources_below(&root.join(directory), &mut sources);
    }
    sources
}

fn marker_owners<'a>(sources: &'a [PathBuf], marker: &str) -> Vec<&'a PathBuf> {
    sources
        .iter()
        .filter(|path| {
            fs::read_to_string(path)
                .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()))
                .contains(marker)
        })
        .collect()
}

fn test_config() -> Config {
    Config {
        port: 8080,
        static_dir: PathBuf::from("public"),
        app_asset_dir: PathBuf::from("app"),
        database_url: "sqlite::memory:".to_owned(),
        database_max_connections: 1,
        app_base_url: "http://localhost:8080".to_owned(),
        allowed_origins: HashSet::from(["http://localhost:8080".to_owned()]),
        session_cookie: "canonical_session".to_owned(),
        cookie_secure: false,
        session_encryption_key: vec![7; 32],
        session_ttl: Duration::from_secs(3600),
        login_rate_limit_attempts: 5,
        login_rate_limit_global_attempts: 100,
        login_rate_limit_window: Duration::from_secs(60),
        login_rate_limit_max_keys: 1_000,
        login_auth_max_concurrency: 8,
        bearer_auth_max_concurrency: 16,
        supabase_url: "http://127.0.0.1:9999".to_owned(),
        supabase_publishable_key: "sb_publishable_refactor_parity".to_owned(),
    }
}

#[test]
fn workspace_metadata_has_exact_members_and_process_targets() {
    let metadata = workspace_metadata();
    let packages = package_map(&metadata);
    let expected = BTreeSet::from([
        "canonical-auth".to_owned(),
        "canonical-config".to_owned(),
        "canonical-session".to_owned(),
        "canonical-session-revoker".to_owned(),
        "canonical-store".to_owned(),
        "canonical-web-server".to_owned(),
    ]);
    assert_eq!(packages.keys().cloned().collect::<BTreeSet<_>>(), expected);

    let expected_process_targets = BTreeMap::from([
        (
            "canonical-web-server",
            vec!["canonical-api-server", "canonical-web-server"],
        ),
        (
            "canonical-session-revoker",
            vec!["canonical-session-revoker"],
        ),
    ]);
    for (package_name, expected_binaries) in expected_process_targets {
        let binaries = packages[package_name]["targets"]
            .as_array()
            .expect("package targets must be an array")
            .iter()
            .filter(|target| {
                target["kind"]
                    .as_array()
                    .is_some_and(|kinds| kinds.iter().any(|kind| kind == "bin"))
            })
            .filter_map(|target| target["name"].as_str())
            .collect::<Vec<_>>();
        assert_eq!(binaries, expected_binaries);
    }
}

#[test]
fn extracted_dependency_graph_remains_layered_and_seaorm_only() {
    let metadata = workspace_metadata();
    let packages = package_map(&metadata);
    let internal = packages.keys().cloned().collect::<BTreeSet<_>>();
    let expected_edges = BTreeMap::from([
        ("canonical-auth", BTreeSet::new()),
        ("canonical-config", BTreeSet::new()),
        ("canonical-store", BTreeSet::new()),
        (
            "canonical-session",
            BTreeSet::from(["canonical-auth".to_owned(), "canonical-store".to_owned()]),
        ),
        (
            "canonical-session-revoker",
            BTreeSet::from([
                "canonical-auth".to_owned(),
                "canonical-config".to_owned(),
                "canonical-session".to_owned(),
                "canonical-store".to_owned(),
            ]),
        ),
        (
            "canonical-web-server",
            BTreeSet::from([
                "canonical-auth".to_owned(),
                "canonical-config".to_owned(),
                "canonical-session".to_owned(),
                "canonical-store".to_owned(),
            ]),
        ),
    ]);

    for (package_name, expected) in expected_edges {
        assert_eq!(
            internal_dependencies(packages[package_name], &internal),
            expected,
            "unexpected internal dependency edge for {package_name}"
        );
    }

    for (package_name, package) in &packages {
        let dependencies = dependency_names(package);
        assert!(
            !dependencies.contains("sqlx"),
            "{package_name} must use SeaORM instead of depending directly on sqlx"
        );
        if package_name.as_str() != "canonical-web-server" {
            for web_dependency in ["axum", "axum-extra", "maud", "tower-http"] {
                assert!(
                    !dependencies.contains(web_dependency),
                    "{package_name} leaked web-layer dependency {web_dependency}"
                );
            }
        }
    }
}

#[test]
fn root_reexports_are_the_extracted_types() {
    assert_eq!(
        TypeId::of::<canonical_web_server::auth::AuthContext>(),
        TypeId::of::<canonical_auth::AuthContext>()
    );
    assert_eq!(
        TypeId::of::<canonical_web_server::auth::AuthTokens>(),
        TypeId::of::<canonical_auth::AuthTokens>()
    );
    assert_eq!(
        TypeId::of::<canonical_web_server::auth::SessionService>(),
        TypeId::of::<canonical_session::SessionService>()
    );
    assert_eq!(
        TypeId::of::<canonical_web_server::config::Config>(),
        TypeId::of::<canonical_config::Config>()
    );
    assert_eq!(
        TypeId::of::<canonical_web_server::db::AssuranceLevel>(),
        TypeId::of::<canonical_store::AssuranceLevel>()
    );
}

#[test]
fn core_definitions_have_single_owners_after_extraction() {
    let root = workspace_root();
    let sources = all_workspace_sources();
    let expected_owners = [
        (
            "pub struct SupabaseAuth",
            "crates/canonical-auth/src/lib.rs",
        ),
        ("pub struct Config", "crates/canonical-config/src/lib.rs"),
        (
            "pub struct SessionService",
            "crates/canonical-session/src/lib.rs",
        ),
        (
            "pub struct SessionRevoker {",
            "crates/canonical-session/src/lib.rs",
        ),
        (
            "pub async fn connect_database",
            "crates/canonical-store/src/lib.rs",
        ),
        (
            "pub struct Migrator",
            "crates/canonical-store/src/migration.rs",
        ),
    ];

    for (marker, expected_owner) in expected_owners {
        let owners = marker_owners(&sources, marker);
        assert_eq!(
            owners.len(),
            1,
            "{marker} must have one implementation owner"
        );
        assert_eq!(owners[0], &root.join(expected_owner));
    }

    for retired_path in [
        "src/auth/session.rs",
        "src/auth/supabase.rs",
        "src/db/mod.rs",
        "src/db/migration.rs",
    ] {
        assert!(
            !root.join(retired_path).exists(),
            "retired pre-extraction implementation returned at {retired_path}"
        );
    }
}

#[test]
fn extracted_process_configs_still_redact_secrets() {
    let mut config = test_config();
    config.database_url = "postgres://web-user:web-password@db/web".to_owned();
    config.session_encryption_key = b"web-session-encryption-secret".to_vec();
    config.supabase_publishable_key = "sb_publishable_web_secret".to_owned();
    let migration = MigrationConfig {
        database_url: "postgres://migration-user:migration-password@db/web".to_owned(),
        database_max_connections: 2,
    };
    let revoker = SessionRevokerConfig {
        database_url: "postgres://revoker-user:revoker-password@db/web".to_owned(),
        database_max_connections: 3,
        session_encryption_key: b"revoker-session-encryption-secret".to_vec(),
        supabase_url: "https://example.supabase.co".to_owned(),
        supabase_publishable_key: "sb_publishable_revoker_secret".to_owned(),
    };

    for (debug, secrets) in [
        (
            format!("{config:?}"),
            vec![
                "web-password",
                "web-session-encryption-secret",
                "sb_publishable_web_secret",
            ],
        ),
        (format!("{migration:?}"), vec!["migration-password"]),
        (
            format!("{revoker:?}"),
            vec![
                "revoker-password",
                "revoker-session-encryption-secret",
                "sb_publishable_revoker_secret",
            ],
        ),
    ] {
        assert!(debug.contains("[REDACTED]"));
        for secret in secrets {
            assert!(!debug.contains(secret), "debug output exposed {secret}");
        }
    }
}

#[tokio::test]
async fn extracted_components_compose_into_the_router() {
    let db = canonical_store::connect_database("sqlite::memory:", 1)
        .await
        .expect("in-memory SeaORM connection must open");
    Migrator::up(&db, None)
        .await
        .expect("modularized migrations must run");
    let auth = SupabaseAuth::new(
        "http://127.0.0.1:9999".to_owned(),
        "sb_publishable_refactor_parity".to_owned(),
    )
    .expect("test auth provider must construct without network I/O");
    let app = build_app(
        AppState::new(test_config(), db.clone(), Arc::new(auth))
            .expect("extracted services must compose into app state"),
    );

    let response = app
        .oneshot(
            Request::builder()
                .uri("/healthz")
                .body(Body::empty())
                .expect("health request must build"),
        )
        .await
        .expect("health route must respond");
    assert_eq!(response.status(), StatusCode::OK);

    db.close().await.expect("test database must close cleanly");
}
