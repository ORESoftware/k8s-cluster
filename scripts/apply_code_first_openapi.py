#!/usr/bin/env python3
from __future__ import annotations

from pathlib import Path


def replace_once(path: str, before: str, after: str, label: str) -> None:
    file = Path(path)
    source = file.read_text()
    count = source.count(before)
    if count != 1:
        raise SystemExit(f"{path}: {label}: expected one anchor, found {count}")
    file.write_text(source.replace(before, after, 1))


def replace_all(path: str, before: str, after: str, minimum: int, label: str) -> None:
    file = Path(path)
    source = file.read_text()
    count = source.count(before)
    if count < minimum:
        raise SystemExit(f"{path}: {label}: expected at least {minimum}, found {count}")
    file.write_text(source.replace(before, after))


replace_once(
    "Cargo.toml",
    'tracing-subscriber = { version = "0.3", features = ["env-filter", "fmt"] }\nurl = "2"',
    'tracing-subscriber = { version = "0.3", features = ["env-filter", "fmt"] }\nutoipa = "=5.5.0"\nutoipa-axum = "=0.2.0"\nutoipa-scalar = "=0.3.0"\nurl = "2"',
    "OpenAPI dependencies",
)

for path in ["src/contracts.rs", "src/contact/contracts.rs"]:
    replace_once(path, "use serde_json::Value;", "use serde_json::Value;\nuse utoipa::ToSchema;", "ToSchema import")
    replace_all(path, "Serialize, Deserialize", "Serialize, Deserialize, ToSchema", 8, "wire schema derives")

replace_once(
    "src/redaction.rs",
    "use sha2::{Digest, Sha256};",
    "use sha2::{Digest, Sha256};\nuse utoipa::ToSchema;",
    "fingerprint ToSchema import",
)
replace_once(
    "src/redaction.rs",
    "Hash, Serialize, Deserialize)]",
    "Hash, Serialize, Deserialize, ToSchema)]",
    "fingerprint schema derive",
)

for path in ["src/dispatch.rs", "src/contact/dispatch.rs"]:
    replace_once(path, "use serde::Serialize;", "use serde::Serialize;\nuse utoipa::ToSchema;", "readiness ToSchema import")
    replace_all(path, "Debug, Clone, Serialize)]", "Debug, Clone, Serialize, ToSchema)]", 2, "readiness schema derives")

