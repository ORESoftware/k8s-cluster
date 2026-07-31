//! Executable OpenAPI and standard documentation routes.
//!
//! Every product route is registered through `utoipa_axum::routes!`, which
//! couples the running Axum handler and its OpenAPI operation. The public
//! document is a fail-closed projection of that complete executable contract;
//! authenticated internal routes retain the full push and contact surface for
//! private SDK generation.

use std::sync::Arc;

use axum::Router;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use utoipa::openapi::security::{HttpAuthScheme, HttpBuilder, SecurityScheme};
use utoipa::openapi::{Components, OpenApi};
use utoipa_axum::{router::OpenApiRouter, routes};
use utoipa_scalar::Scalar;

use crate::contact::http::{ContactApiState, contact_openapi_router};
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
    let mut public = openapi.clone();
    public
        .paths
        .paths
        .retain(|path, _| PUBLIC_PATHS.contains(&path.as_str()));
    public.info.title = "push-notification-server API (public)".to_owned();
    public
        .extensions
        .get_or_insert_with(Default::default)
        .insert(
            "x-dd-contract-scope".to_owned(),
            serde_json::Value::String("public".to_owned()),
        );
    Ok(public)
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
async fn internal_openapi_json(State(state): State<DocsState>, headers: HeaderMap) -> Response {
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
async fn internal_api_docs(State(state): State<DocsState>, headers: HeaderMap) -> Response {
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
            assert!(
                public.paths.paths.contains_key(*path),
                "missing public path {path}"
            );
        }
    }
}
