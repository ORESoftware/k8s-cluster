//! Executable OpenAPI support for the formal-methods webhook service.
//!
//! Local Axum routes are registered through `utoipa_axum::routes!`. The shared
//! runtime-config crate returns its live router and OpenAPI fragment together.
//! This module composes those executable contracts, records the HMAC and
//! network trust boundaries, and serves only the generated fail-closed public
//! subset through unauthenticated documentation routes.

use std::collections::BTreeSet;
use std::sync::Arc;

use axum::body::Bytes;
use serde_json::{Map, Value};
use utoipa::openapi::OpenApi;
use utoipa_scalar::Scalar;

const PUBLIC_OPENAPI_JSON: &str = include_str!("../generated/api-docs.json");
const HTTP_METHODS: &[&str] = &[
    "get", "post", "put", "patch", "delete", "head", "options", "trace",
];
const PUBLIC_PATHS: &[&str] = &["/openapi.json", "/api/docs.json", "/api/docs", "/docs/api"];
const ANY_JSON_TYPES: [&str; 7] = [
    "object", "array", "string", "number", "integer", "boolean", "null",
];
const SHARED_FREE_FORM_VALUE_POINTERS: [&str; 2] = [
    "/components/schemas/RuntimeConfigEntry/properties/value",
    "/components/schemas/RuntimeConfigEntry/properties/meta",
];
const SHARED_FREE_FORM_MAP_POINTER: &str =
    "/components/schemas/RuntimeConfigSnapshotResponse/properties/entries/additionalProperties";

#[derive(Clone)]
pub struct ApiDocs {
    pub public_json: Bytes,
    pub public_scalar_html: Bytes,
    pub internal_json: Bytes,
    pub internal_scalar_html: Bytes,
}

impl ApiDocs {
    pub fn new(openapi: &OpenApi) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let public_value: Value = serde_json::from_str(PUBLIC_OPENAPI_JSON)?;
        ensure(
            public_value["x-dd-contract-scope"] == "public",
            "embedded runtime OpenAPI must be the fail-closed public contract",
        )?;
        ensure(
            public_value["x-dd-service"] == "formal-methods-service-rs",
            "embedded runtime OpenAPI has unexpected service identity",
        )?;
        ensure(
            public_value["info"]["title"] == "formal-methods-service-rs API (public)",
            "embedded runtime OpenAPI has unexpected service metadata",
        )?;

        Ok(Self {
            public_json: Bytes::from_static(PUBLIC_OPENAPI_JSON.as_bytes()),
            public_scalar_html: Bytes::from(
                Scalar::new(Value::String("/openapi.json".to_string())).to_html(),
            ),
            internal_json: Bytes::from(canonical_json(openapi)?),
            internal_scalar_html: Bytes::from(Scalar::new(openapi.clone()).to_html()),
        })
    }
}

fn ensure(
    condition: bool,
    message: &'static str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if condition {
        Ok(())
    } else {
        Err(message.into())
    }
}

pub fn public_json() -> &'static str {
    PUBLIC_OPENAPI_JSON
}

/// Compose the local and shared executable contracts. Duplicate paths,
/// schemas, or security schemes are rejected unless their JSON is identical;
/// this prevents a host service from silently shadowing a shared route model.
pub fn compose(local: OpenApi, shared: OpenApi) -> OpenApi {
    let mut document = serde_json::to_value(local).expect("serialize local OpenAPI");
    let mut shared = serde_json::to_value(shared).expect("serialize shared OpenAPI");
    make_shared_free_form_schemas_utoipa_compatible(&mut shared);

    merge_object_section(&mut document, &shared, "paths");
    merge_component_sections(&mut document, &shared);
    merge_tags(&mut document, &shared);
    register_github_webhook_security(&mut document);
    finalize_value(&mut document);

    serde_json::from_value(document).expect("deserialize composed OpenAPI")
}

fn object_mut<'a>(value: &'a mut Value, key: &str) -> &'a mut Map<String, Value> {
    if value.get(key).is_none() {
        value[key] = Value::Object(Map::new());
    }
    value[key]
        .as_object_mut()
        .unwrap_or_else(|| panic!("OpenAPI {key} must be an object"))
}

fn merge_object_section(document: &mut Value, incoming: &Value, key: &str) {
    let Some(incoming) = incoming.get(key).and_then(Value::as_object) else {
        return;
    };
    let target = object_mut(document, key);
    for (name, value) in incoming {
        match target.get(name) {
            Some(existing) if existing != value => {
                panic!("OpenAPI {key} collision for {name}")
            }
            Some(_) => {}
            None => {
                target.insert(name.clone(), value.clone());
            }
        }
    }
}