replace_once(
    "src/http_api.rs",
    "use axum::routing::{get, post};\nuse axum::{Json, Router};\nuse serde::{Deserialize, Serialize};",
    "use axum::{Json, Router};\nuse serde::{Deserialize, Serialize};\nuse utoipa::ToSchema;\nuse utoipa_axum::{router::OpenApiRouter, routes};",
    "push OpenAPI imports",
)
replace_once(
    "src/http_api.rs",
    '''pub fn router(state: ApiState) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .route("/v1/push/jobs", post(submit_job))
        .route("/v1/push/jobs/batch", post(submit_batch))
        .layer(DefaultBodyLimit::max(MAX_HTTP_BODY_BYTES))
        .with_state(state)
}
''',
    '''pub(crate) fn push_openapi_router() -> OpenApiRouter<ApiState> {
    OpenApiRouter::new()
        .routes(routes!(healthz))
        .routes(routes!(readyz))
        .routes(routes!(submit_job))
        .routes(routes!(submit_batch))
}

pub fn router(state: ApiState) -> Router {
    let (router, _) = push_openapi_router().split_for_parts();
    router
        .layer(DefaultBodyLimit::max(MAX_HTTP_BODY_BYTES))
        .with_state(state)
}
''',
    "push route registry",
)
replace_once(
    "src/http_api.rs",
    "async fn healthz() -> Json<ServiceHealth> {",
    '''#[utoipa::path(
    get,
    path = "/healthz",
    operation_id = "getPushServiceHealth",
    tag = "operations",
    security(()),
    responses((status = 200, description = "Process liveness", body = ServiceHealth))
)]
async fn healthz() -> Json<ServiceHealth> {''',
    "health path",
)
replace_once(
    "src/http_api.rs",
    "async fn readyz(State(state): State<ApiState>) -> Response {",
    '''#[utoipa::path(
    get,
    path = "/readyz",
    operation_id = "getPushServiceReadiness",
    tag = "operations",
    security(()),
    responses(
        (status = 200, description = "Authentication and at least one push provider are configured", body = ReadinessResponse),
        (status = 503, description = "Authentication or push providers are unavailable", body = ReadinessResponse)
    )
)]
async fn readyz(State(state): State<ApiState>) -> Response {''',
    "readiness path",
)
replace_once(
    "src/http_api.rs",
    '''async fn submit_job(
    State(state): State<ApiState>,''',
    '''#[utoipa::path(
    post,
    path = "/v1/push/jobs",
    operation_id = "submitPushJob",
    tag = "push",
    request_body = PushJob,
    security(("bearer_auth" = [])),
    responses(
        (status = 202, description = "Provider accepted the push job", body = PushOutcome),
        (status = 400, description = "Contract validation failed", body = ErrorEnvelope),
        (status = 401, description = "Authentication failed", body = ErrorEnvelope),
        (status = 422, description = "Provider rejected the target", body = PushOutcome),
        (status = 429, description = "Provider throttled the request", body = PushOutcome),
        (status = 502, description = "Permanent provider failure", body = PushOutcome),
        (status = 503, description = "Transient or internal provider failure", body = PushOutcome)
    )
)]
async fn submit_job(
    State(state): State<ApiState>,''',
    "push submission path",
)
replace_once(
    "src/http_api.rs",
    '''async fn submit_batch(
    State(state): State<ApiState>,''',
    '''#[utoipa::path(
    post,
    path = "/v1/push/jobs/batch",
    operation_id = "submitPushBatch",
    tag = "push",
    request_body = BatchRequest,
    security(("bearer_auth" = [])),
    responses(
        (status = 202, description = "Every job was accepted", body = BatchResponse),
        (status = 207, description = "Batch contains mixed outcomes", body = BatchResponse),
        (status = 400, description = "Batch size is invalid", body = ErrorEnvelope),
        (status = 401, description = "Authentication failed", body = ErrorEnvelope)
    )
)]
async fn submit_batch(
    State(state): State<ApiState>,''',
    "push batch path",
)
replace_all("src/http_api.rs", "#[derive(Debug, Serialize)]", "#[derive(Debug, Serialize, ToSchema)]", 5, "push response schemas")
replace_once("src/http_api.rs", "#[derive(Debug, Deserialize)]\npub struct BatchRequest", "#[derive(Debug, Deserialize, ToSchema)]\npub struct BatchRequest", "push batch request schema")
replace_once("src/http_api.rs", "    service: &'static str,", "    #[schema(value_type = String)]\n    service: &'static str,", "service schema")
replace_once("src/http_api.rs", "    mode: &'static str,", "    #[schema(value_type = String)]\n    mode: &'static str,", "auth mode schema")
replace_once("src/http_api.rs", "    code: &'static str,", "    #[schema(value_type = String)]\n    code: &'static str,", "error code schema")

