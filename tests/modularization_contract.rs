//! Architecture contracts for the fabrication service.
//!
//! These tests intentionally inspect source layout: modularity is a property of
//! the crate's composition, not only of its runtime behavior.

const MAIN: &str = include_str!("../src/main.rs");
const WEB_MAIN: &str = include_str!("../src/bin/dd-fabrication-web-server.rs");
const LIB: &str = include_str!("../src/lib.rs");
const ADDITIVE: &str = include_str!("../src/additive_printing/mod.rs");
const CONFIG: &str = include_str!("../src/config.rs");
const HTTP: &str = include_str!("../src/http.rs");
const MESSAGING: &str = include_str!("../src/messaging.rs");
const METRICS: &str = include_str!("../src/metrics.rs");
const OBSERVABILITY: &str = include_str!("../src/observability.rs");
const PERSISTENCE: &str = include_str!("../src/persistence.rs");
const REALTIME: &str = include_str!("../src/realtime.rs");
const SECRETS: &str = include_str!("../src/secrets.rs");
const SHARED_AUTH: &str = include_str!("../src/shared_auth.rs");
const TRANSPORT: &str = include_str!("../src/transport/mod.rs");
const WEB_SERVER: &str = include_str!("../src/web_server/mod.rs");
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

    assert!(nonempty_line_count(WEB_MAIN) <= 5);
    assert!(WEB_MAIN.contains("dd_fabrication_server::run_web().await"));
    for implementation_detail in ["Router::new", "sea_orm", "async_nats", "dd_telemetry"] {
        assert!(
            !WEB_MAIN.contains(implementation_detail),
            "web main.rs must not own {implementation_detail}"
        );
    }
}

#[test]
fn runtime_concerns_have_dedicated_modules() {
    for module in [
        "config",
        "http",
        "messaging",
        "metrics",
        "observability",
        "persistence",
        "realtime",
        "secrets",
        "shared_auth",
        "transport",
        "web_server",
    ] {
        assert!(
            LIB.lines()
                .any(|line| line.trim() == format!("mod {module};")),
            "lib.rs must compose the {module} module"
        );
    }

    assert!(CONFIG.contains("struct ServiceConfig"));
    assert!(HTTP.contains("async fn healthz"));
    assert!(HTTP.contains("async fn readyz"));
    assert!(HTTP.contains("async fn metrics"));
    assert!(METRICS.contains("struct Metrics"));
    assert!(MESSAGING.contains("connect_optional"));
    assert!(OBSERVABILITY.contains("dd_telemetry::init"));
    assert!(PERSISTENCE.contains("enum Persistence"));
    assert!(REALTIME.contains("struct EventEnvelope"));
    assert!(TRANSPORT.contains("pub(crate) fn router"));
    assert!(WEB_SERVER.contains("pub(crate) async fn run"));
    assert!(SECRETS.contains("struct SecretOverlay"));
    assert!(SHARED_AUTH.contains("struct SharedAuthVerifier"));
    assert!(LIB.contains("pub async fn run()"));
    assert!(LIB.contains("pub async fn run_web()"));

    for handler in ["healthz", "readyz", "metrics"] {
        assert!(
            !LIB.contains(&format!("async fn {handler}")),
            "lib.rs must delegate the {handler} HTTP adapter"
        );
        assert!(LIB.contains(&format!("get(http::{handler})")));
    }
}

#[test]
fn library_runtime_entrypoint_is_public() {
    let _entrypoint = dd_fabrication_server::run;
    let _web_entrypoint = dd_fabrication_server::run_web;
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
fn shared_auth_owns_both_complete_fail_closed_http_gates() {
    assert!(has_direct_dependency(MANIFEST, "shared-auth-lib"));
    assert!(!has_direct_dependency(MANIFEST, "jsonwebtoken"));
    assert!(SHARED_AUTH.contains("AuthGuard"));
    assert!(SHARED_AUTH.contains("AuthOutcome::Degraded"));
    assert!(SHARED_AUTH.contains("async fn authorize_bearer"));
    assert!(SHARED_AUTH.contains("async fn require_operator"));
    assert!(SHARED_AUTH.contains("impl FromRequestParts<AppState> for Operator"));
    assert!(SHARED_AUTH.contains("#[tracing::instrument("));
    assert!(SHARED_AUTH.contains("auth.authorization.succeeded"));
    assert!(WEB_SERVER.contains("authorize_bearer"));
    assert!(WEB_SERVER.contains("async fn require_operator"));
    assert!(WEB_SERVER.contains("route_layer(middleware::from_fn_with_state"));
    assert!(WEB_SERVER.contains("auth.authorization.succeeded"));
    assert!(!LIB.contains("impl FromRequestParts<AppState> for Operator"));
    assert!(!std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src/supabase_auth.rs")
        .exists());
}

#[test]
fn both_processes_share_mash_websocket_tcp_and_nats_contracts() {
    assert!(has_direct_dependency(MANIFEST, "maud"));
    assert!(has_direct_dependency(MANIFEST, "async-nats"));
    assert!(MANIFEST.contains("features = [\"macros\", \"ws\"]"));
    assert!(TRANSPORT.contains("/ws/html"));
    assert!(TRANSPORT.contains("/ws/json"));
    assert!(TRANSPORT.contains("spawn_publisher"));
    assert!(TRANSPORT.contains("serve_tcp"));
    assert!(REALTIME.contains("ServiceSurface::Fabrication"));
    assert!(REALTIME.contains("ServiceSurface::Web"));
    assert!(LIB.contains("transport::router(realtime_hub"));
    assert!(WEB_SERVER.contains("transport::router(hub, ServiceSurface::Web)"));
}

#[test]
fn schema_migrations_stay_declarative_and_outside_application_startup() {
    assert!(PERSISTENCE.contains("dd_fabrication_server/schema.sql"));
    assert!(PERSISTENCE.contains("SUPABASE_DATABASE_URL"));
    for forbidden_runtime_ddl in [
        "Schema::create_table",
        "Schema::drop_table",
        "execute_unprepared(\"CREATE",
        "execute_unprepared(\"ALTER",
        "execute_unprepared(\"DROP",
    ] {
        assert!(!PERSISTENCE.contains(forbidden_runtime_ddl));
        assert!(!LIB.contains(forbidden_runtime_ddl));
    }
}

#[test]
fn additive_printing_is_a_modular_feature_slice() {
    assert!(LIB.contains("mod additive_printing;"));
    assert!(LIB.contains("additive_printing::router(realtime_hub.clone())"));
    for module in ["analysis", "http", "model"] {
        assert!(ADDITIVE.contains(&format!("mod {module};")));
    }
    assert!(!LIB.contains("fn analyze_fdm"));
    assert!(!LIB.contains("fn analyze_resin"));
}

#[test]
fn observability_is_composed_at_service_boundaries() {
    assert!(LIB.contains("dd_telemetry::http_trace_layer"));
    assert!(LIB.contains("messaging.publish"));
    assert!(LIB.contains("messaging.process"));
    assert!(LIB.contains(".route(\"/metrics\""));
    assert!(HTTP.contains("dd_fabrication_server_persistence_enabled"));
    assert!(OBSERVABILITY.contains("persistence.enabled"));
    assert!(OBSERVABILITY.contains("auth.system = \"shared-auth\""));
    assert!(OBSERVABILITY.contains("shared-auth+supabase"));
    assert!(OBSERVABILITY.contains("telemetry.logs = \"stdout/loki\""));
    assert!(OBSERVABILITY.contains("telemetry.traces = \"otlp\""));
    assert!(OBSERVABILITY.contains("telemetry.metrics = \"prometheus\""));

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