fn merge_component_sections(document: &mut Value, incoming: &Value) {
    let Some(incoming_components) = incoming.get("components").and_then(Value::as_object) else {
        return;
    };
    let components = object_mut(document, "components");
    for (section, incoming_values) in incoming_components {
        let Some(incoming_values) = incoming_values.as_object() else {
            match components.get(section) {
                Some(existing) if existing != incoming_values => {
                    panic!("OpenAPI components collision for {section}")
                }
                Some(_) => {}
                None => {
                    components.insert(section.clone(), incoming_values.clone());
                }
            }
            continue;
        };
        let target = components
            .entry(section.clone())
            .or_insert_with(|| Value::Object(Map::new()))
            .as_object_mut()
            .unwrap_or_else(|| panic!("OpenAPI components.{section} must be an object"));
        for (name, value) in incoming_values {
            match target.get(name) {
                Some(existing) if existing != value => {
                    panic!("OpenAPI components.{section} collision for {name}")
                }
                Some(_) => {}
                None => {
                    target.insert(name.clone(), value.clone());
                }
            }
        }
    }
}

fn merge_tags(document: &mut Value, incoming: &Value) {
    let mut tags = BTreeSet::new();
    for source in [document.get("tags"), incoming.get("tags")] {
        let Some(values) = source.and_then(Value::as_array) else {
            continue;
        };
        for value in values {
            if let Some(name) = value.get("name").and_then(Value::as_str) {
                tags.insert(name.to_string());
            }
        }
    }
    document["tags"] = Value::Array(
        tags.into_iter()
            .map(|name| serde_json::json!({ "name": name }))
            .collect(),
    );
}

fn register_github_webhook_security(document: &mut Value) {
    let components = object_mut(document, "components");
    let security_schemes = components
        .entry("securitySchemes".to_string())
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .expect("OpenAPI components.securitySchemes must be an object");
    let scheme = serde_json::json!({
        "type": "apiKey",
        "in": "header",
        "name": "x-hub-signature-256",
        "description": "GitHub HMAC-SHA256 signature in the exact `sha256=<lowercase hex>` wire format. The SDK accepts a caller-supplied header and never receives or derives the webhook secret."
    });
    match security_schemes.get("github_webhook_signature") {
        Some(existing) if existing != &scheme => {
            panic!("OpenAPI security scheme collision for github_webhook_signature")
        }
        Some(_) => {}
        None => {
            security_schemes.insert("github_webhook_signature".to_string(), scheme);
        }
    }
}

fn finalize_value(document: &mut Value) {
    document["openapi"] = Value::String("3.1.0".to_string());
    document["jsonSchemaDialect"] =
        Value::String("https://json-schema.org/draft/2020-12/schema".to_string());
    document["info"] = serde_json::json!({
        "title": "formal-methods-service-rs API",
        "version": env!("CARGO_PKG_VERSION"),
        "description": "Typed GitHub webhook, operational, and runtime-configuration API for the formal-methods analysis service. The document is composed from the exact handlers and DTOs registered by the live Axum routers."
    });
    document["x-dd-contract-scope"] = Value::String("internal".to_string());
    document["x-dd-service"] = Value::String("formal-methods-service-rs".to_string());
    document["x-dd-source-of-truth"] = Value::String("utoipa-axum".to_string());

    let public = PUBLIC_PATHS.iter().copied().collect::<BTreeSet<_>>();
    let paths = document
        .get_mut("paths")
        .and_then(Value::as_object_mut)
        .expect("composed OpenAPI paths");
    for (path, path_item) in paths {
        let path_item = path_item
            .as_object_mut()
            .unwrap_or_else(|| panic!("OpenAPI path item {path} must be an object"));
        for method in HTTP_METHODS {
            let Some(operation) = path_item.get_mut(*method).and_then(Value::as_object_mut) else {
                continue;
            };
            let visibility = if public.contains(path.as_str()) {
                "public"
            } else {
                "internal"
            };
            operation.insert(
                "x-dd-visibility".to_string(),
                Value::String(visibility.to_string()),
            );
            if !operation.contains_key("x-dd-auth") {
                let auth = if visibility == "public" {
                    Some("public")
                } else {
                    match path.as_str() {
                        "/health" | "/ready" | "/metrics" => Some("cluster-network-policy"),
                        "/webhook/github" => Some("github-webhook-signature"),
                        "/internal/runtime-config"
                        | "/internal/update-runtime-config"
                        | "/internal/runtime-config/reset" => {
                            Some("X-Server-Auth (RUNTIME_CONFIG_SERVER_SECRET)")
                        }
                        _ => None,
                    }
                };
                if let Some(auth) = auth {
                    operation.insert("x-dd-auth".to_string(), Value::String(auth.to_string()));
                }
            }
            if path == "/webhook/github" {
                operation.insert(
                    "x-dd-max-request-body-bytes".to_string(),
                    Value::from(crate::routes::MAX_BODY_BYTES as u64),
                );
            }
        }
    }
}