replace_once(
    "src/contact/http.rs",
    "use axum::routing::{get, post};\nuse axum::{Json, Router};\nuse serde::{Deserialize, Serialize};",
    "use axum::{Json, Router};\nuse serde::{Deserialize, Serialize};\nuse utoipa::ToSchema;\nuse utoipa_axum::{router::OpenApiRouter, routes};",
    "contact OpenAPI imports",
)
replace_once(
    "src/contact/http.rs",
    '''pub fn contact_router(state: ContactApiState) -> Router {
    Router::new()
        .route("/v1/contact/readyz", get(readyz))
        .route("/v1/contact/jobs", post(submit_job))
        .route("/v1/contact/jobs/batch", post(submit_batch))
        .layer(DefaultBodyLimit::max(MAX_CONTACT_HTTP_BODY_BYTES))
        .with_state(state)
}
''',
    '''pub(crate) fn contact_openapi_router() -> OpenApiRouter<ContactApiState> {
    OpenApiRouter::new()
        .routes(routes!(readyz))
        .routes(routes!(submit_job))
        .routes(routes!(submit_batch))
}

pub fn contact_router(state: ContactApiState) -> Router {
    let (router, _) = contact_openapi_router().split_for_parts();
    router
        .layer(DefaultBodyLimit::max(MAX_CONTACT_HTTP_BODY_BYTES))
        .with_state(state)
}
''',
    "contact route registry",
)
replace_once(
    "src/contact/http.rs",
    "async fn readyz(State(state): State<ContactApiState>) -> Response {",
    '''#[utoipa::path(
    get,
    path = "/v1/contact/readyz",
    operation_id = "getContactServiceReadiness",
    tag = "operations",
    security(()),
    responses(
        (status = 200, description = "Authentication and at least one contact provider are configured", body = ContactReadinessResponse),
        (status = 503, description = "Authentication or contact providers are unavailable", body = ContactReadinessResponse)
    )
)]
async fn readyz(State(state): State<ContactApiState>) -> Response {''',
    "contact readiness path",
)
replace_once(
    "src/contact/http.rs",
    '''async fn submit_job(
    State(state): State<ContactApiState>,''',
    '''#[utoipa::path(
    post,
    path = "/v1/contact/jobs",
    operation_id = "submitContactJob",
    tag = "contact",
    request_body = ContactJob,
    security(("bearer_auth" = [])),
    responses(
        (status = 202, description = "Provider accepted the contact job", body = ContactOutcome),
        (status = 400, description = "Contract validation failed", body = ContactErrorEnvelope),
        (status = 401, description = "Authentication failed", body = ContactErrorEnvelope),
        (status = 422, description = "Provider rejected the target", body = ContactOutcome),
        (status = 429, description = "Provider throttled the request", body = ContactOutcome),
        (status = 502, description = "Permanent provider failure", body = ContactOutcome),
        (status = 503, description = "Transient or internal provider failure", body = ContactOutcome)
    )
)]
async fn submit_job(
    State(state): State<ContactApiState>,''',
    "contact submission path",
)
replace_once(
    "src/contact/http.rs",
    '''async fn submit_batch(
    State(state): State<ContactApiState>,''',
    '''#[utoipa::path(
    post,
    path = "/v1/contact/jobs/batch",
    operation_id = "submitContactBatch",
    tag = "contact",
    request_body = ContactBatchRequest,
    security(("bearer_auth" = [])),
    responses(
        (status = 202, description = "Every job was accepted", body = ContactBatchResponse),
        (status = 207, description = "Batch contains mixed outcomes", body = ContactBatchResponse),
        (status = 400, description = "Batch size is invalid", body = ContactErrorEnvelope),
        (status = 401, description = "Authentication failed", body = ContactErrorEnvelope)
    )
)]
async fn submit_batch(
    State(state): State<ContactApiState>,''',
    "contact batch path",
)
replace_all("src/contact/http.rs", "#[derive(Debug, Serialize)]", "#[derive(Debug, Serialize, ToSchema)]", 4, "contact response schemas")
replace_once("src/contact/http.rs", "#[derive(Debug, Deserialize)]\npub struct ContactBatchRequest", "#[derive(Debug, Deserialize, ToSchema)]\npub struct ContactBatchRequest", "contact batch request schema")
replace_once("src/contact/http.rs", "    authentication_mode: &'static str,", "    #[schema(value_type = String)]\n    authentication_mode: &'static str,", "contact auth mode schema")
replace_once("src/contact/http.rs", "    code: &'static str,", "    #[schema(value_type = String)]\n    code: &'static str,", "contact error code schema")

