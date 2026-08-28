//! Executable OpenAPI support for the durable worker control plane.
//!
//! The internal document is generated from the exact `utoipa-axum` routes that
//! are mounted by the service. The public document is projected from that
//! executable contract through an explicit path allowlist and component-ref
//! reachability pass, so new authenticated routes cannot leak into public docs
//! by accident.

use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::Arc,
};

use bytes::Bytes;
use serde_json::{Map, Value};
use utoipa::openapi::OpenApi;
use utoipa_scalar::Scalar;

pub const OPENAPI_CONTENT_TYPE: &str =
    "application/vnd.oai.openapi+json;version=3.1; charset=utf-8";

const HTTP_METHODS: &[&str] = &[
    "get", "post", "put", "patch", "delete", "head", "options", "trace",
];
const PUBLIC_PATHS: &[&str] = &[
    "/",
    "/healthz",
    "/readyz",
    "/metrics",
    "/openapi.json",
    "/api/docs.json",
    "/api/docs",
    "/docs/api",
];

#[derive(Clone)]
pub struct ApiDocs {
    pub public_json: Bytes,
    pub public_html: Bytes,
    pub internal_json: Bytes,
    pub internal_html: Bytes,
}

impl ApiDocs {
    pub fn new(openapi: &OpenApi) -> Result<Self, serde_json::Error> {
        let public = project_public(openapi)?;
        Ok(Self {
            public_json: Bytes::from(canonical_json(&public)?),
            public_html: Bytes::from(Scalar::new(public).to_html()),
            internal_json: Bytes::from(canonical_json(openapi)?),
            internal_html: Bytes::from(Scalar::new(openapi.clone()).to_html()),
        })
    }
}

pub type SharedApiDocs = Arc<ApiDocs>;

/// Add stable service metadata, authentication schemes, visibility annotations,
/// and OpenAPI 3.1 identity to the route-derived document.
pub fn finalize(openapi: OpenApi) -> OpenApi {
    let mut document = serde_json::to_value(openapi).expect("serialize durable-worker OpenAPI");
    document["openapi"] = Value::String("3.1.0".to_string());
    document["info"] = serde_json::json!({
        "title": "dd-durable-worker-server API",
        "version": env!("CARGO_PKG_VERSION"),
        "description": "Language-neutral durable task and DAG orchestration for heterogeneous AI-agent and general-purpose workers. State transitions are explicit durable boundaries rather than deterministic stack-frame replay."
    });
    document["x-dd-contract-scope"] = Value::String("internal".to_string());
    document["x-dd-service"] = Value::String("dd-durable-worker-server".to_string());
    document["x-dd-source-of-truth"] = Value::String("utoipa-axum".to_string());

    let components = object_mut(&mut document, "components");
    let security_schemes = components
        .entry("securitySchemes".to_string())
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .expect("OpenAPI components.securitySchemes must be an object");
    security_schemes.insert(
        "workerAuth".to_string(),
        serde_json::json!({
            "type": "apiKey",
            "in": "header",
            "name": "X-Worker-Auth",
            "description": "Shared internal service/worker credential. X-Server-Auth is accepted only as a compatibility alias and is intentionally not the canonical SDK header."
        }),
    );

    let public = PUBLIC_PATHS.iter().copied().collect::<BTreeSet<_>>();
    let paths = document
        .get_mut("paths")
        .and_then(Value::as_object_mut)
        .expect("route-derived OpenAPI paths must be an object");
    for (path, path_item) in paths {
        let path_item = path_item
            .as_object_mut()
            .unwrap_or_else(|| panic!("OpenAPI path item {path} must be an object"));
        for method in HTTP_METHODS {
            let Some(operation) = path_item.get_mut(*method).and_then(Value::as_object_mut) else {
                continue;
            };
            let is_public = public.contains(path.as_str());
            operation.insert(
                "x-dd-visibility".to_string(),
                Value::String(if is_public { "public" } else { "internal" }.to_string()),
            );
            operation.insert(
                "x-dd-auth".to_string(),
                Value::String(
                    match path.as_str() {
                        "/healthz" | "/readyz" | "/metrics" => "cluster-network-policy",
                        _ if is_public => "public",
                        _ => "worker-secret",
                    }
                    .to_string(),
                ),
            );
        }
    }

    serde_json::from_value(document).expect("deserialize finalized durable-worker OpenAPI")
}

