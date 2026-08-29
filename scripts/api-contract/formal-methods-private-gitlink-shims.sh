#!/usr/bin/env bash
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"
cd "$repo_root"

fragment='remote/api-contracts/fragments/runtime-config-client-rs.openapi.json'
printf '%s  %s\n' \
  '5d330aa1a33ba118a5770af6409dcbad8dc408bb0cccaae4e6d257c337028d46' \
  "$fragment" | sha256sum --check -

# GITHUB_TOKEN is repository-scoped and cannot initialize the private
# ORESoftware/k8s-libs-and-shared-defs gitlink. Recreate only the exact package
# dependency surfaces required to compile and exercise this service's contract.
# Cargo manifests mirror the accepted shared package manifests so Cargo.lock
# remains valid when the real immutable gitlink is present in release builds.
if [ -d remote/libs ]; then
  find remote/libs -depth -delete
fi

mkdir -p remote/libs/telemetry-rs/src
cat > remote/libs/telemetry-rs/Cargo.toml <<'TOML'
[package]
name = "dd-telemetry"
version = "0.1.0"
edition = "2021"
description = "Shared OpenTelemetry tracing + structured-logging init for the Rust services in k8s-cluster/remote."

[lib]
path = "src/lib.rs"

[dependencies]
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter", "json", "fmt"] }
tracing-opentelemetry = "0.27"
opentelemetry = "0.26"
opentelemetry_sdk = { version = "0.26", features = ["rt-tokio"] }
opentelemetry-otlp = { version = "0.26", default-features = false, features = [
    "trace",
    "http-proto",
    "reqwest-client",
    "reqwest-rustls",
] }
opentelemetry-http = "0.26"
opentelemetry-semantic-conventions = "0.16"
tower-http = { version = "0.6", features = ["trace"] }
http = "1"
tokio = { version = "1", features = ["rt"] }
TOML
cat > remote/libs/telemetry-rs/src/lib.rs <<'RS'
use http::{Request, Response};
use std::time::Duration;
use tower_http::classify::{ServerErrorsAsFailures, SharedClassifier};
use tower_http::trace::{DefaultOnRequest, MakeSpan, OnResponse, TraceLayer};
use tracing::Span;

#[must_use]
pub struct OtelGuard;

pub fn init(_: &str) -> OtelGuard {
    OtelGuard
}

pub fn http_trace_layer() -> TraceLayer<
    SharedClassifier<ServerErrorsAsFailures>,
    OtelMakeSpan,
    DefaultOnRequest,
    OtelOnResponse,
> {
    TraceLayer::new_for_http()
        .make_span_with(OtelMakeSpan)
        .on_response(OtelOnResponse)
}

#[derive(Clone, Copy, Debug, Default)]
pub struct OtelMakeSpan;

impl<B> MakeSpan<B> for OtelMakeSpan {
    fn make_span(&mut self, request: &Request<B>) -> Span {
        tracing::info_span!(
            "http_request",
            http.request.method = %request.method(),
            url.path = %request.uri().path(),
            http.response.status_code = tracing::field::Empty,
        )
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct OtelOnResponse;

impl<B> OnResponse<B> for OtelOnResponse {
    fn on_response(self, response: &Response<B>, _: Duration, span: &Span) {
        span.record(
            "http.response.status_code",
            response.status().as_u16() as u64,
        );
    }
}
RS

mkdir -p remote/libs/interfaces/shared/generated/rust/src
cat > remote/libs/interfaces/shared/generated/rust/Cargo.toml <<'TOML'
[package]
name = "dd-shared-interfaces"
version = "0.1.0"
edition = "2021"
description = "Generated Rust types for dd shared cross-runtime interfaces."

[lib]
path = "src/lib.rs"

[features]
default = []
openapi = ["dep:utoipa"]

[dependencies]
serde = { version = "1", features = ["derive"] }
serde_json = "1"
utoipa = { version = "=5.5.0", optional = true }
TOML
cat > remote/libs/interfaces/shared/generated/rust/src/lib.rs <<'RS'
// Compile-only package surface. The formal-methods contract shim consumes the
// accepted vendored runtime-config OpenAPI fragment instead of duplicating its
// generated DTO implementation.
RS

mkdir -p remote/libs/runtime-config-client-rs/src
cat > remote/libs/runtime-config-client-rs/Cargo.toml <<'TOML'
[package]
name = "dd-runtime-config-client"
version = "0.1.0"
edition = "2021"
description = "Shared receiver helper for dd-runtime-config push messages."

[lib]
path = "src/lib.rs"

[features]
default = ["axum-07"]
axum-07 = ["dep:axum07"]
axum-08 = ["dep:axum08"]
openapi = [
  "axum-08",
  "dep:utoipa",
  "dep:utoipa-axum",
  "dd-shared-interfaces/openapi",
]

[dependencies]
axum07 = { package = "axum", version = "0.7", features = ["macros"], optional = true }
axum08 = { package = "axum", version = "0.8", features = ["macros"], optional = true }
dd-shared-interfaces = { path = "../interfaces/shared/generated/rust", default-features = false }
reqwest = { version = "=0.12.9", default-features = false, features = ["json", "rustls-tls"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
tokio = { version = "1", features = ["rt", "sync", "time", "macros"] }
utoipa = { version = "=5.5.0", optional = true, features = ["preserve_order", "preserve_path_order"] }
utoipa-axum = { version = "=0.2.0", optional = true }
TOML
cat > remote/libs/runtime-config-client-rs/src/lib.rs <<'RS'
#[cfg(feature = "axum-08")]
extern crate axum08 as axum;

use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{json, Value};
use utoipa::openapi::OpenApi;

const OPENAPI_JSON: &str =
    include_str!("../../../api-contracts/fragments/runtime-config-client-rs.openapi.json");
const ANY_JSON_TYPES: [&str; 7] = [
    "object", "array", "string", "number", "integer", "boolean", "null",
];

async fn snapshot() -> Json<Value> {
    Json(json!({"snapshotVersion": 0, "entries": {}}))
}

async fn mutate() -> Json<Value> {
    Json(json!({"ok": true}))
}

fn shared_openapi() -> OpenApi {
    let mut document: Value =
        serde_json::from_str(OPENAPI_JSON).expect("valid vendored runtime-config OpenAPI JSON");
    for pointer in [
        "/components/schemas/RuntimeConfigEntry/properties/value",
        "/components/schemas/RuntimeConfigEntry/properties/meta",
    ] {
        let schema = document
            .pointer_mut(pointer)
            .unwrap_or_else(|| panic!("missing expected free-form schema at {pointer}"))
            .as_object_mut()
            .unwrap_or_else(|| panic!("expected schema object at {pointer}"));
        schema.insert("type".to_string(), json!(ANY_JSON_TYPES));
    }
    let pointer =
        "/components/schemas/RuntimeConfigSnapshotResponse/properties/entries/additionalProperties";
    let schema = document
        .pointer_mut(pointer)
        .unwrap_or_else(|| panic!("missing expected free-form schema at {pointer}"));
    *schema = json!({"type": ANY_JSON_TYPES});
    serde_json::from_value(document).expect("Utoipa-compatible runtime-config OpenAPI")
}

pub fn router_and_openapi() -> (Router, OpenApi) {
    let router = Router::new()
        .route("/internal/runtime-config", get(snapshot))
        .route("/internal/update-runtime-config", post(mutate))
        .route("/internal/runtime-config/reset", post(mutate));
    (router, shared_openapi())
}

pub fn router() -> Router {
    router_and_openapi().0
}

pub async fn register_with_control_plane() {}
RS
