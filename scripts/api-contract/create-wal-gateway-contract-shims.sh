#!/usr/bin/env bash
set -euo pipefail

fragment='remote/api-contracts/fragments/runtime-config-client-rs.openapi.json'
printf '%s  %s\n' \
  '5d330aa1a33ba118a5770af6409dcbad8dc408bb0cccaae4e6d257c337028d46' \
  "$fragment" | sha256sum --check -

# GitHub's per-repository GITHUB_TOKEN cannot initialize the private
# cross-repository remote/libs gitlink. These shims expose only the compile
# surfaces needed by wal-gateway. The real runtime-config implementation and
# the exact generated fragment are tested in k8s-libs-and-shared-defs; this
# repository pins that merged commit and verifies the fragment SHA-256.
rm -rf remote/libs

mkdir -p remote/libs/telemetry-rs/src
cat > remote/libs/telemetry-rs/Cargo.toml <<'TOML'
[package]
name = "dd-telemetry"
version = "0.1.0"
edition = "2021"

[lib]
path = "src/lib.rs"

[dependencies]
tracing = "0.1"
tower-http = { version = "0.6", features = ["trace"] }
http = "1"
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

mkdir -p remote/libs/nats/subject-defs/generated/rust/src
cat > remote/libs/nats/subject-defs/generated/rust/Cargo.toml <<'TOML'
[package]
name = "dd-nats-subject-defs"
version = "0.1.0"
edition = "2021"

[lib]
path = "src/lib.rs"
TOML
cat > remote/libs/nats/subject-defs/generated/rust/src/lib.rs <<'RS'
pub const CDC_STREAM_NAME: &str = "CDC";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CdcRowChangeSubject {
    pub prefix: String,
    pub schema: String,
    pub table: String,
    pub op: String,
}

pub fn cdc_row_change_subject(
    prefix: &str,
    schema: &str,
    table: &str,
    op: &str,
) -> String {
    format!("{prefix}.{schema}.{table}.{op}")
}

pub fn parse_cdc_row_change_subject(subject: &str) -> Option<CdcRowChangeSubject> {
    let mut parts = subject.split('.');
    let parsed = CdcRowChangeSubject {
        prefix: parts.next()?.to_string(),
        schema: parts.next()?.to_string(),
        table: parts.next()?.to_string(),
        op: parts.next()?.to_string(),
    };
    if parts.next().is_some() {
        return None;
    }
    Some(parsed)
}
RS

mkdir -p remote/libs/runtime-config-client-rs/src
cat > remote/libs/runtime-config-client-rs/Cargo.toml <<'TOML'
[package]
name = "dd-runtime-config-client"
version = "0.1.0"
edition = "2021"

[lib]
path = "src/lib.rs"

[features]
default = ["openapi"]
openapi = []

[dependencies]
axum = "0.8"
serde_json = "1"
utoipa = "=5.5.0"
TOML
cat > remote/libs/runtime-config-client-rs/src/lib.rs <<'RS'
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{json, Value};
use utoipa::openapi::OpenApi;

const OPENAPI_JSON: &str =
    include_str!("../../../api-contracts/fragments/runtime-config-client-rs.openapi.json");

const ANY_JSON_TYPES: [&str; 7] = [
    "object",
    "array",
    "string",
    "number",
    "integer",
    "boolean",
    "null",
];

async fn snapshot() -> Json<Value> {
    Json(json!({"snapshotVersion": 0, "entries": {}}))
}

async fn mutate() -> Json<Value> {
    Json(json!({"ok": true}))
}

fn normalize_openapi_31_free_form_schemas(document: &mut Value) {
    // Utoipa 5.5 emits valid OpenAPI 3.1 free-form JSON schemas but its
    // OpenApi deserializer does not accept a schema represented solely by a
    // description or an empty object. Preserve the authoritative fragment on
    // disk and make the equivalent "any JSON value" meaning explicit only in
    // this compile-only cross-repository shim.
    for pointer in [
        "/components/schemas/RuntimeConfigEntry/properties/value",
        "/components/schemas/RuntimeConfigEntry/properties/meta",
    ] {
        let schema = document
            .pointer_mut(pointer)
            .unwrap_or_else(|| panic!("missing expected free-form schema at {pointer}"));
        let object = schema
            .as_object_mut()
            .unwrap_or_else(|| panic!("expected schema object at {pointer}"));
        object.insert("type".to_string(), json!(ANY_JSON_TYPES));
    }

    let pointer =
        "/components/schemas/RuntimeConfigSnapshotResponse/properties/entries/additionalProperties";
    let schema = document
        .pointer_mut(pointer)
        .unwrap_or_else(|| panic!("missing expected additionalProperties schema at {pointer}"));
    assert!(
        schema.as_object().is_some_and(serde_json::Map::is_empty),
        "expected the vendored additionalProperties schema to remain free-form"
    );
    *schema = json!({"type": ANY_JSON_TYPES});
}

fn shared_openapi() -> OpenApi {
    let mut document: Value =
        serde_json::from_str(OPENAPI_JSON).expect("valid vendored runtime-config OpenAPI JSON");
    normalize_openapi_31_free_form_schemas(&mut document);
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