fn make_shared_free_form_schemas_utoipa_compatible(value: &mut Value) {
    let any_json_types = serde_json::json!(ANY_JSON_TYPES);
    for pointer in SHARED_FREE_FORM_VALUE_POINTERS {
        let schema = value
            .pointer_mut(pointer)
            .unwrap_or_else(|| panic!("missing shared free-form schema at {pointer}"))
            .as_object_mut()
            .unwrap_or_else(|| panic!("shared free-form schema at {pointer} must be an object"));
        match schema.get("type") {
            None => {
                schema.insert("type".to_string(), any_json_types.clone());
            }
            Some(existing) if existing == &any_json_types => {}
            Some(_) => panic!("shared free-form schema at {pointer} was unexpectedly narrowed"),
        }
    }

    let schema = value
        .pointer_mut(SHARED_FREE_FORM_MAP_POINTER)
        .unwrap_or_else(|| {
            panic!("missing shared free-form schema at {SHARED_FREE_FORM_MAP_POINTER}")
        });
    let compatible = serde_json::json!({"type": ANY_JSON_TYPES});
    assert!(
        schema.as_object().is_some_and(Map::is_empty) || schema == &compatible,
        "shared free-form schema at {SHARED_FREE_FORM_MAP_POINTER} was unexpectedly narrowed"
    );
    *schema = compatible;
}

fn restore_shared_free_form_schemas(value: &mut Value) {
    // Shared runtime-config fields intentionally accept arbitrary JSON. Keep
    // that OpenAPI 3.1 meaning instead of narrowing generated SDK input to a
    // temporary Utoipa-deserializable union.
    for pointer in SHARED_FREE_FORM_VALUE_POINTERS {
        let schema = value
            .pointer_mut(pointer)
            .unwrap_or_else(|| panic!("missing shared free-form schema at {pointer}"))
            .as_object_mut()
            .unwrap_or_else(|| panic!("shared free-form schema at {pointer} must be an object"));
        schema.remove("type");
    }

    let schema = value
        .pointer_mut(SHARED_FREE_FORM_MAP_POINTER)
        .unwrap_or_else(|| {
            panic!("missing shared free-form schema at {SHARED_FREE_FORM_MAP_POINTER}")
        });
    *schema = Value::Object(Map::new());
}

fn sort_json_objects(value: &mut Value) {
    match value {
        Value::Object(object) => {
            let mut entries = std::mem::take(object).into_iter().collect::<Vec<_>>();
            entries.sort_by(|(left, _), (right, _)| left.cmp(right));
            for (_, child) in &mut entries {
                sort_json_objects(child);
            }
            object.extend(entries);
        }
        Value::Array(items) => {
            for item in items {
                sort_json_objects(item);
            }
        }
        _ => {}
    }
}

pub fn canonical_json(openapi: &OpenApi) -> Result<String, serde_json::Error> {
    let mut value = serde_json::to_value(openapi)?;
    restore_shared_free_form_schemas(&mut value);
    sort_json_objects(&mut value);
    let mut json = serde_json::to_string_pretty(&value)?;
    json.push('\n');
    Ok(json)
}

pub type SharedApiDocs = Arc<ApiDocs>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_paths_are_fail_closed() {
        assert_eq!(
            PUBLIC_PATHS,
            ["/openapi.json", "/api/docs.json", "/api/docs", "/docs/api"]
        );
    }

    #[test]
    fn canonical_contract_restores_shared_free_form_json() {
        let json = canonical_json(&crate::routes::openapi_document()).expect("canonical OpenAPI");
        let value: Value = serde_json::from_str(&json).expect("parse canonical OpenAPI");
        for pointer in SHARED_FREE_FORM_VALUE_POINTERS {
            let schema = value
                .pointer(pointer)
                .unwrap_or_else(|| panic!("missing shared free-form schema at {pointer}"));
            assert!(schema.get("type").is_none());
        }
        assert_eq!(
            value.pointer(SHARED_FREE_FORM_MAP_POINTER),
            Some(&Value::Object(Map::new()))
        );
    }
}
