//! Executable OpenAPI contract support.
//!
//! Route registration and contract collection happen together in
//! `OpenApiRouter`. The complete typed contract is retained for private
//! SDKs and authenticated internal documentation. Standard unauthenticated
//! documentation routes serve only the generated fail-closed public subset.

use std::sync::Arc;

use axum::body::Bytes;
use utoipa::openapi::security::{HttpAuthScheme, HttpBuilder, SecurityScheme};
use utoipa::openapi::{Components, OpenApi};
use utoipa_scalar::Scalar;

const PUBLIC_OPENAPI_JSON: &str = include_str!("../generated/api-docs.json");

#[derive(Clone)]
pub struct ApiDocs {
    pub public_json: Bytes,
    pub public_scalar_html: Bytes,
    pub internal_json: Bytes,
    pub internal_scalar_html: Bytes,
}

impl ApiDocs {
    pub fn new(openapi: &OpenApi) -> anyhow::Result<Self> {
        let public_value: serde_json::Value = serde_json::from_str(PUBLIC_OPENAPI_JSON)?;
        anyhow::ensure!(
            public_value["x-dd-contract-scope"] == "public",
            "embedded runtime OpenAPI must be the fail-closed public contract"
        );
        anyhow::ensure!(
            public_value["info"]["title"] == "dd-embeddings-rs API (public)",
            "embedded runtime OpenAPI has unexpected service metadata"
        );
        let public_openapi: OpenApi = serde_json::from_value(public_value)?;
        let internal_json = canonical_json(openapi)?;
        Ok(Self {
            public_json: Bytes::from_static(PUBLIC_OPENAPI_JSON.as_bytes()),
            public_scalar_html: Bytes::from(Scalar::new(public_openapi).to_html()),
            internal_json: Bytes::from(internal_json),
            internal_scalar_html: Bytes::from(Scalar::new(openapi.clone()).to_html()),
        })
    }
}

pub fn public_json() -> &'static str {
    PUBLIC_OPENAPI_JSON
}

fn register_schema<T: utoipa::ToSchema>(components: &mut Components) {
    let mut schemas = vec![(
        <T as utoipa::ToSchema>::name().into_owned(),
        <T as utoipa::PartialSchema>::schema(),
    )];
    <T as utoipa::ToSchema>::schemas(&mut schemas);
    components.schemas.extend(schemas);
}

pub fn finalize(mut openapi: OpenApi) -> OpenApi {
    openapi.info.title = "dd-embeddings-rs API".to_string();
    openapi.info.version = env!("CARGO_PKG_VERSION").to_string();
    openapi.info.description = Some(
        "Typed multi-provider embeddings, reranking, Qdrant RAG, and Postgres multi-signal search API. The document is generated from the same annotated handlers and Serde DTOs registered by the running Axum router."
            .to_string(),
    );
    openapi.info.contact = None;
    openapi.info.license = None;

    let components = openapi.components.get_or_insert_with(Components::new);
    register_schema::<crate::error::ErrorResponse>(components);
    components.add_security_scheme(
        "bearer_auth",
        SecurityScheme::Http(
            HttpBuilder::new()
                .scheme(HttpAuthScheme::Bearer)
                .bearer_format("opaque service token")
                .description(Some(
                    "Set `Authorization: Bearer <token>`. Protected routes fail closed when EMBEDDINGS_API_AUTH_BEARER is absent."
                        .to_string(),
                ))
                .build(),
        ),
    );
    openapi
}

pub fn canonical_json(openapi: &OpenApi) -> Result<String, serde_json::Error> {
    let mut json = serde_json::to_string_pretty(openapi)?;
    json.push('\n');
    Ok(json)
}

pub type SharedApiDocs = Arc<ApiDocs>;
