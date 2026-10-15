//! Executable OpenAPI and Scalar documentation for the Tor dashboard.
//!
//! The complete document is collected from the same `OpenApiRouter` route
//! declarations used at runtime. The public document is a fail-closed
//! projection driven by the exact same sensitivity classifier as the auth
//! middleware, so route visibility and runtime protection cannot drift.

use std::sync::Arc;

use axum::body::Bytes;
use utoipa::openapi::security::{HttpAuthScheme, HttpBuilder, SecurityScheme};
use utoipa::openapi::{Components, OpenApi};
use utoipa_scalar::Scalar;

#[derive(Clone)]
pub struct ApiDocs {
    pub public_json: Bytes,
    pub public_scalar_html: Bytes,
    pub internal_json: Bytes,
    pub internal_scalar_html: Bytes,
}

impl ApiDocs {
    pub fn new(internal: &OpenApi) -> anyhow::Result<Self> {
        let public = public_document(internal)?;
        Ok(Self {
            public_json: Bytes::from(canonical_json(&public)?),
            public_scalar_html: Bytes::from(Scalar::new(public).to_html()),
            internal_json: Bytes::from(canonical_json(internal)?),
            internal_scalar_html: Bytes::from(Scalar::new(internal.clone()).to_html()),
        })
    }
}

pub fn finalize(openapi: OpenApi) -> anyhow::Result<OpenApi> {
    let mut openapi = openapi;
    openapi.info.title = "tor-server dashboard API".to_owned();
    openapi.info.version = env!("CARGO_PKG_VERSION").to_owned();
    openapi.info.description = Some(
        "Executable dashboard, status, documentation, WebSocket, PAC, and bounded fetch contract. Route registration and OpenAPI collection share the same Axum handlers; sensitive operations use the same classifier as runtime TOR_UI_TOKEN enforcement."
            .to_owned(),
    );
    openapi.info.contact = None;
    openapi.info.license = None;

    let components = openapi.components.get_or_insert_with(Components::new);
    components.add_security_scheme(
        "ui_token",
        SecurityScheme::Http(
            HttpBuilder::new()
                .scheme(HttpAuthScheme::Bearer)
                .bearer_format("opaque dashboard token")
                .description(Some(
                    "Set `Authorization: Bearer <TOR_UI_TOKEN>`. The existing `?token=` compatibility path remains supported for dashboard navigation, but bearer headers are preferred."
                        .to_owned(),
                ))
                .build(),
        ),
    );

    let mut value = serde_json::to_value(openapi)?;
    value["openapi"] = serde_json::Value::String("3.1.0".to_owned());
    value["jsonSchemaDialect"] =
        serde_json::Value::String("https://json-schema.org/draft/2020-12/schema".to_owned());
    value["x-dd-contract-scope"] = serde_json::Value::String("internal".to_owned());
    Ok(serde_json::from_value(value)?)
}

pub fn public_document(internal: &OpenApi) -> anyhow::Result<OpenApi> {
    let mut value = serde_json::to_value(internal)?;
    let paths = value["paths"]
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("OpenAPI paths must be an object"))?;
    paths.retain(|path, _| !super::requires_ui_token(path));

    // The public operations are HTML/static/liveness/documentation routes and
    // intentionally expose no private dashboard schemas or token scheme.
    value["components"] = serde_json::json!({"schemas": {}});
    value["security"] = serde_json::json!([]);
    value["x-dd-contract-scope"] = serde_json::Value::String("public".to_owned());
    if let Some(title) = value["info"]["title"].as_str() {
        value["info"]["title"] = serde_json::Value::String(format!("{title} (public)"));
    }
    Ok(serde_json::from_value(value)?)
}

pub fn document_for_scope(internal: &OpenApi, scope: &str) -> anyhow::Result<OpenApi> {
    match scope {
        "internal" => Ok(internal.clone()),
        "public" => public_document(internal),
        other => anyhow::bail!("unknown OpenAPI scope '{other}'; expected public|internal"),
    }
}

pub fn canonical_json(openapi: &OpenApi) -> Result<String, serde_json::Error> {
    let mut json = serde_json::to_string_pretty(openapi)?;
    json.push('\n');
    Ok(json)
}

pub type SharedApiDocs = Arc<ApiDocs>;

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn path_set(value: &serde_json::Value) -> BTreeSet<String> {
        value["paths"]
            .as_object()
            .expect("OpenAPI paths object")
            .keys()
            .cloned()
            .collect()
    }

    #[test]
    fn public_projection_uses_the_runtime_sensitivity_classifier() {
        let internal = super::super::openapi_document().expect("internal OpenAPI document");
        let public = public_document(&internal).expect("public OpenAPI document");
        let internal_value = serde_json::to_value(&internal).expect("serialize internal document");
        let public_value = serde_json::to_value(&public).expect("serialize public document");
        let internal_paths = path_set(&internal_value);
        let public_paths = path_set(&public_value);

        for path in &internal_paths {
            assert_eq!(
                public_paths.contains(path),
                !super::super::requires_ui_token(path),
                "runtime auth and public contract disagree for {path}"
            );
        }

        assert_eq!(public_value["x-dd-contract-scope"], "public");
        assert_eq!(internal_value["x-dd-contract-scope"], "internal");
        assert!(public_value["components"]["securitySchemes"].is_null());
        assert!(!internal_value["components"]["securitySchemes"]["ui_token"].is_null());
    }

    #[test]
    fn canonical_exports_are_stable_and_newline_terminated() {
        let internal = super::super::openapi_document().expect("internal OpenAPI document");
        let first = canonical_json(&internal).expect("first export");
        let second = canonical_json(&internal).expect("second export");
        assert_eq!(first, second);
        assert!(first.ends_with('\n'));
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&first).unwrap()["openapi"],
            "3.1.0"
        );
    }

    #[test]
    fn scope_selection_returns_the_exact_internal_and_public_documents() {
        let internal = super::super::openapi_document().expect("internal OpenAPI document");
        let public = public_document(&internal).expect("public OpenAPI document");
        let selected_internal =
            document_for_scope(&internal, "internal").expect("select internal document");
        let selected_public =
            document_for_scope(&internal, "public").expect("select public document");

        assert_eq!(
            serde_json::to_value(selected_internal).expect("serialize selected internal"),
            serde_json::to_value(&internal).expect("serialize expected internal")
        );
        assert_eq!(
            serde_json::to_value(selected_public).expect("serialize selected public"),
            serde_json::to_value(public).expect("serialize expected public")
        );
    }

    #[test]
    fn unknown_contract_scope_is_rejected() {
        let internal = super::super::openapi_document().expect("internal OpenAPI document");
        let error = match document_for_scope(&internal, "partner") {
            Ok(_) => panic!("unknown scope must fail"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("expected public|internal"));
    }
}