api_docs = r'''//! Executable OpenAPI and standard documentation routes.
//!
//! Every product route is registered through `utoipa_axum::routes!`, which
//! couples the running Axum handler and its OpenAPI operation. The public
//! document is a fail-closed projection of that complete executable contract;
//! authenticated internal routes retain the full push and contact surface for
//! private SDK generation.

use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::Router;
use utoipa::openapi::security::{HttpAuthScheme, HttpBuilder, SecurityScheme};
use utoipa::openapi::{Components, OpenApi};
use utoipa_axum::{router::OpenApiRouter, routes};
use utoipa_scalar::Scalar;

use crate::contact::{ContactApiState, contact_openapi_router};
use crate::http_api::{ApiState, RequestAuthenticator, push_openapi_router};

const OPENAPI_CONTENT_TYPE: &str = "application/vnd.oai.openapi+json;version=3.1";
const PUBLIC_PATHS: &[&str] = &[
    "/healthz",
    "/readyz",
    "/v1/contact/readyz",
    "/openapi.json",
    "/api/docs.json",
    "/api/docs",
    "/docs/api",
];

#[derive(Clone)]
struct DocsState {
    public_json: Bytes,
    internal_json: Bytes,
    public_html: Bytes,
    internal_html: Bytes,
    authenticator: Arc<dyn RequestAuthenticator>,
}

impl DocsState {
    fn new(
        openapi: &OpenApi,
        authenticator: Arc<dyn RequestAuthenticator>,
    ) -> Result<Self, serde_json::Error> {
        let public = public_projection(openapi)?;
        Ok(Self {
            public_json: Bytes::from(canonical_json(&public)?),
            internal_json: Bytes::from(canonical_json(openapi)?),
            public_html: Bytes::from(Scalar::new(public).to_html()),
            internal_html: Bytes::from(Scalar::new(openapi.clone()).to_html()),
            authenticator,
        })
    }
}

pub fn application_router(
    push_state: ApiState,
    contact_state: ContactApiState,
    authenticator: Arc<dyn RequestAuthenticator>,
) -> Result<Router, serde_json::Error> {
    let (push_router, push_openapi) = push_openapi_router().split_for_parts();
    let (contact_router, contact_openapi) = contact_openapi_router().split_for_parts();
    let (docs_router, docs_openapi) = docs_openapi_router().split_for_parts();

    let openapi = merge_and_finalize(push_openapi, contact_openapi, docs_openapi);
    let docs_state = DocsState::new(&openapi, authenticator)?;

    Ok(push_router
        .with_state(push_state)
        .merge(contact_router.with_state(contact_state))
        .merge(docs_router.with_state(docs_state)))
}

pub fn openapi_document() -> OpenApi {
    merge_and_finalize(
        push_openapi_router().into_openapi(),
        contact_openapi_router().into_openapi(),
        docs_openapi_router().into_openapi(),
    )
}

pub fn public_openapi_document() -> Result<OpenApi, serde_json::Error> {
    public_projection(&openapi_document())
}

fn merge_and_finalize(mut push: OpenApi, contact: OpenApi, docs: OpenApi) -> OpenApi {
    push.merge(contact);
    push.merge(docs);
    finalize(push)
}

fn docs_openapi_router() -> OpenApiRouter<DocsState> {
    OpenApiRouter::new()
        .routes(routes!(public_openapi_json))
        .routes(routes!(public_openapi_alias))
        .routes(routes!(public_api_docs))
        .routes(routes!(public_api_docs_alias))
        .routes(routes!(internal_openapi_json))
        .routes(routes!(internal_api_docs))
}

fn finalize(mut openapi: OpenApi) -> OpenApi {
    openapi.info.title = "push-notification-server API".to_owned();
    openapi.info.version = env!("CARGO_PKG_VERSION").to_owned();
    openapi.info.description = Some(
        "Provider-neutral push, email, and SMS delivery API. The contract is generated from the same annotated handlers and Serde DTOs mounted by the running Axum process."
            .to_owned(),
    );
    let components = openapi.components.get_or_insert_with(Components::new);
    components.add_security_scheme(
        "bearer_auth",
        SecurityScheme::Http(
            HttpBuilder::new()
                .scheme(HttpAuthScheme::Bearer)
                .bearer_format("opaque service token")
                .description(Some(
                    "Set Authorization: Bearer <token>. Protected routes fail closed when authentication is not configured."
                        .to_owned(),
                ))
                .build(),
        ),
    );
    openapi
}

fn public_projection(openapi: &OpenApi) -> Result<OpenApi, serde_json::Error> {
    let mut value = serde_json::to_value(openapi)?;
    let allowed = PUBLIC_PATHS.iter().copied().collect::<std::collections::BTreeSet<_>>();
    value["paths"]
        .as_object_mut()
        .expect("OpenAPI paths are always an object")
        .retain(|path, _| allowed.contains(path.as_str()));
    value["info"]["title"] = serde_json::Value::String(
        "push-notification-server API (public)".to_owned(),
    );
    value["x-dd-contract-scope"] = serde_json::Value::String("public".to_owned());
    serde_json::from_value(value)
}

pub fn canonical_json(openapi: &OpenApi) -> Result<String, serde_json::Error> {
    let mut json = serde_json::to_string_pretty(openapi)?;
    json.push('\n');
    Ok(json)
}

fn openapi_response(bytes: Bytes) -> Response {
    ([(header::CONTENT_TYPE, OPENAPI_CONTENT_TYPE)], bytes).into_response()
}

fn html_response(bytes: Bytes) -> Response {
    ([(header::CONTENT_TYPE, "text/html; charset=utf-8")], bytes).into_response()
}

fn authorized(state: &DocsState, headers: &HeaderMap) -> bool {
    state.authenticator.authenticate(headers)
}

fn unauthorized() -> Response {
    (StatusCode::UNAUTHORIZED, "request authentication failed").into_response()
}

#[utoipa::path(
    get,
    path = "/openapi.json",
    operation_id = "getPublicOpenApi",
    tag = "documentation",
    security(()),
    responses((status = 200, description = "Fail-closed public OpenAPI 3.1 document", body = String, content_type = OPENAPI_CONTENT_TYPE))
)]
async fn public_openapi_json(State(state): State<DocsState>) -> Response {
    openapi_response(state.public_json)
}

#[utoipa::path(
    get,
    path = "/api/docs.json",
    operation_id = "getPublicOpenApiCompatibilityAlias",
    tag = "documentation",
    security(()),
    responses((status = 200, description = "Compatibility alias for the public OpenAPI document", body = String, content_type = OPENAPI_CONTENT_TYPE))
)]
async fn public_openapi_alias(State(state): State<DocsState>) -> Response {
    openapi_response(state.public_json)
}

#[utoipa::path(
    get,
    path = "/api/docs",
    operation_id = "getPublicApiReference",
    tag = "documentation",
    security(()),
    responses((status = 200, description = "Scalar reference for the public contract", body = String, content_type = "text/html"))
)]
async fn public_api_docs(State(state): State<DocsState>) -> Response {
    html_response(state.public_html)
}

#[utoipa::path(
    get,
    path = "/docs/api",
    operation_id = "getPublicApiReferenceCompatibilityAlias",
    tag = "documentation",
    security(()),
    responses((status = 200, description = "Compatibility alias for the public Scalar reference", body = String, content_type = "text/html"))
)]
async fn public_api_docs_alias(State(state): State<DocsState>) -> Response {
    html_response(state.public_html)
}

#[utoipa::path(
    get,
    path = "/internal/openapi.json",
    operation_id = "getInternalOpenApi",
    tag = "documentation",
    security(("bearer_auth" = [])),
    responses(
        (status = 200, description = "Complete private OpenAPI 3.1 document", body = String, content_type = OPENAPI_CONTENT_TYPE),
        (status = 401, description = "Authentication failed")
    )
)]
async fn internal_openapi_json(
    State(state): State<DocsState>,
    headers: HeaderMap,
) -> Response {
    if !authorized(&state, &headers) {
        return unauthorized();
    }
    openapi_response(state.internal_json)
}

#[utoipa::path(
    get,
    path = "/internal/docs/api",
    operation_id = "getInternalApiReference",
    tag = "documentation",
    security(("bearer_auth" = [])),
    responses(
        (status = 200, description = "Scalar reference for the complete private contract", body = String, content_type = "text/html"),
        (status = 401, description = "Authentication failed")
    )
)]
async fn internal_api_docs(
    State(state): State<DocsState>,
    headers: HeaderMap,
) -> Response {
    if !authorized(&state, &headers) {
        return unauthorized();
    }
    html_response(state.internal_html)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_contract_is_a_fail_closed_subset() {
        let internal = openapi_document();
        let public = public_openapi_document().expect("public contract");
        assert!(internal.paths.paths.contains_key("/v1/push/jobs"));
        assert!(internal.paths.paths.contains_key("/v1/contact/jobs"));
        assert!(!public.paths.paths.contains_key("/v1/push/jobs"));
        assert!(!public.paths.paths.contains_key("/v1/contact/jobs"));
        for path in PUBLIC_PATHS {
            assert!(public.paths.paths.contains_key(*path), "missing public path {path}");
        }
    }
}
'''
Path("src/api_docs.rs").write_text(api_docs)

