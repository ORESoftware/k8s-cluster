//! Axum router and executable OpenAPI wiring.

pub mod health;
pub mod webhook;

use std::sync::Arc;

use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Router};
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::trace::TraceLayer;
use utoipa::openapi::OpenApi;
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::docs::{self, ApiDocs, SharedApiDocs};
use crate::state::AppState;

/// Inbound webhook payloads are bounded. GitHub allows larger deliveries, but
/// this service accepts at most 8 MiB before the handler can allocate or parse
/// the body.
pub const MAX_BODY_BYTES: usize = 8 * 1024 * 1024;
const OPENAPI_CONTENT_TYPE: &str = "application/vnd.oai.openapi+json;version=3.1";

/// The same registrations create both the live local router and its OpenAPI
/// operations. Adding a local HTTP route anywhere else is a contract violation.
pub fn local_router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(health::health))
        .routes(routes!(health::ready))
        .routes(routes!(health::metrics))
        .routes(routes!(openapi_json))
        .routes(routes!(api_docs_json))
        .routes(routes!(api_docs_ui))
        .routes(routes!(docs_api_ui))
        .routes(routes!(webhook::github))
}

/// Construct the complete internal contract without reading service config,
/// initializing telemetry, constructing a GitHub client or analyzer pipeline,
/// registering with runtime-config, or binding a socket.
pub fn openapi_document() -> OpenApi {
    let (_, shared_openapi) = dd_runtime_config_client::router_and_openapi();
    docs::compose(local_router().into_openapi(), shared_openapi)
}

/// Construct the production router and compose the shared runtime-config
/// router/contract pair without copying shared paths or schemas.
pub fn app_router(state: AppState) -> Result<Router, Box<dyn std::error::Error + Send + Sync>> {
    let (shared_router, shared_openapi) = dd_runtime_config_client::router_and_openapi();
    let (local_router, local_openapi) = local_router().split_for_parts();
    let openapi = docs::compose(local_openapi, shared_openapi);
    let api_docs = Arc::new(ApiDocs::new(&openapi)?);

    Ok(local_router
        .with_state(state)
        .merge(shared_router)
        .layer(Extension(api_docs))
        .layer(RequestBodyLimitLayer::new(MAX_BODY_BYTES))
        .layer(TraceLayer::new_for_http()))
}

/// Backward-compatible constructor used by the existing webhook integration
/// suite and downstream in-repository callers. It delegates to the same
/// executable router/contract composition as production; there is no second
/// route table.
pub fn router(state: AppState) -> Router {
    app_router(state).expect("formal-methods executable API router must build")
}

#[utoipa::path(
    get,
    path = "/openapi.json",
    operation_id = "getFormalMethodsPublicOpenApi",
    tag = "documentation",
    security(()),
    responses((status = 200, description = "Fail-closed public OpenAPI 3.1 contract", content_type = "application/vnd.oai.openapi+json;version=3.1"))
)]
async fn openapi_json(Extension(docs): Extension<SharedApiDocs>) -> Response {
    public_openapi_response(docs)
}

#[utoipa::path(
    get,
    path = "/api/docs.json",
    operation_id = "getFormalMethodsPublicOpenApiCompatibilityAlias",
    tag = "documentation",
    security(()),
    responses((status = 200, description = "Compatibility alias for the fail-closed public OpenAPI 3.1 contract", content_type = "application/vnd.oai.openapi+json;version=3.1"))
)]
async fn api_docs_json(Extension(docs): Extension<SharedApiDocs>) -> Response {
    public_openapi_response(docs)
}

#[utoipa::path(
    get,
    path = "/api/docs",
    operation_id = "getFormalMethodsPublicApiReference",
    tag = "documentation",
    security(()),
    responses((status = 200, description = "Interactive Scalar reference loaded from /openapi.json", body = String, content_type = "text/html"))
)]
async fn api_docs_ui(Extension(docs): Extension<SharedApiDocs>) -> Response {
    public_scalar_response(docs)
}

#[utoipa::path(
    get,
    path = "/docs/api",
    operation_id = "getFormalMethodsPublicApiReferenceCompatibilityAlias",
    tag = "documentation",
    security(()),
    responses((status = 200, description = "Compatibility alias for the public Scalar API reference", body = String, content_type = "text/html"))
)]
async fn docs_api_ui(Extension(docs): Extension<SharedApiDocs>) -> Response {
    public_scalar_response(docs)
}

fn public_openapi_response(docs: SharedApiDocs) -> Response {
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, OPENAPI_CONTENT_TYPE)],
        docs.public_json.clone(),
    )
        .into_response()
}

fn public_scalar_response(docs: SharedApiDocs) -> Response {
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        docs.public_scalar_html.clone(),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    #[test]
    fn local_and_shared_paths_are_present_once() {
        let value = serde_json::to_value(openapi_document()).expect("serialize OpenAPI");
        let paths = value["paths"]
            .as_object()
            .expect("OpenAPI paths")
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>();
        let expected = [
            "/health",
            "/ready",
            "/metrics",
            "/openapi.json",
            "/api/docs.json",
            "/api/docs",
            "/docs/api",
            "/webhook/github",
            "/internal/runtime-config",
            "/internal/update-runtime-config",
            "/internal/runtime-config/reset",
        ]
        .into_iter()
        .map(str::to_string)
        .collect::<BTreeSet<_>>();
        assert_eq!(paths, expected);
    }

    #[test]
    fn webhook_contract_preserves_security_and_body_limit() {
        let value = serde_json::to_value(openapi_document()).expect("serialize OpenAPI");
        let operation = &value["paths"]["/webhook/github"]["post"];
        assert_eq!(
            operation["security"],
            serde_json::json!([{ "github_webhook_signature": [] }])
        );
        assert_eq!(
            operation["x-dd-max-request-body-bytes"],
            MAX_BODY_BYTES as u64
        );
        assert_eq!(operation["x-dd-visibility"], "internal");
    }
}