/// Produce the public contract from an already-finalized internal document.
/// Only explicitly allowlisted paths and the component definitions reachable
/// from those paths survive the projection.
pub fn project_public(openapi: &OpenApi) -> Result<OpenApi, serde_json::Error> {
    let mut document = serde_json::to_value(openapi)?;
    let original_components = document
        .get("components")
        .cloned()
        .unwrap_or_else(|| Value::Object(Map::new()));

    let public = PUBLIC_PATHS.iter().copied().collect::<BTreeSet<_>>();
    if let Some(paths) = document.get_mut("paths").and_then(Value::as_object_mut) {
        paths.retain(|path, _| public.contains(path.as_str()));
    }

    let projected_components = project_referenced_components(
        document.get("paths").unwrap_or(&Value::Null),
        &original_components,
    );
    if projected_components.is_empty() {
        document
            .as_object_mut()
            .expect("OpenAPI document must be an object")
            .remove("components");
    } else {
        document["components"] = Value::Object(projected_components);
    }

    document["info"] = serde_json::json!({
        "title": "dd-durable-worker-server API (public)",
        "version": env!("CARGO_PKG_VERSION"),
        "description": "Fail-closed public and operational surface for the durable worker control plane. Worker, run, step, signal, and internal documentation routes are intentionally omitted."
    });
    document["x-dd-contract-scope"] = Value::String("public".to_string());

    serde_json::from_value(document)
}

fn project_referenced_components(
    public_paths: &Value,
    original_components: &Value,
) -> Map<String, Value> {
    let mut pending = BTreeSet::new();
    collect_component_refs(public_paths, &mut pending);

    let mut visited = BTreeSet::new();
    let mut queue = pending.into_iter().collect::<VecDeque<_>>();
    while let Some(reference) = queue.pop_front() {
        if !visited.insert(reference.clone()) {
            continue;
        }
        let Some((section, name)) = parse_component_ref(&reference) else {
            continue;
        };
        let Some(component) = original_components
            .get(&section)
            .and_then(Value::as_object)
            .and_then(|values| values.get(&name))
        else {
            continue;
        };
        let mut nested = BTreeSet::new();
        collect_component_refs(component, &mut nested);
        queue.extend(nested);
    }

    let mut by_section = BTreeMap::<String, BTreeSet<String>>::new();
    for reference in visited {
        if let Some((section, name)) = parse_component_ref(&reference) {
            by_section.entry(section).or_default().insert(name);
        }
    }

    let mut projected = Map::new();
    for (section, names) in by_section {
        let Some(source) = original_components.get(&section).and_then(Value::as_object) else {
            continue;
        };
        let mut values = Map::new();
        for name in names {
            if let Some(value) = source.get(&name) {
                values.insert(name, value.clone());
            }
        }
        if !values.is_empty() {
            projected.insert(section, Value::Object(values));
        }
    }
    projected
}

fn collect_component_refs(value: &Value, output: &mut BTreeSet<String>) {
    match value {
        Value::Object(object) => {
            if let Some(reference) = object.get("$ref").and_then(Value::as_str) {
                if reference.starts_with("#/components/") {
                    output.insert(reference.to_string());
                }
            }
            for child in object.values() {
                collect_component_refs(child, output);
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_component_refs(item, output);
            }
        }
        _ => {}
    }
}

fn parse_component_ref(reference: &str) -> Option<(String, String)> {
    let rest = reference.strip_prefix("#/components/")?;
    let (section, name) = rest.split_once('/')?;
    Some((decode_json_pointer(section), decode_json_pointer(name)))
}

fn decode_json_pointer(value: &str) -> String {
    value.replace("~1", "/").replace("~0", "~")
}

fn object_mut<'a>(value: &'a mut Value, key: &str) -> &'a mut Map<String, Value> {
    if value.get(key).is_none() {
        value[key] = Value::Object(Map::new());
    }
    value[key]
        .as_object_mut()
        .unwrap_or_else(|| panic!("OpenAPI {key} must be an object"))
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
    sort_json_objects(&mut value);
    let mut json = serde_json::to_string_pretty(&value)?;
    json.push('\n');
    Ok(json)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api;

    #[test]
    fn public_projection_is_fail_closed() {
        let internal = api::openapi_document();
        let public = project_public(&internal).expect("public OpenAPI projection");
        let value = serde_json::to_value(public).expect("serialize public OpenAPI");
        assert_eq!(value["x-dd-contract-scope"], "public");
        assert!(value["paths"].get("/api/v1/runs").is_none());
        assert!(value["paths"].get("/api/docs").is_some());
        assert!(value["components"]["securitySchemes"]
            .get("workerAuth")
            .is_none());
    }

    #[test]
    fn canonical_export_is_deterministic() {
        let document = api::openapi_document();
        assert_eq!(
            canonical_json(&document).expect("first export"),
            canonical_json(&document).expect("second export")
        );
    }
}
