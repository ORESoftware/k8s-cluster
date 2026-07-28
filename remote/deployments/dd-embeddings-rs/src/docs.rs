//! Executable OpenAPI contract support.
//!
//! Route registration and contract collection happen together in
//! `OpenApiRouter`. This module only adds service metadata/security and turns
//! that generated document into immutable bytes served at runtime.

use std::sync::Arc;

use axum::body::Bytes;
use utoipa::openapi::security::{HttpAuthScheme, HttpBuilder, SecurityScheme};
use utoipa::openapi::{Components, OpenApi};
use utoipa_scalar::Scalar;

#[derive(Clone)]
pub struct ApiDocs {
    pub json: Bytes,
    pub scalar_html: Bytes,
}

impl ApiDocs {
    pub fn new(openapi: &OpenApi) -> Result<Self, serde_json::Error> {
        let json = canonical_json(openapi)?;
        let scalar_html = Scalar::new(openapi.clone()).to_html();
        Ok(Self {
            json: Bytes::from(json),
            scalar_html: Bytes::from(scalar_html),
        })
    }
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
    // `OpenApiRouter::new()` currently inherits Utoipa crate metadata in its
    // default `Info`. That metadata describes the generator dependency, not
    // this service. Clear it explicitly so the exported contract never claims
    // the tool author's contact details or license as API provenance.
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
                    "Set `Authorization: Bearer <token>`. Runtime enforcement is enabled when EMBEDDINGS_API_AUTH_BEARER is configured."
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
