//! Deterministic public and internal OpenAPI documents for the t2v API.
//!
//! Route paths come from `utoipa_axum::routes!` registrations in `lib.rs`.
//! The public document is assembled only from the public router. The internal
//! document merges that contract with partner-webhook and operator routers,
//! then declares the exact authentication schemes those live routes enforce.

use axum::body::Bytes;
use serde_json::{Map, Value};
use std::sync::Arc;
use utoipa::openapi::OpenApi;
use utoipa_scalar::Scalar;

pub const OPENAPI_CONTENT_TYPE: &str =
    "application/vnd.oai.openapi+json;version=3.1";

#[derive(Clone)]
pub struct ApiDocs {
    pub public_json: Bytes,
    pub internal_json: Bytes,
    pub public_scalar_html: Bytes,
    pub internal_scalar_html: Bytes,
}

pub type SharedApiDocs = Arc<ApiDocs>;

pub struct ApiDocuments {
    pub public: OpenApi,
    pub internal: OpenApi,
}

impl ApiDocs {
    pub fn new(documents: &ApiDocuments) -> Result<Self, String> {
        let public_json = canonical_json(&documents.public)
            .map_err(|error| format!("serializing public OpenAPI: {error}"))?;
        let internal_json = canonical_json(&documents.internal)
            .map_err(|error| format!("serializing internal OpenAPI: {error}"))?;
        Ok(Self {
            public_json: Bytes::from(public_json),
            internal_json: Bytes::from(internal_json),
            public_scalar_html: Bytes::from(Scalar::new(documents.public.clone()).to_html()),
            internal_scalar_html: Bytes::from(Scalar::new(documents.internal.clone()).to_html()),
        })
    }
}

pub fn canonical_json(document: &OpenApi) -> Result<String, serde_json::Error> {
    let mut json = serde_json::to_string_pretty(document)?;
    json.push('\n');
    Ok(json)
}

pub fn finalize(
    public: OpenApi,
    partner: OpenApi,
    operator: OpenApi,
) -> Result<ApiDocuments, String> {
    let mut public_value = serde_json::to_value(public)
        .map_err(|error| format!("serializing public route contract: {error}"))?;
    set_document_metadata(&mut public_value, "public")?;
    remove_security_schemes(&mut public_value)?;

    let mut internal_value = public_value.clone();
    merge_contract(&mut internal_value, partner)?;
    merge_contract(&mut internal_value, operator)?;
    set_document_metadata(&mut internal_value, "internal")?;
    install_internal_security_schemes(&mut internal_value)?;

    let public = serde_json::from_value(public_value)
        .map_err(|error| format!("decoding public OpenAPI: {error}"))?;
    let internal = serde_json::from_value(internal_value)
        .map_err(|error| format!("decoding internal OpenAPI: {error}"))?;
    Ok(ApiDocuments { public, internal })
}

fn document_object(document: &mut Value) -> Result<&mut Map<String, Value>, String> {
    document
        .as_object_mut()
        .ok_or_else(|| "OpenAPI document must be a JSON object".to_string())
}

fn set_document_metadata(document: &mut Value, scope: &str) -> Result<(), String> {
    let object = document_object(document)?;
    object.insert("openapi".to_string(), Value::String("3.1.0".to_string()));
    object.insert(
        "jsonSchemaDialect".to_string(),
        Value::String("https://json-schema.org/draft/2020-12/schema".to_string()),
    );
    object.insert(
        "info".to_string(),
        serde_json::json!({
            "title": format!("t2v-v2t API ({scope})"),
            "version": env!("CARGO_PKG_VERSION"),
            "description": if scope == "public" {
                "Public speech-to-text, text-to-speech, translation, audio-analysis, and deterministic API-reference contract."
            } else {
                "Complete t2v-v2t contract including Vapi partner callbacks, operator call control, history, metrics, and authenticated internal documentation."
            }
        }),
    );
    object.insert(
        "x-dd-contract-scope".to_string(),
        Value::String(scope.to_string()),
    );
    object.insert("x-dd-language".to_string(), Value::String("rust".to_string()));
    Ok(())
}

fn merge_contract(target: &mut Value, extra: OpenApi) -> Result<(), String> {
    let extra = serde_json::to_value(extra)
        .map_err(|error| format!("serializing route contract fragment: {error}"))?;
    for section in ["paths", "components", "tags"] {
        let Some(source) = extra.get(section) else {
            continue;
        };
        let target_object = document_object(target)?;
        let destination = target_object
            .entry(section.to_string())
            .or_insert_with(|| match source {
                Value::Array(_) => Value::Array(Vec::new()),
                _ => Value::Object(Map::new()),
            });
        merge_value(destination, source, section)?;
    }
    Ok(())
}

fn merge_value(target: &mut Value, source: &Value, context: &str) -> Result<(), String> {
    match (target, source) {
        (Value::Object(target), Value::Object(source)) => {
            for (key, value) in source {
                match target.get_mut(key) {
                    None => {
                        target.insert(key.clone(), value.clone());
                    }
                    Some(existing) => {
                        merge_value(existing, value, &format!("{context}.{key}"))?;
                    }
                }
            }
            Ok(())
        }
        (Value::Array(target), Value::Array(source)) => {
            for value in source {
                if !target.contains(value) {
                    target.push(value.clone());
                }
            }
            Ok(())
        }
        (target, source) if target == source => Ok(()),
        _ => Err(format!("OpenAPI contract conflict at {context}")),
    }
}

fn components_object(document: &mut Value) -> Result<&mut Map<String, Value>, String> {
    let object = document_object(document)?;
    object
        .entry("components".to_string())
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .ok_or_else(|| "OpenAPI components must be an object".to_string())
}

fn remove_security_schemes(document: &mut Value) -> Result<(), String> {
    components_object(document)?.remove("securitySchemes");
    Ok(())
}

fn install_internal_security_schemes(document: &mut Value) -> Result<(), String> {
    components_object(document)?.insert(
        "securitySchemes".to_string(),
        serde_json::json!({
            "server_auth": {
                "type": "http",
                "scheme": "bearer",
                "description": "Operator token configured by T2V_SERVER_AUTH_SECRET."
            },
            "vapi_secret": {
                "type": "apiKey",
                "in": "header",
                "name": "x-vapi-secret",
                "description": "Vapi callback secret configured by T2V_VAPI_WEBHOOK_SECRET."
            }
        }),
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_rejects_conflicting_contract_fragments() {
        let mut left = serde_json::json!({"paths": {"/same": {"get": {"operationId": "one"}}}});
        let right = serde_json::json!({"paths": {"/same": {"get": {"operationId": "two"}}}});
        let error = merge_value(&mut left, &right, "root").expect_err("conflict must fail");
        assert!(error.contains("operationId"));
    }

    #[test]
    fn merge_unions_equal_objects_and_distinct_paths() {
        let mut left = serde_json::json!({"paths": {"/one": {"get": {"operationId": "one"}}}});
        let right = serde_json::json!({"paths": {"/two": {"get": {"operationId": "two"}}}});
        merge_value(&mut left, &right, "root").expect("disjoint paths merge");
        assert!(left["paths"]["/one"].is_object());
        assert!(left["paths"]["/two"].is_object());
    }
}
