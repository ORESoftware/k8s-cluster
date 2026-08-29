//! Executable OpenAPI support for the WAL gateway.
//!
//! Local Axum routes are registered through `utoipa_axum::routes!`. The shared
//! runtime-config crate returns its live router and OpenAPI fragment together.
//! This module composes those two executable contracts, marks the fail-closed
//! public subset explicitly, and renders both machine-readable and Scalar docs.

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
const PUBLIC_PATHS: &[&str] = &[
    "/",
    "/openapi.json",
    "/api/docs.json",
    "/api/docs",
    "/docs/api",
];

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
            public_value["info"]["title"] == "wal-gateway-rs API (public)",
            "embedded runtime OpenAPI has unexpected service metadata",
        )?;
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
    let shared = serde_json::to_value(shared).expect("serialize shared OpenAPI");

    merge_object_section(&mut document, &shared, "paths");
    merge_component_sections(&mut document, &shared);
    merge_tags(&mut document, &shared);
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

fn finalize_value(document: &mut Value) {
    document["info"] = serde_json::json!({
        "title": "wal-gateway-rs API",
        "version": env!("CARGO_PKG_VERSION"),
        "description": "Typed operational and runtime-configuration API for the Postgres-to-NATS WAL gateway. The document is composed from the exact handlers registered by the live Axum routers."
    });
    document["x-dd-contract-scope"] = Value::String("internal".to_string());
    document["x-dd-service"] = Value::String("wal-gateway-rs".to_string());
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
                        "/healthz" | "/readyz" | "/metrics" => Some("cluster-network-policy"),
                        _ => None,
                    }
                };
                if let Some(auth) = auth {
                    operation.insert("x-dd-auth".to_string(), Value::String(auth.to_string()));
                }
            }
        }
    }
}

fn restore_shared_free_form_schemas(value: &mut Value) {
    // The shared handlers model these fields as serde_json::Value and therefore
    // intentionally accept any JSON value. Utoipa can emit that valid OpenAPI
    // 3.1 shape but cannot deserialize it again, so the private-submodule CI
    // shim temporarily uses an explicit union. Restore the authoritative
    // free-form shape before exporting the executable contract and SDK input.
    for pointer in [
        "/components/schemas/RuntimeConfigEntry/properties/value",
        "/components/schemas/RuntimeConfigEntry/properties/meta",
    ] {
        let schema = value
            .pointer_mut(pointer)
            .unwrap_or_else(|| panic!("missing shared free-form schema at {pointer}"))
            .as_object_mut()
            .unwrap_or_else(|| panic!("shared free-form schema at {pointer} must be an object"));
        schema.remove("type");
    }

    let pointer =
        "/components/schemas/RuntimeConfigSnapshotResponse/properties/entries/additionalProperties";
    let schema = value
        .pointer_mut(pointer)
        .unwrap_or_else(|| panic!("missing shared free-form schema at {pointer}"));
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
    // Serializing the Utoipa model directly can expose randomized map order.
    // Normalize every JSON object while preserving semantically ordered arrays.
    let mut value = serde_json::to_value(openapi)?;
    restore_shared_free_form_schemas(&mut value);
    sort_json_objects(&mut value);
    let mut json = serde_json::to_string_pretty(&value)?;
    json.push('\n');
    Ok(json)
}

pub type SharedApiDocs = Arc<ApiDocs>;