replace_once(
    "src/lib.rs",
    "pub mod contact;",
    "pub mod api_docs;\npub mod contact;",
    "api_docs module",
)
replace_once(
    "src/lib.rs",
    "pub use contact::{",
    "pub use api_docs::{application_router, canonical_json, openapi_document, public_openapi_document};\npub use contact::{",
    "api_docs exports",
)

replace_once(
    "src/main.rs",
    '''use push_notification_server::{
    ApiState, ContactApiState, NatsConfig, contact_registry_from_env, contact_router,
    provider_registry_from_env, request_authenticator_from_env, router, run_nats_consumer,
};''',
    '''use push_notification_server::{
    ApiState, ContactApiState, NatsConfig, application_router, canonical_json,
    contact_registry_from_env, openapi_document, provider_registry_from_env,
    public_openapi_document, request_authenticator_from_env, run_nats_consumer,
};''',
    "main imports",
)
replace_once(
    "src/main.rs",
    '''async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()''',
    '''async fn main() -> Result<(), Box<dyn std::error::Error>> {
    if let Some(scope) = export_openapi_scope() {
        let openapi = match scope.as_str() {
            "internal" => openapi_document(),
            "public" => public_openapi_document()?,
            other => return Err(format!("unsupported OpenAPI scope: {other}").into()),
        };
        print!("{}", canonical_json(&openapi)?);
        return Ok(());
    }

    tracing_subscriber::fmt()''',
    "side-effect-free export",
)
replace_once(
    "src/main.rs",
    '''    let app = router(ApiState::new(registry, authenticator.clone())).merge(contact_router(
        ContactApiState::new(contact_registry, authenticator),
    ));''',
    '''    let app = application_router(
        ApiState::new(registry, authenticator.clone()),
        ContactApiState::new(contact_registry, authenticator.clone()),
        authenticator,
    )?;''',
    "combined executable router",
)
replace_once(
    "src/main.rs",
    '''fn bind_address() -> Result<SocketAddr, std::net::AddrParseError> {''',
    '''fn export_openapi_scope() -> Option<String> {
    env::args().find_map(|argument| {
        argument
            .strip_prefix("--export-openapi=")
            .map(ToOwned::to_owned)
    })
}

fn bind_address() -> Result<SocketAddr, std::net::AddrParseError> {''',
    "export scope parser",
)

