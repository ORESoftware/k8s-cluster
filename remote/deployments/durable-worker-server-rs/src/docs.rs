use std::{collections::BTreeSet, sync::Arc};

use bytes::Bytes;
use serde_json::{Map, Value};
use utoipa::openapi::OpenApi;
use utoipa_scalar::Scalar;

pub const OPENAPI_CONTENT_TYPE: &str = "application/vnd.oai.openapi+json;version=3.1";
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
const HTTP_METHODS: &[&str] = &[
    "get", "post", "put", "patch", "delete", "head", "options", "trace",
];

#[derive(Clone)]
pub struct ApiDocs {
    pub public_json: Bytes,
    pub public_html: Bytes,
    pub internal_json: Bytes,
    pub internal_html: Bytes,
}

impl ApiDocs {
    pub fn new(internal: &OpenApi) -> Result<Self, serde_json::Error> {
        let public = project_public(internal)?;
        Ok(Self {
            public_json: Bytes::from(canonical_json(&public)?),
            public_html: Bytes::from(Scalar::new(public).to_html()),
            internal_json: Bytes::from(canonical_json(internal)?),
            internal_html: Bytes::from(Scalar::new(internal.clone()).to_html()),
        })
    }
}

pub fn finalize(openapi: OpenApi) -> OpenApi {
    let mut document = serde_json::to_value(openapi).expect("serialize generated OpenAPI");
    document["openapi"] = Value::String("3.1.0".to_string());
    document["info"] = serde_json::json!({
        "title": "dd-durable-worker-server API",
        "version": env!("CARGO_PKG_VERSION"),
        "description": "Independent durable execution control plane for one-off tasks and DAG runs. State is persisted with NATS JetStream KV; lifecycle events are published to JetStream and streamed-output writes require a JetStream acknowledgement."
    });
    document["x-dd-service"] = Value::String("dd-durable-worker-server".to_string());
    document["x-dd-contract-scope"] = Value::String("internal".to_string());
    document["x-dd-source-of-truth"] = Value::String("utoipa-axum".to_string());

    let components = object_mut(&mut document, "components");
    let schemes = components
        .entry("securitySchemes".to_string())
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .expect("OpenAPI components.securitySchemes must be an object");
    schemes.insert(
        "workerAuth".to_string(),
        serde_json::json!({
            "type": "apiKey",
            "in": "header",
            "name": "X-Worker-Auth",
            "description": "Shared internal worker/service secret. X-Server-Auth is accepted as a compatibility alias at runtime."
        }),
    );

    let public = PUBLIC_PATHS.iter().copied().collect::<BTreeSet<_>>();
    let paths = document
        .get_mut("paths")
        .and_then(Value::as_object_mut)
        .expect("generated OpenAPI paths must be an object");
    for (path, path_item) in paths {
        let Some(path_item) = path_item.as_object_mut() else {
            continue;
        };
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
                Value::String(if is_public { "public" } else { "worker-secret" }.to_string()),
            );
        }
    }
    serde_json::from_value(document).expect("deserialize finalized OpenAPI")
}

pub fn project_public(internal: &OpenApi) -> Result<OpenApi, serde_json::Error> {
    let mut document = serde_json::to_value(internal)?;
    document["info"]["title"] = Value::String("dd-durable-worker-server API (public)".to_string());
    document["info"]["description"] = Value::String(
        "Fail-closed public documentation surface. Durable worker orchestration routes remain internal."
            .to_string(),
    );
    document["x-dd-contract-scope"] = Value::String("public".to_string());

    let allow = PUBLIC_PATHS.iter().copied().collect::<BTreeSet<_>>();
    if let Some(paths) = document.get_mut("paths").and_then(Value::as_object_mut) {
        paths.retain(|path, _| allow.contains(path.as_str()));
    }
    if let Some(object) = document.as_object_mut() {
        object.remove("security");
    }
    if let Some(components) = document.get_mut("components").and_then(Value::as_object_mut) {
        components.remove("securitySchemes");
    }
    retain_referenced_schemas(&mut document);
    serde_json::from_value(document)
}

pub fn canonical_json(openapi: &OpenApi) -> Result<String, serde_json::Error> {
    let mut value = serde_json::to_value(openapi)?;
    sort_json_objects(&mut value);
    let mut json = serde_json::to_string_pretty(&value)?;
    json.push('\n');
    Ok(json)
}

fn object_mut<'a>(value: &'a mut Value, key: &str) -> &'a mut Map<String, Value> {
    if value.get(key).is_none() {
        value[key] = Value::Object(Map::new());
    }
    value[key]
        .as_object_mut()
        .unwrap_or_else(|| panic!("OpenAPI {key} must be an object"))
}

fn retain_referenced_schemas(document: &mut Value) {
    let mut referenced = BTreeSet::new();
    if let Some(paths) = document.get("paths") {
        collect_schema_refs(paths, &mut referenced);
    }

    loop {
        let before = referenced.len();
        let Some(schemas) = document
            .pointer("/components/schemas")
            .and_then(Value::as_object)
        else {
            break;
        };
        for name in referenced.clone() {
            if let Some(schema) = schemas.get(&name) {
                collect_schema_refs(schema, &mut referenced);
            }
        }
        if referenced.len() == before {
            break;
        }
    }

    if let Some(schemas) = document
        .pointer_mut("/components/schemas")
        .and_then(Value::as_object_mut)
    {
        schemas.retain(|name, _| referenced.contains(name));
    }
}

fn collect_schema_refs(value: &Value, output: &mut BTreeSet<String>) {
    match value {
        Value::Object(object) => {
            if let Some(reference) = object.get("$ref").and_then(Value::as_str) {
                if let Some(name) = reference.strip_prefix("#/components/schemas/") {
                    output.insert(name.to_string());
                }
            }
            for child in object.values() {
                collect_schema_refs(child, output);
            }
        }
        Value::Array(values) => {
            for child in values {
                collect_schema_refs(child, output);
            }
        }
        _ => {}
    }
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
        Value::Array(values) => {
            for child in values {
                sort_json_objects(child);
            }
        }
        _ => {}
    }
}

pub type SharedApiDocs = Arc<ApiDocs>;