workflow = r'''name: executable OpenAPI contract

on:
  pull_request:
    paths:
      - '.github/workflows/openapi-contract.yml'
      - 'Cargo.toml'
      - 'Cargo.lock'
      - 'generated/openapi.*.json'
      - 'src/api_docs.rs'
      - 'src/contact/contracts.rs'
      - 'src/contact/dispatch.rs'
      - 'src/contact/http.rs'
      - 'src/contracts.rs'
      - 'src/dispatch.rs'
      - 'src/http_api.rs'
      - 'src/lib.rs'
      - 'src/main.rs'
      - 'src/redaction.rs'
  push:
    branches:
      - main
    paths:
      - '.github/workflows/openapi-contract.yml'
      - 'Cargo.toml'
      - 'Cargo.lock'
      - 'generated/openapi.*.json'
      - 'src/api_docs.rs'
      - 'src/contact/contracts.rs'
      - 'src/contact/dispatch.rs'
      - 'src/contact/http.rs'
      - 'src/contracts.rs'
      - 'src/dispatch.rs'
      - 'src/http_api.rs'
      - 'src/lib.rs'
      - 'src/main.rs'
      - 'src/redaction.rs'
  workflow_dispatch:

permissions:
  contents: read

concurrency:
  group: push-openapi-${{ github.workflow }}-${{ github.ref }}
  cancel-in-progress: true

jobs:
  contract:
    runs-on: ubuntu-24.04
    timeout-minutes: 30
    steps:
      - name: Checkout
        uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1
        with:
          persist-credentials: false
          show-progress: false

      - name: Setup Rust 1.88
        uses: dtolnay/rust-toolchain@4cda84d5c5c54efe2404f9d843567869ab1699d4
        with:
          toolchain: '1.88.0'
          components: rustfmt, clippy

      - name: Compile and test the executable contract
        run: |
          set -euo pipefail
          cargo fmt --all -- --check
          cargo check --locked
          cargo clippy --locked --all-targets -- -D warnings
          cargo test --locked --all-targets

      - name: Prove deterministic public and internal exports
        run: |
          set -euo pipefail
          cargo run --locked --quiet -- --export-openapi=internal > /tmp/openapi.internal.1.json
          cargo run --locked --quiet -- --export-openapi=internal > /tmp/openapi.internal.2.json
          cargo run --locked --quiet -- --export-openapi=public > /tmp/openapi.public.1.json
          cargo run --locked --quiet -- --export-openapi=public > /tmp/openapi.public.2.json
          cmp /tmp/openapi.internal.1.json /tmp/openapi.internal.2.json
          cmp /tmp/openapi.public.1.json /tmp/openapi.public.2.json
          cmp /tmp/openapi.internal.1.json generated/openapi.internal.json
          cmp /tmp/openapi.public.1.json generated/openapi.public.json

      - name: Validate visibility, auth, and stable operations
        run: |
          python3 - <<'PY'
          import json
          from pathlib import Path

          internal = json.loads(Path('generated/openapi.internal.json').read_text())
          public = json.loads(Path('generated/openapi.public.json').read_text())
          expected_public = {
              '/healthz', '/readyz', '/v1/contact/readyz',
              '/openapi.json', '/api/docs.json', '/api/docs', '/docs/api',
          }
          expected_internal = expected_public | {
              '/v1/push/jobs', '/v1/push/jobs/batch',
              '/v1/contact/jobs', '/v1/contact/jobs/batch',
              '/internal/openapi.json', '/internal/docs/api',
          }
          assert set(public['paths']) == expected_public
          assert set(internal['paths']) == expected_internal
          assert public['x-dd-contract-scope'] == 'public'
          assert 'bearer_auth' in internal['components']['securitySchemes']
          operation_ids = []
          for path, item in internal['paths'].items():
              for method, operation in item.items():
                  if method.lower() not in {'get', 'post', 'put', 'patch', 'delete'}:
                      continue
                  operation_ids.append(operation['operationId'])
                  if path.startswith('/v1/') and path != '/v1/contact/readyz':
                      assert operation.get('security') == [{'bearer_auth': []}], (method, path)
          assert len(operation_ids) == len(set(operation_ids))
          PY

      - name: Reject repository drift
        run: |
          set -euo pipefail
          git diff --check
          test -z "$(git status --short)"
          ! grep -R -n -E '^(<<<<<<<|=======|>>>>>>>)' src generated Cargo.toml
'''
Path(".github/workflows/openapi-contract.yml").write_text(workflow)

Path("docs/openapi-contract.md").write_text(
    """# Executable HTTP API contract\n\n"
    "The running Axum routes and the OpenAPI operations are registered together through "
    "`utoipa_axum::routes!`. `generated/openapi.internal.json` is the complete private SDK "
    "source. `generated/openapi.public.json` is a fail-closed projection containing only "
    "health, readiness, and standard documentation routes.\n\n"
    "Standard routes:\n\n"
    "- `GET /openapi.json` and `GET /api/docs.json`: public OpenAPI 3.1 JSON;\n"
    "- `GET /api/docs` and `GET /docs/api`: public Scalar reference;\n"
    "- `GET /internal/openapi.json`: authenticated private OpenAPI JSON;\n"
    "- `GET /internal/docs/api`: authenticated private Scalar reference.\n\n"
    "Export without runtime credentials or network access:\n\n"
    "```bash\n"
    "cargo run --locked -- --export-openapi=public\n"
    "cargo run --locked -- --export-openapi=internal\n"
    "```\n"
)
