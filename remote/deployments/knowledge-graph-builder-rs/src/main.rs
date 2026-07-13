use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    env,
    error::Error,
    net::SocketAddr,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, RwLock,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use axum::{
    extract::{DefaultBodyLimit, Query, State},
    http::{HeaderMap, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use dd_nats_subject_defs::{
    KNOWLEDGE_GRAPH_BUILD_REQUESTS_QUEUE_GROUP, KNOWLEDGE_GRAPH_BUILD_REQUESTS_SUBJECT,
    KNOWLEDGE_GRAPH_PIPELINE_JOBS_SUBJECT, KNOWLEDGE_GRAPH_RESULTS_SUBJECT,
    KNOWLEDGE_GRAPH_UPDATES_SUBJECT, RUNTIME_EVENTS_SUBJECT,
};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

const SERVICE_NAME: &str = "dd-knowledge-graph-builder";
const SCHEMA_VERSION: &str = "knowledge_graph.build.v1";
const MAX_HTTP_BODY_BYTES: usize = 4 * 1024 * 1024;
const MAX_NATS_PAYLOAD_BYTES: usize = 4 * 1024 * 1024;
const MAX_NODES_PER_REQUEST: usize = 2_000;
const MAX_EDGES_PER_REQUEST: usize = 4_000;
const MAX_RECORDS_PER_REQUEST: usize = 512;
const MAX_NODE_STORE: usize = 50_000;
const MAX_EDGE_STORE: usize = 200_000;
const MAX_PIPELINE_JOBS: usize = 2_000;
const MAX_TEXT_LEN: usize = 4_096;
const MAX_LONG_TEXT_LEN: usize = 24_000;
const MAX_TOKEN_LEN: usize = 160;
const MAX_ALIASES: usize = 32;
const MAX_QUERY_DEPTH: usize = 6;
const MAX_PATH_DEPTH: usize = 12;
const MAX_SUBGRAPH_NODES: usize = 2_000;
const DEFAULT_CENTRALITY_TOP: usize = 25;
const MAX_ENTITIES_PER_RECORD: usize = 64;
const MAX_PROPERTY_KEYS: usize = 64;
// Cap on the serialized size of any caller-supplied arbitrary-JSON blob we retain
// (node/edge `properties`), so a stream of large requests cannot exhaust the
// in-process store one bounded record at a time.
const MAX_JSON_VALUE_BYTES: usize = 16 * 1024;
// Co-occurrence edge construction is O(n^2) in the entities of a record; bound
// both the fan-out per record and the total edges generated per request so a
// single request cannot pin the global write lock building millions of edges.
const MAX_COOCCURRENCE_ENTITIES: usize = 32;
const MAX_COOCCURRENCE_EDGES_PER_REQUEST: usize = 20_000;
const DEFAULT_EXPORT_LIMIT: usize = 5_000;
const MAX_EXPORT_LIMIT: usize = 50_000;

#[derive(Clone)]
struct AppState {
    config: Arc<Config>,
    metrics: Arc<Metrics>,
    nats: Option<async_nats::Client>,
    store: Arc<RwLock<GraphStore>>,
}

#[derive(Clone)]
struct Config {
    server_auth_secret: Option<String>,
    allow_unauthenticated: bool,
    build_request_subject: String,
    update_subject: String,
    result_subject: String,
    pipeline_job_subject: String,
    runtime_event_subject: String,
    queue_group: String,
}

#[derive(Default)]
struct Metrics {
    http_requests_total: AtomicU64,
    upsert_requests_total: AtomicU64,
    extract_requests_total: AtomicU64,
    query_requests_total: AtomicU64,
    path_requests_total: AtomicU64,
    centrality_requests_total: AtomicU64,
    nodes_upserted_total: AtomicU64,
    edges_upserted_total: AtomicU64,
    pipeline_jobs_total: AtomicU64,
    auth_failures_total: AtomicU64,
    errors_total: AtomicU64,
    nats_messages_total: AtomicU64,
    nats_published_total: AtomicU64,
}

#[derive(Default)]
struct GraphStore {
    nodes: BTreeMap<String, GraphNode>,
    edges: BTreeMap<String, GraphEdge>,
    // Undirected adjacency for traversal/path queries.
    adjacency: BTreeMap<String, BTreeSet<String>>,
    pipeline_jobs: Vec<PipelineJob>,
    seq: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct GraphNode {
    node_id: String,
    entity_type: String,
    label: String,
    aliases: Vec<String>,
    properties: BTreeMap<String, Value>,
    sources: BTreeSet<String>,
    mentions: u64,
    first_seen_ms: u128,
    last_seen_ms: u128,
    last_seq: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct GraphEdge {
    edge_id: String,
    from: String,
    to: String,
    relation: String,
    weight: f64,
    directed: bool,
    properties: BTreeMap<String, Value>,
    sources: BTreeSet<String>,
    observations: u64,
    first_seen_ms: u128,
    last_seen_ms: u128,
    last_seq: u64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IncomingNode {
    id: Option<String>,
    #[serde(alias = "entityType")]
    r#type: Option<String>,
    label: String,
    aliases: Option<Vec<String>>,
    properties: Option<BTreeMap<String, Value>>,
    source: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IncomingEdge {
    id: Option<String>,
    from: String,
    from_type: Option<String>,
    to: String,
    to_type: Option<String>,
    relation: String,
    weight: Option<f64>,
    directed: Option<bool>,
    properties: Option<BTreeMap<String, Value>>,
    source: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpsertRequest {
    request_id: Option<String>,
    graph_id: Option<String>,
    source: Option<String>,
    nodes: Option<Vec<IncomingNode>>,
    edges: Option<Vec<IncomingEdge>>,
    pipeline: Option<PipelineOptions>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExtractRecord {
    text: Option<String>,
    entities: Option<Vec<IncomingNode>>,
    relations: Option<Vec<IncomingEdge>>,
    source: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExtractRequest {
    request_id: Option<String>,
    graph_id: Option<String>,
    source: Option<String>,
    records: Vec<ExtractRecord>,
    cooccurrence: Option<bool>,
    cooccurrence_relation: Option<String>,
    pipeline: Option<PipelineOptions>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct QueryRequest {
    request_id: Option<String>,
    node_id: Option<String>,
    label: Option<String>,
    #[serde(alias = "entityType")]
    r#type: Option<String>,
    depth: Option<usize>,
    relation: Option<String>,
    limit: Option<usize>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PathRequest {
    request_id: Option<String>,
    from: String,
    from_type: Option<String>,
    to: String,
    to_type: Option<String>,
    max_depth: Option<usize>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CentralityRequest {
    request_id: Option<String>,
    top: Option<usize>,
    #[serde(alias = "entityType")]
    r#type: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct CentralityEntry {
    node_id: String,
    label: String,
    entity_type: String,
    degree: usize,
    weighted_degree: f64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExportQuery {
    limit: Option<usize>,
    offset: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PipelineOptions {
    enabled: Option<bool>,
    job_type: Option<String>,
    sink: Option<String>,
    airflow_dag: Option<String>,
    spark_app: Option<String>,
    parameters: Option<Value>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PipelineRequest {
    request_id: Option<String>,
    job_type: Option<String>,
    graph_ids: Option<Vec<String>>,
    sink: Option<String>,
    airflow_dag: Option<String>,
    spark_app: Option<String>,
    parameters: Option<Value>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct PipelineJob {
    job_id: String,
    request_id: String,
    job_type: String,
    status: String,
    graph_ids: Vec<String>,
    sink: String,
    airflow_dag: Option<String>,
    spark_app: Option<String>,
    parameters: Value,
    node_count: usize,
    edge_count: usize,
    submitted_at_ms: u128,
}

#[derive(Debug, Default, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct MutationSummary {
    nodes_created: usize,
    nodes_updated: usize,
    edges_created: usize,
    edges_updated: usize,
}

enum AuthFailure {
    MissingSecret,
    Unauthorized,
}

fn env_value(key: &str, fallback: &str) -> String {
    env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| fallback.to_string())
}

fn optional_env(key: &str) -> Option<String> {
    env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn env_bool(key: &str, fallback: bool) -> bool {
    env::var(key)
        .ok()
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(fallback)
}

fn config_from_env() -> Config {
    Config {
        server_auth_secret: optional_env("SERVER_AUTH_SECRET")
            .or_else(|| optional_env("KNOWLEDGE_GRAPH_SERVER_AUTH_SECRET")),
        allow_unauthenticated: env_bool("KNOWLEDGE_GRAPH_ALLOW_UNAUTHENTICATED", false),
        build_request_subject: env_value(
            "KNOWLEDGE_GRAPH_BUILD_REQUEST_SUBJECT",
            KNOWLEDGE_GRAPH_BUILD_REQUESTS_SUBJECT,
        ),
        update_subject: env_value(
            "KNOWLEDGE_GRAPH_UPDATE_SUBJECT",
            KNOWLEDGE_GRAPH_UPDATES_SUBJECT,
        ),
        result_subject: env_value(
            "KNOWLEDGE_GRAPH_RESULT_SUBJECT",
            KNOWLEDGE_GRAPH_RESULTS_SUBJECT,
        ),
        pipeline_job_subject: env_value(
            "KNOWLEDGE_GRAPH_PIPELINE_JOB_SUBJECT",
            KNOWLEDGE_GRAPH_PIPELINE_JOBS_SUBJECT,
        ),
        runtime_event_subject: env_value("KNOWLEDGE_GRAPH_RUNTIME_EVENT_SUBJECT", RUNTIME_EVENTS_SUBJECT),
        queue_group: env_value(
            "KNOWLEDGE_GRAPH_QUEUE_GROUP",
            KNOWLEDGE_GRAPH_BUILD_REQUESTS_QUEUE_GROUP,
        ),
    }
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn request_id(input: Option<&String>, fallback: &str) -> String {
    input
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .unwrap_or(fallback)
        .chars()
        .take(MAX_TOKEN_LEN)
        .collect()
}

fn clean_text(value: Option<&String>, max_len: usize) -> Option<String> {
    value
        .map(|text| text.trim())
        .filter(|text| !text.is_empty())
        .map(|text| {
            text.chars()
                .filter(|ch| !ch.is_control())
                .take(max_len)
                .collect()
        })
}

fn clean_required(value: &str, label: &str) -> Result<String, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(format!("{label} must not be empty"));
    }
    if trimmed.chars().any(char::is_control) {
        return Err(format!("{label} must not contain control characters"));
    }
    Ok(trimmed.chars().take(MAX_TEXT_LEN).collect())
}

// Normalize a free-text token into a stable, lowercase, hyphenated slug.
fn slug(value: &str) -> String {
    let lowered = value.trim().to_ascii_lowercase();
    let collapsed = lowered
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch
            } else {
                '-'
            }
        })
        .collect::<String>();
    collapsed
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-")
        .chars()
        .take(MAX_TOKEN_LEN)
        .collect()
}

fn normalize_type(value: Option<&String>) -> String {
    value
        .map(|raw| slug(raw))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "entity".to_string())
}

// Deterministic node id from an explicit id, or from type + label slug.
fn node_id_for(id: Option<&String>, entity_type: &str, label: &str) -> Result<String, String> {
    if let Some(explicit) = id.map(|value| slug(value)).filter(|s| !s.is_empty()) {
        return Ok(explicit);
    }
    let label_slug = slug(label);
    if label_slug.is_empty() {
        return Err("node label must contain at least one alphanumeric character".to_string());
    }
    Ok(format!("{entity_type}:{label_slug}")
        .chars()
        .take(MAX_TOKEN_LEN)
        .collect())
}

fn edge_id_for(from: &str, relation: &str, to: &str, directed: bool) -> String {
    let relation_slug = slug(relation);
    if directed {
        format!("{from}|{relation_slug}|{to}")
    } else {
        // Order-independent id so a<->b and b<->a collapse to one edge.
        let (a, b) = if from <= to { (from, to) } else { (to, from) };
        format!("{a}|{relation_slug}|{b}|u")
    }
    .chars()
    .take(2 * MAX_TOKEN_LEN + 32)
    .collect()
}

fn clean_aliases(values: Option<Vec<String>>) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for value in values.unwrap_or_default() {
        if let Some(text) = clean_text(Some(&value), MAX_TOKEN_LEN) {
            if seen.insert(text.clone()) {
                out.push(text);
            }
        }
        if out.len() >= MAX_ALIASES {
            break;
        }
    }
    out
}

// Length-independent-content comparison so an attacker cannot recover the secret
// byte-by-byte from response timing. (The length itself is allowed to short-circuit.)
fn constant_time_eq(a: &str, b: &str) -> bool {
    let a = a.as_bytes();
    let b = b.as_bytes();
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

fn require_auth(headers: &HeaderMap, state: &AppState) -> Result<(), AuthFailure> {
    if state.config.allow_unauthenticated {
        return Ok(());
    }
    let Some(secret) = state.config.server_auth_secret.as_ref() else {
        return Err(AuthFailure::MissingSecret);
    };
    let provided = headers
        .get("x-server-auth")
        .or_else(|| headers.get("auth"))
        .and_then(|value| value.to_str().ok());
    match provided {
        Some(value) if constant_time_eq(value, secret) => Ok(()),
        _ => Err(AuthFailure::Unauthorized),
    }
}

// Truncate to a key cap and reject blobs whose serialized form is too large to retain.
fn bounded_map(map: BTreeMap<String, Value>, label: &str) -> Result<BTreeMap<String, Value>, String> {
    let map: BTreeMap<String, Value> = map.into_iter().take(MAX_PROPERTY_KEYS).collect();
    let size = serde_json::to_vec(&map).map(|bytes| bytes.len()).unwrap_or(usize::MAX);
    if size > MAX_JSON_VALUE_BYTES {
        return Err(format!("{label} exceeds {MAX_JSON_VALUE_BYTES} serialized bytes"));
    }
    Ok(map)
}

fn bounded_value(value: Value, label: &str) -> Result<Value, String> {
    let size = serde_json::to_vec(&value).map(|bytes| bytes.len()).unwrap_or(usize::MAX);
    if size > MAX_JSON_VALUE_BYTES {
        return Err(format!("{label} exceeds {MAX_JSON_VALUE_BYTES} serialized bytes"));
    }
    Ok(value)
}

fn auth_failure_response(state: &AppState, failure: AuthFailure) -> Response {
    state
        .metrics
        .auth_failures_total
        .fetch_add(1, Ordering::Relaxed);
    let message = match failure {
        AuthFailure::MissingSecret => "server auth secret is not configured",
        AuthFailure::Unauthorized => "unauthorized",
    };
    (
        StatusCode::UNAUTHORIZED,
        Json(json!({ "ok": false, "error": message })),
    )
        .into_response()
}

fn durable_token(prefix: &str, source: &str, suffix: &str) -> String {
    let source = slug(source);
    let source = if source.is_empty() {
        "unknown".to_string()
    } else {
        source
    };
    format!("{prefix}-{source}-{suffix}")
        .chars()
        .take(MAX_TOKEN_LEN)
        .collect()
}

// ---- graph mutation -------------------------------------------------------

fn upsert_node(store: &mut GraphStore, incoming: IncomingNode, fallback_source: &str, summary: &mut MutationSummary) -> Result<String, String> {
    let entity_type = normalize_type(incoming.r#type.as_ref());
    let label = clean_required(&incoming.label, "node label")?;
    let node_id = node_id_for(incoming.id.as_ref(), &entity_type, &label)?;
    let source = incoming
        .source
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(fallback_source)
        .to_string();
    let aliases = clean_aliases(incoming.aliases);
    let properties = bounded_map(incoming.properties.unwrap_or_default(), "node properties")?;
    store.seq += 1;
    let seq = store.seq;
    let now = now_ms();
    match store.nodes.get_mut(&node_id) {
        Some(node) => {
            for alias in aliases {
                if node.aliases.len() < MAX_ALIASES && !node.aliases.contains(&alias) {
                    node.aliases.push(alias);
                }
            }
            for (key, value) in properties {
                node.properties.entry(key).or_insert(value);
            }
            if !source.is_empty() {
                node.sources.insert(source);
            }
            node.mentions += 1;
            node.last_seen_ms = now;
            node.last_seq = seq;
            summary.nodes_updated += 1;
        }
        None => {
            let mut sources = BTreeSet::new();
            if !source.is_empty() {
                sources.insert(source);
            }
            store.nodes.insert(
                node_id.clone(),
                GraphNode {
                    node_id: node_id.clone(),
                    entity_type,
                    label,
                    aliases,
                    properties,
                    sources,
                    mentions: 1,
                    first_seen_ms: now,
                    last_seen_ms: now,
                    last_seq: seq,
                },
            );
            store.adjacency.entry(node_id.clone()).or_default();
            summary.nodes_created += 1;
        }
    }
    Ok(node_id)
}

// Resolve an edge endpoint to a node id, auto-creating a thin placeholder node
// when the endpoint references an entity that has not been upserted yet.
fn resolve_endpoint(
    store: &mut GraphStore,
    raw: &str,
    raw_type: Option<&String>,
    fallback_source: &str,
    summary: &mut MutationSummary,
) -> Result<String, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("edge endpoint must not be empty".to_string());
    }
    // If it already matches an existing node id, use it directly.
    if store.nodes.contains_key(trimmed) {
        return Ok(trimmed.to_string());
    }
    upsert_node(
        store,
        IncomingNode {
            id: None,
            r#type: raw_type.cloned(),
            label: trimmed.to_string(),
            aliases: None,
            properties: None,
            source: Some(fallback_source.to_string()),
        },
        fallback_source,
        summary,
    )
}

fn upsert_edge(
    store: &mut GraphStore,
    incoming: IncomingEdge,
    fallback_source: &str,
    summary: &mut MutationSummary,
) -> Result<String, String> {
    let relation = clean_required(&incoming.relation, "relation")?;
    let directed = incoming.directed.unwrap_or(true);
    let weight = incoming.weight.filter(|w| w.is_finite()).unwrap_or(1.0);
    let source = incoming
        .source
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(fallback_source)
        .to_string();
    let from = resolve_endpoint(store, &incoming.from, incoming.from_type.as_ref(), fallback_source, summary)?;
    let to = resolve_endpoint(store, &incoming.to, incoming.to_type.as_ref(), fallback_source, summary)?;
    if from == to {
        return Err("self-loops are not supported".to_string());
    }
    let edge_id = incoming
        .id
        .map(|value| slug(&value))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| edge_id_for(&from, &relation, &to, directed));
    let properties = bounded_map(incoming.properties.unwrap_or_default(), "edge properties")?;
    store.seq += 1;
    let seq = store.seq;
    let now = now_ms();
    match store.edges.get_mut(&edge_id) {
        Some(edge) => {
            // Keep the accumulated weight finite and bounded so repeated observations
            // cannot drive it to non-finite values that would corrupt centrality.
            edge.weight = (edge.weight + weight).clamp(0.0, 1e12);
            edge.observations = edge.observations.saturating_add(1);
            if !source.is_empty() {
                edge.sources.insert(source);
            }
            for (key, value) in properties {
                edge.properties.entry(key).or_insert(value);
            }
            edge.last_seen_ms = now;
            edge.last_seq = seq;
            summary.edges_updated += 1;
        }
        None => {
            let mut sources = BTreeSet::new();
            if !source.is_empty() {
                sources.insert(source);
            }
            store.edges.insert(
                edge_id.clone(),
                GraphEdge {
                    edge_id: edge_id.clone(),
                    from: from.clone(),
                    to: to.clone(),
                    relation,
                    weight,
                    directed,
                    properties,
                    sources,
                    observations: 1,
                    first_seen_ms: now,
                    last_seen_ms: now,
                    last_seq: seq,
                },
            );
            summary.edges_created += 1;
        }
    }
    store.adjacency.entry(from.clone()).or_default().insert(to.clone());
    store.adjacency.entry(to).or_default().insert(from);
    Ok(edge_id)
}

// Evict the oldest nodes/edges (by last-seen sequence) when over capacity so the
// in-process graph stays bounded on the single-node cluster.
fn prune_store(store: &mut GraphStore) {
    if store.edges.len() > MAX_EDGE_STORE {
        let overflow = store.edges.len() - MAX_EDGE_STORE;
        let mut by_seq = store
            .edges
            .values()
            .map(|edge| (edge.last_seq, edge.edge_id.clone()))
            .collect::<Vec<_>>();
        by_seq.sort();
        for (_, edge_id) in by_seq.into_iter().take(overflow) {
            if let Some(edge) = store.edges.remove(&edge_id) {
                if let Some(neighbors) = store.adjacency.get_mut(&edge.from) {
                    neighbors.remove(&edge.to);
                }
                if let Some(neighbors) = store.adjacency.get_mut(&edge.to) {
                    neighbors.remove(&edge.from);
                }
            }
        }
    }
    if store.nodes.len() > MAX_NODE_STORE {
        let overflow = store.nodes.len() - MAX_NODE_STORE;
        let mut by_seq = store
            .nodes
            .values()
            .map(|node| (node.last_seq, node.node_id.clone()))
            .collect::<Vec<_>>();
        by_seq.sort();
        let victims = by_seq
            .into_iter()
            .take(overflow)
            .map(|(_, id)| id)
            .collect::<BTreeSet<_>>();
        store
            .edges
            .retain(|_, edge| !victims.contains(&edge.from) && !victims.contains(&edge.to));
        for victim in &victims {
            store.nodes.remove(victim);
            store.adjacency.remove(victim);
        }
        for neighbors in store.adjacency.values_mut() {
            neighbors.retain(|id| !victims.contains(id));
        }
    }
}

// ---- traversal & analysis -------------------------------------------------

fn resolve_query_node(store: &GraphStore, node_id: Option<&String>, label: Option<&String>, entity_type: Option<&String>) -> Option<String> {
    if let Some(id) = node_id.map(|value| slug(value)).filter(|s| !s.is_empty()) {
        if store.nodes.contains_key(&id) {
            return Some(id);
        }
    }
    if let Some(label) = label {
        let entity_type = normalize_type(entity_type);
        if let Ok(candidate) = node_id_for(None, &entity_type, label) {
            if store.nodes.contains_key(&candidate) {
                return Some(candidate);
            }
        }
        // Fall back to a label/alias scan across any type.
        let needle = label.trim().to_ascii_lowercase();
        return store
            .nodes
            .values()
            .find(|node| {
                node.label.to_ascii_lowercase() == needle
                    || node.aliases.iter().any(|alias| alias.to_ascii_lowercase() == needle)
            })
            .map(|node| node.node_id.clone());
    }
    None
}

// Breadth-first expansion from a seed node up to `depth` hops, optionally filtered
// to a single relation. Returns the induced subgraph.
fn subgraph(store: &GraphStore, seed: &str, depth: usize, relation: Option<&str>, limit: usize) -> (Vec<GraphNode>, Vec<GraphEdge>) {
    let relation_slug = relation.map(slug);
    let mut visited = BTreeSet::new();
    let mut queue = VecDeque::new();
    visited.insert(seed.to_string());
    queue.push_back((seed.to_string(), 0usize));
    while let Some((node_id, dist)) = queue.pop_front() {
        if dist >= depth || visited.len() >= limit {
            continue;
        }
        if let Some(neighbors) = store.adjacency.get(&node_id) {
            for neighbor in neighbors {
                if visited.contains(neighbor) {
                    continue;
                }
                if visited.len() >= limit {
                    break;
                }
                visited.insert(neighbor.clone());
                queue.push_back((neighbor.clone(), dist + 1));
            }
        }
    }
    let nodes = visited
        .iter()
        .filter_map(|id| store.nodes.get(id).cloned())
        .collect::<Vec<_>>();
    let edges = store
        .edges
        .values()
        .filter(|edge| visited.contains(&edge.from) && visited.contains(&edge.to))
        .filter(|edge| {
            relation_slug
                .as_ref()
                .map(|wanted| &slug(&edge.relation) == wanted)
                .unwrap_or(true)
        })
        .cloned()
        .collect::<Vec<_>>();
    (nodes, edges)
}

// Unweighted shortest path between two nodes via BFS over the undirected adjacency.
fn shortest_path(store: &GraphStore, from: &str, to: &str, max_depth: usize) -> Option<Vec<String>> {
    if from == to {
        return Some(vec![from.to_string()]);
    }
    let mut visited = BTreeSet::new();
    let mut parents: BTreeMap<String, String> = BTreeMap::new();
    let mut queue = VecDeque::new();
    visited.insert(from.to_string());
    queue.push_back((from.to_string(), 0usize));
    while let Some((node_id, dist)) = queue.pop_front() {
        if dist >= max_depth {
            continue;
        }
        if let Some(neighbors) = store.adjacency.get(&node_id) {
            for neighbor in neighbors {
                if visited.insert(neighbor.clone()) {
                    parents.insert(neighbor.clone(), node_id.clone());
                    if neighbor == to {
                        // Reconstruct path from `to` back to `from`.
                        let mut path = vec![to.to_string()];
                        let mut cursor = to.to_string();
                        while let Some(parent) = parents.get(&cursor) {
                            path.push(parent.clone());
                            if parent == from {
                                break;
                            }
                            cursor = parent.clone();
                        }
                        path.reverse();
                        return Some(path);
                    }
                    queue.push_back((neighbor.clone(), dist + 1));
                }
            }
        }
    }
    None
}

fn centrality(store: &GraphStore, top: usize, entity_type: Option<&str>) -> Vec<CentralityEntry> {
    let type_slug = entity_type.map(slug).filter(|s| !s.is_empty());
    let mut weighted: BTreeMap<String, f64> = BTreeMap::new();
    for edge in store.edges.values() {
        *weighted.entry(edge.from.clone()).or_default() += edge.weight;
        *weighted.entry(edge.to.clone()).or_default() += edge.weight;
    }
    let mut entries = store
        .nodes
        .values()
        .filter(|node| {
            type_slug
                .as_ref()
                .map(|wanted| &node.entity_type == wanted)
                .unwrap_or(true)
        })
        .map(|node| CentralityEntry {
            node_id: node.node_id.clone(),
            label: node.label.clone(),
            entity_type: node.entity_type.clone(),
            degree: store.adjacency.get(&node.node_id).map(|n| n.len()).unwrap_or(0),
            weighted_degree: weighted.get(&node.node_id).copied().unwrap_or(0.0),
        })
        .collect::<Vec<_>>();
    entries.sort_by(|a, b| {
        b.degree
            .cmp(&a.degree)
            .then(b.weighted_degree.partial_cmp(&a.weighted_degree).unwrap_or(std::cmp::Ordering::Equal))
            .then(a.node_id.cmp(&b.node_id))
    });
    entries.truncate(top.clamp(1, 1_000));
    entries
}

// ---- naive entity extraction ---------------------------------------------

// Extract candidate entity surface forms from free text by collecting runs of
// capitalized words. Deliberately simple: callers that already have entities
// should pass them explicitly; this is a best-effort fallback.
fn extract_entities_from_text(text: &str) -> Vec<String> {
    let mut entities = Vec::new();
    let mut current: Vec<&str> = Vec::new();
    let flush = |current: &mut Vec<&str>, entities: &mut Vec<String>| {
        if !current.is_empty() {
            let phrase = current.join(" ");
            if phrase.len() >= 2 {
                entities.push(phrase);
            }
            current.clear();
        }
    };
    for token in text.split_whitespace() {
        let trimmed = token.trim_matches(|c: char| !c.is_alphanumeric());
        let is_capitalized = trimmed
            .chars()
            .next()
            .map(|c| c.is_uppercase())
            .unwrap_or(false)
            && trimmed.len() > 1;
        if is_capitalized {
            current.push(trimmed);
            if current.len() >= 5 {
                flush(&mut current, &mut entities);
            }
        } else {
            flush(&mut current, &mut entities);
        }
        if entities.len() >= MAX_ENTITIES_PER_RECORD {
            break;
        }
    }
    flush(&mut current, &mut entities);
    let mut seen = BTreeSet::new();
    entities
        .into_iter()
        .filter(|phrase| seen.insert(phrase.to_ascii_lowercase()))
        .take(MAX_ENTITIES_PER_RECORD)
        .collect()
}

// ---- request processing ---------------------------------------------------

fn graph_id_or_default(graph_id: Option<&String>, fallback: &str) -> String {
    graph_id
        .map(|value| slug(value))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| slug(fallback))
}

async fn process_upsert(state: &AppState, request: UpsertRequest) -> Result<Value, String> {
    let nodes = request.nodes.unwrap_or_default();
    let edges = request.edges.unwrap_or_default();
    if nodes.len() > MAX_NODES_PER_REQUEST {
        return Err(format!("nodes length must be at most {MAX_NODES_PER_REQUEST}"));
    }
    if edges.len() > MAX_EDGES_PER_REQUEST {
        return Err(format!("edges length must be at most {MAX_EDGES_PER_REQUEST}"));
    }
    if nodes.is_empty() && edges.is_empty() {
        return Err("upsert request must include at least one node or edge".to_string());
    }
    let request_id = request_id(request.request_id.as_ref(), "upsert");
    let fallback_source = request
        .source
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("upsert")
        .to_string();
    let graph_id = graph_id_or_default(request.graph_id.as_ref(), &fallback_source);

    let mut summary = MutationSummary::default();
    {
        let mut store = state.store.write().unwrap_or_else(|lock| lock.into_inner());
        for node in nodes {
            upsert_node(&mut store, node, &fallback_source, &mut summary)?;
        }
        for edge in edges {
            upsert_edge(&mut store, edge, &fallback_source, &mut summary)?;
        }
        prune_store(&mut store);
    }
    finish_mutation(state, &request_id, &graph_id, summary).await
}

async fn process_extract(state: &AppState, request: ExtractRequest) -> Result<Value, String> {
    if request.records.len() > MAX_RECORDS_PER_REQUEST {
        return Err(format!("records length must be at most {MAX_RECORDS_PER_REQUEST}"));
    }
    if request.records.is_empty() {
        return Err("extract request must include at least one record".to_string());
    }
    let request_id = request_id(request.request_id.as_ref(), "extract");
    let fallback_source = request
        .source
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("extract")
        .to_string();
    let graph_id = graph_id_or_default(request.graph_id.as_ref(), &fallback_source);
    let cooccurrence = request.cooccurrence.unwrap_or(true);
    let cooccurrence_relation = request
        .cooccurrence_relation
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("co-occurs-with")
        .to_string();

    let mut summary = MutationSummary::default();
    let mut cooccurrence_edges = 0usize;
    {
        let mut store = state.store.write().unwrap_or_else(|lock| lock.into_inner());
        for record in request.records {
            let record_source = record
                .source
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or(&fallback_source)
                .to_string();
            let mut record_node_ids: Vec<String> = Vec::new();

            // Explicit entities take priority.
            let explicit = record.entities.unwrap_or_default();
            if !explicit.is_empty() {
                for entity in explicit.into_iter().take(MAX_ENTITIES_PER_RECORD) {
                    let id = upsert_node(&mut store, entity, &record_source, &mut summary)?;
                    record_node_ids.push(id);
                }
            } else if let Some(text) = clean_text(record.text.as_ref(), MAX_LONG_TEXT_LEN) {
                for surface in extract_entities_from_text(&text) {
                    let id = upsert_node(
                        &mut store,
                        IncomingNode {
                            id: None,
                            r#type: Some("concept".to_string()),
                            label: surface,
                            aliases: None,
                            properties: None,
                            source: Some(record_source.clone()),
                        },
                        &record_source,
                        &mut summary,
                    )?;
                    record_node_ids.push(id);
                }
            }

            // Explicit relations.
            for relation in record.relations.unwrap_or_default().into_iter().take(MAX_EDGES_PER_REQUEST) {
                upsert_edge(&mut store, relation, &record_source, &mut summary)?;
            }

            // Co-occurrence edges among the entities mentioned in this record.
            if cooccurrence {
                record_node_ids.sort();
                record_node_ids.dedup();
                record_node_ids.truncate(MAX_COOCCURRENCE_ENTITIES);
                'pairs: for i in 0..record_node_ids.len() {
                    for j in (i + 1)..record_node_ids.len() {
                        if cooccurrence_edges >= MAX_COOCCURRENCE_EDGES_PER_REQUEST {
                            break 'pairs;
                        }
                        upsert_edge(
                            &mut store,
                            IncomingEdge {
                                id: None,
                                from: record_node_ids[i].clone(),
                                from_type: None,
                                to: record_node_ids[j].clone(),
                                to_type: None,
                                relation: cooccurrence_relation.clone(),
                                weight: Some(1.0),
                                directed: Some(false),
                                properties: None,
                                source: Some(record_source.clone()),
                            },
                            &record_source,
                            &mut summary,
                        )?;
                        cooccurrence_edges += 1;
                    }
                }
            }
        }
        prune_store(&mut store);
    }
    finish_mutation(state, &request_id, &graph_id, summary).await
}

async fn finish_mutation(state: &AppState, request_id: &str, graph_id: &str, summary: MutationSummary) -> Result<Value, String> {
    state
        .metrics
        .nodes_upserted_total
        .fetch_add((summary.nodes_created + summary.nodes_updated) as u64, Ordering::Relaxed);
    state
        .metrics
        .edges_upserted_total
        .fetch_add((summary.edges_created + summary.edges_updated) as u64, Ordering::Relaxed);
    let (node_count, edge_count) = {
        let store = state.store.read().unwrap_or_else(|lock| lock.into_inner());
        (store.nodes.len(), store.edges.len())
    };
    let response = json!({
        "ok": true,
        "requestId": request_id,
        "graphId": graph_id,
        "schemaVersion": SCHEMA_VERSION,
        "summary": summary,
        "graph": { "nodeCount": node_count, "edgeCount": edge_count },
        "atMs": now_ms()
    });
    publish_json(
        state,
        &state.config.update_subject,
        &json!({
            "type": "knowledge_graph.update",
            "source": SERVICE_NAME,
            "graphId": graph_id,
            "summary": summary,
            "nodeCount": node_count,
            "edgeCount": edge_count,
            "atMs": now_ms()
        }),
    )
    .await;
    publish_runtime_event(
        state,
        "knowledge_graph.update",
        json!({ "graphId": graph_id, "nodesCreated": summary.nodes_created, "edgesCreated": summary.edges_created }),
    )
    .await;
    Ok(response)
}

async fn create_pipeline_job(state: &AppState, request: PipelineRequest) -> Result<PipelineJob, String> {
    let request_id = request_id(request.request_id.as_ref(), "pipeline");
    let parameters = bounded_value(request.parameters.unwrap_or_else(|| json!({})), "pipeline parameters")?;
    let job_id = durable_token("knowledge-graph-job", &request_id, &now_ms().to_string());
    let (node_count, edge_count) = {
        let store = state.store.read().unwrap_or_else(|lock| lock.into_inner());
        (store.nodes.len(), store.edges.len())
    };
    let job = PipelineJob {
        job_id,
        request_id,
        job_type: request
            .job_type
            .unwrap_or_else(|| "graph-analytics".to_string())
            .chars()
            .take(MAX_TOKEN_LEN)
            .collect(),
        status: "queued".to_string(),
        graph_ids: request.graph_ids.unwrap_or_default(),
        sink: request
            .sink
            .unwrap_or_else(|| "minio://knowledge-graph/exports".to_string())
            .chars()
            .take(MAX_TOKEN_LEN)
            .collect(),
        airflow_dag: request.airflow_dag.map(|v| v.chars().take(MAX_TOKEN_LEN).collect()),
        spark_app: request
            .spark_app
            .or_else(|| Some("knowledge-graph-analytics".to_string()))
            .map(|v| v.chars().take(MAX_TOKEN_LEN).collect()),
        parameters,
        node_count,
        edge_count,
        submitted_at_ms: now_ms(),
    };
    {
        let mut store = state.store.write().unwrap_or_else(|lock| lock.into_inner());
        store.pipeline_jobs.push(job.clone());
        if store.pipeline_jobs.len() > MAX_PIPELINE_JOBS {
            let overflow = store.pipeline_jobs.len() - MAX_PIPELINE_JOBS;
            store.pipeline_jobs.drain(0..overflow);
        }
    }
    state.metrics.pipeline_jobs_total.fetch_add(1, Ordering::Relaxed);
    publish_json(
        state,
        &state.config.pipeline_job_subject,
        &json!({
            "schemaVersion": "knowledge_graph.pipeline.job.v1",
            "source": SERVICE_NAME,
            "job": job
        }),
    )
    .await;
    publish_runtime_event(
        state,
        "knowledge_graph.pipeline.job_queued",
        json!({ "jobId": job.job_id, "jobType": job.job_type }),
    )
    .await;
    Ok(job)
}

async fn maybe_submit_pipeline_job(state: &AppState, request_id: &str, graph_id: &str, options: Option<PipelineOptions>) -> Option<PipelineJob> {
    let options = options?;
    if options.enabled == Some(false) {
        return None;
    }
    let request = PipelineRequest {
        request_id: Some(request_id.to_string()),
        job_type: options.job_type,
        graph_ids: Some(vec![graph_id.to_string()]),
        sink: options.sink,
        airflow_dag: options.airflow_dag,
        spark_app: options.spark_app,
        parameters: options.parameters,
    };
    match create_pipeline_job(state, request).await {
        Ok(job) => Some(job),
        Err(error) => {
            state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
            tracing::error!("knowledge-graph pipeline job creation failed: {error}");
            None
        }
    }
}

// ---- nats publish ---------------------------------------------------------

async fn publish_json(state: &AppState, subject: &str, value: &Value) {
    let Some(nats) = state.nats.as_ref() else {
        return;
    };
    match serde_json::to_vec(value) {
        Ok(payload) => {
            if nats.publish(subject.to_string(), payload.into()).await.is_ok() {
                state.metrics.nats_published_total.fetch_add(1, Ordering::Relaxed);
            }
        }
        Err(error) => {
            state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
            tracing::error!("knowledge-graph failed to encode nats payload: {error}");
        }
    }
}

async fn publish_runtime_event(state: &AppState, event_type: &str, attrs: Value) {
    publish_json(
        state,
        &state.config.runtime_event_subject,
        &json!({
            "type": event_type,
            "source": SERVICE_NAME,
            "atMs": now_ms(),
            "attributes": attrs
        }),
    )
    .await;
}

async fn publish_result(state: &AppState, kind: &str, result: &Value) {
    publish_json(
        state,
        &state.config.result_subject,
        &json!({
            "type": format!("knowledge_graph.{kind}"),
            "source": SERVICE_NAME,
            "result": result
        }),
    )
    .await;
}

// ---- descriptors ----------------------------------------------------------

fn service_descriptor(state: &AppState) -> Value {
    json!({
        "ok": true,
        "service": SERVICE_NAME,
        "schemaVersion": SCHEMA_VERSION,
        "description": "Rust knowledge-graph builder: entity/relation ingestion, naive text extraction, co-occurrence graph construction, neighbor/subgraph and shortest-path queries, degree centrality, and Spark/Airflow graph-analytics handoff.",
        "auth": {
            "operatorAuth": "X-Server-Auth or Auth",
            "allowUnauthenticated": state.config.allow_unauthenticated
        },
        "subjects": {
            "buildRequests": state.config.build_request_subject,
            "updates": state.config.update_subject,
            "results": state.config.result_subject,
            "pipelineJobs": state.config.pipeline_job_subject,
            "runtimeEvents": state.config.runtime_event_subject,
            "queueGroup": state.config.queue_group
        },
        "endpoints": {
            "home": "GET /",
            "descriptor": "GET /descriptor",
            "schema": "GET /schema",
            "example": "GET /example",
            "stats": "GET /graph/stats",
            "export": "GET /graph/export",
            "upsert": "POST /graph/upsert",
            "extract": "POST /graph/extract",
            "query": "POST /graph/query",
            "paths": "POST /graph/paths",
            "centrality": "POST /graph/centrality",
            "pipelineJobs": "POST /pipeline/jobs",
            "healthz": "GET /healthz",
            "readyz": "GET /readyz",
            "metrics": "GET /metrics",
            "apiDocs": "GET /docs/api"
        }
    })
}

fn schema_payload() -> Value {
    json!({
        "ok": true,
        "schemaVersion": SCHEMA_VERSION,
        "contracts": {
            "node": {
                "id": "optional explicit node id; otherwise derived from type+label",
                "type": "entity type, e.g. person | org | concept | dataset",
                "label": "display name (required)",
                "aliases": ["alternate surface forms"],
                "properties": { "anyKey": "bounded JSON value" },
                "source": "optional provenance token"
            },
            "edge": {
                "from": "source node id or label",
                "to": "target node id or label",
                "relation": "predicate, e.g. authored | cites | works-at",
                "weight": "optional positive weight (accumulated on repeat observation)",
                "directed": "default true",
                "fromType": "optional type used when auto-creating the source node",
                "toType": "optional type used when auto-creating the target node"
            },
            "extractRecord": {
                "text": "free text scanned for capitalized entity surface forms when no explicit entities are given",
                "entities": ["explicit node objects"],
                "relations": ["explicit edge objects"]
            }
        },
        "outputs": [
            "mutation summaries (nodes/edges created/updated)",
            "neighbor subgraphs",
            "shortest paths between entities",
            "degree/weighted-degree centrality rankings",
            "node-link graph export",
            "Spark/Airflow graph-analytics pipeline job intents"
        ]
    })
}

fn example_payload() -> Value {
    json!({
        "upsert": {
            "graphId": "research-collab",
            "source": "pubmed",
            "nodes": [
                { "type": "person", "label": "Ada Lovelace" },
                { "type": "org", "label": "Analytical Engine Lab" },
                { "type": "concept", "label": "Computation" }
            ],
            "edges": [
                { "from": "Ada Lovelace", "fromType": "person", "to": "Analytical Engine Lab", "toType": "org", "relation": "works-at" },
                { "from": "Ada Lovelace", "fromType": "person", "to": "Computation", "toType": "concept", "relation": "studies" }
            ],
            "pipeline": { "enabled": true, "jobType": "graph-analytics", "sparkApp": "knowledge-graph-pagerank" }
        },
        "extract": {
            "graphId": "research-collab",
            "source": "plos",
            "cooccurrence": true,
            "records": [
                { "recordId": "doc-1", "text": "Ada Lovelace and Charles Babbage collaborated on the Analytical Engine." }
            ]
        },
        "query": { "label": "Ada Lovelace", "type": "person", "depth": 2 },
        "paths": { "from": "Ada Lovelace", "to": "Computation" }
    })
}

// ---- http handlers --------------------------------------------------------

async fn root() -> Html<&'static str> {
    Html(concat!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">",
        "<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">",
        "<title>dd-knowledge-graph-builder</title></head><body>",
        "<h1>dd-knowledge-graph-builder</h1>",
        "<p>Rust knowledge-graph construction and query service. See <a href=\"/descriptor\">/descriptor</a>, ",
        "<a href=\"/schema\">/schema</a>, <a href=\"/example\">/example</a>, and <a href=\"/docs/api\">/docs/api</a>.</p>",
        "</body></html>"
    ))
}

async fn descriptor(State(state): State<AppState>) -> impl IntoResponse {
    Json(service_descriptor(&state))
}

async fn schema() -> impl IntoResponse {
    Json(schema_payload())
}

async fn example() -> impl IntoResponse {
    Json(example_payload())
}

fn bad_request(state: &AppState, error: String) -> Response {
    state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
    (
        StatusCode::BAD_REQUEST,
        Json(json!({ "ok": false, "error": error })),
    )
        .into_response()
}

async fn upsert_http(State(state): State<AppState>, headers: HeaderMap, Json(request): Json<UpsertRequest>) -> Response {
    state.metrics.http_requests_total.fetch_add(1, Ordering::Relaxed);
    if let Err(failure) = require_auth(&headers, &state) {
        return auth_failure_response(&state, failure);
    }
    state.metrics.upsert_requests_total.fetch_add(1, Ordering::Relaxed);
    let pipeline = request.pipeline.clone();
    let graph_id = graph_id_or_default(request.graph_id.as_ref(), request.source.as_deref().unwrap_or("upsert"));
    let rid = request_id(request.request_id.as_ref(), "upsert");
    match process_upsert(&state, request).await {
        Ok(mut response) => {
            let job = maybe_submit_pipeline_job(&state, &rid, &graph_id, pipeline).await;
            response["pipelineJob"] = json!(job);
            Json(response).into_response()
        }
        Err(error) => bad_request(&state, error),
    }
}

async fn extract_http(State(state): State<AppState>, headers: HeaderMap, Json(request): Json<ExtractRequest>) -> Response {
    state.metrics.http_requests_total.fetch_add(1, Ordering::Relaxed);
    if let Err(failure) = require_auth(&headers, &state) {
        return auth_failure_response(&state, failure);
    }
    state.metrics.extract_requests_total.fetch_add(1, Ordering::Relaxed);
    let pipeline = request.pipeline.clone();
    let graph_id = graph_id_or_default(request.graph_id.as_ref(), request.source.as_deref().unwrap_or("extract"));
    let rid = request_id(request.request_id.as_ref(), "extract");
    match process_extract(&state, request).await {
        Ok(mut response) => {
            let job = maybe_submit_pipeline_job(&state, &rid, &graph_id, pipeline).await;
            response["pipelineJob"] = json!(job);
            Json(response).into_response()
        }
        Err(error) => bad_request(&state, error),
    }
}

async fn query_http(State(state): State<AppState>, headers: HeaderMap, Json(request): Json<QueryRequest>) -> Response {
    state.metrics.http_requests_total.fetch_add(1, Ordering::Relaxed);
    if let Err(failure) = require_auth(&headers, &state) {
        return auth_failure_response(&state, failure);
    }
    state.metrics.query_requests_total.fetch_add(1, Ordering::Relaxed);
    let depth = request.depth.unwrap_or(1).clamp(1, MAX_QUERY_DEPTH);
    let limit = request.limit.unwrap_or(MAX_SUBGRAPH_NODES).clamp(1, MAX_SUBGRAPH_NODES);
    let outcome = {
        let store = state.store.read().unwrap_or_else(|lock| lock.into_inner());
        resolve_query_node(&store, request.node_id.as_ref(), request.label.as_ref(), request.r#type.as_ref())
            .map(|seed| {
                let (nodes, edges) = subgraph(&store, &seed, depth, request.relation.as_deref(), limit);
                (seed, nodes, edges)
            })
    };
    let Some((seed, nodes, edges)) = outcome else {
        return bad_request(&state, "node not found for the given nodeId/label/type".to_string());
    };
    let result = json!({
        "ok": true,
        "requestId": request_id(request.request_id.as_ref(), "query"),
        "seed": seed,
        "depth": depth,
        "nodeCount": nodes.len(),
        "edgeCount": edges.len(),
        "nodes": nodes,
        "edges": edges
    });
    publish_result(&state, "query", &result).await;
    Json(result).into_response()
}

async fn paths_http(State(state): State<AppState>, headers: HeaderMap, Json(request): Json<PathRequest>) -> Response {
    state.metrics.http_requests_total.fetch_add(1, Ordering::Relaxed);
    if let Err(failure) = require_auth(&headers, &state) {
        return auth_failure_response(&state, failure);
    }
    state.metrics.path_requests_total.fetch_add(1, Ordering::Relaxed);
    let max_depth = request.max_depth.unwrap_or(6).clamp(1, MAX_PATH_DEPTH);
    let outcome = {
        let store = state.store.read().unwrap_or_else(|lock| lock.into_inner());
        let from = resolve_query_node(&store, Some(&request.from), Some(&request.from), request.from_type.as_ref());
        let to = resolve_query_node(&store, Some(&request.to), Some(&request.to), request.to_type.as_ref());
        match (from, to) {
            (Some(from), Some(to)) => {
                let path = shortest_path(&store, &from, &to, max_depth);
                let path_nodes = path
                    .as_ref()
                    .map(|ids| ids.iter().filter_map(|id| store.nodes.get(id).cloned()).collect::<Vec<_>>())
                    .unwrap_or_default();
                Some((from, to, path, path_nodes))
            }
            _ => None,
        }
    };
    let Some((from, to, path, path_nodes)) = outcome else {
        return bad_request(&state, "from and/or to node not found".to_string());
    };
    let result = json!({
        "ok": true,
        "requestId": request_id(request.request_id.as_ref(), "paths"),
        "from": from,
        "to": to,
        "found": path.is_some(),
        "hops": path.as_ref().map(|p| p.len().saturating_sub(1)),
        "path": path,
        "pathNodes": path_nodes
    });
    publish_result(&state, "paths", &result).await;
    Json(result).into_response()
}

async fn centrality_http(State(state): State<AppState>, headers: HeaderMap, Json(request): Json<CentralityRequest>) -> Response {
    state.metrics.http_requests_total.fetch_add(1, Ordering::Relaxed);
    if let Err(failure) = require_auth(&headers, &state) {
        return auth_failure_response(&state, failure);
    }
    state.metrics.centrality_requests_total.fetch_add(1, Ordering::Relaxed);
    let top = request.top.unwrap_or(DEFAULT_CENTRALITY_TOP);
    let ranking = {
        let store = state.store.read().unwrap_or_else(|lock| lock.into_inner());
        centrality(&store, top, request.r#type.as_deref())
    };
    let result = json!({
        "ok": true,
        "requestId": request_id(request.request_id.as_ref(), "centrality"),
        "metric": "degree-centrality",
        "count": ranking.len(),
        "ranking": ranking
    });
    publish_result(&state, "centrality", &result).await;
    Json(result).into_response()
}

async fn pipeline_jobs_http(State(state): State<AppState>, headers: HeaderMap, Json(request): Json<PipelineRequest>) -> Response {
    state.metrics.http_requests_total.fetch_add(1, Ordering::Relaxed);
    if let Err(failure) = require_auth(&headers, &state) {
        return auth_failure_response(&state, failure);
    }
    match create_pipeline_job(&state, request).await {
        Ok(job) => Json(json!({ "ok": true, "job": job })).into_response(),
        Err(error) => bad_request(&state, error),
    }
}

async fn graph_stats(State(state): State<AppState>, headers: HeaderMap) -> Response {
    state.metrics.http_requests_total.fetch_add(1, Ordering::Relaxed);
    if let Err(failure) = require_auth(&headers, &state) {
        return auth_failure_response(&state, failure);
    }
    let store = state.store.read().unwrap_or_else(|lock| lock.into_inner());
    let mut type_counts: BTreeMap<String, usize> = BTreeMap::new();
    for node in store.nodes.values() {
        *type_counts.entry(node.entity_type.clone()).or_default() += 1;
    }
    let mut relation_counts: BTreeMap<String, usize> = BTreeMap::new();
    for edge in store.edges.values() {
        *relation_counts.entry(edge.relation.clone()).or_default() += 1;
    }
    let degrees = store.adjacency.values().map(|n| n.len()).collect::<Vec<_>>();
    let max_degree = degrees.iter().copied().max().unwrap_or(0);
    let avg_degree = if degrees.is_empty() {
        0.0
    } else {
        degrees.iter().sum::<usize>() as f64 / degrees.len() as f64
    };
    let top = centrality(&store, 10, None);
    Json(json!({
        "ok": true,
        "service": SERVICE_NAME,
        "nodeCount": store.nodes.len(),
        "edgeCount": store.edges.len(),
        "pipelineJobCount": store.pipeline_jobs.len(),
        "nodeTypeCounts": type_counts,
        "relationCounts": relation_counts,
        "maxDegree": max_degree,
        "avgDegree": avg_degree,
        "topByDegree": top
    }))
    .into_response()
}

async fn graph_export(State(state): State<AppState>, headers: HeaderMap, Query(query): Query<ExportQuery>) -> Response {
    state.metrics.http_requests_total.fetch_add(1, Ordering::Relaxed);
    if let Err(failure) = require_auth(&headers, &state) {
        return auth_failure_response(&state, failure);
    }
    // Paginate so a large graph cannot force an unbounded response/allocation.
    let limit = query.limit.unwrap_or(DEFAULT_EXPORT_LIMIT).clamp(1, MAX_EXPORT_LIMIT);
    let offset = query.offset.unwrap_or(0);
    let store = state.store.read().unwrap_or_else(|lock| lock.into_inner());
    let total_nodes = store.nodes.len();
    let total_edges = store.edges.len();
    let nodes = store.nodes.values().skip(offset).take(limit).cloned().collect::<Vec<_>>();
    let edges = store.edges.values().skip(offset).take(limit).cloned().collect::<Vec<_>>();
    drop(store);
    Json(json!({
        "ok": true,
        "format": "node-link",
        "schemaVersion": SCHEMA_VERSION,
        "totalNodeCount": total_nodes,
        "totalEdgeCount": total_edges,
        "offset": offset,
        "limit": limit,
        "nodeCount": nodes.len(),
        "edgeCount": edges.len(),
        "nodes": nodes,
        "edges": edges
    }))
    .into_response()
}

async fn healthz(State(state): State<AppState>) -> impl IntoResponse {
    let store = state.store.read().unwrap_or_else(|lock| lock.into_inner());
    Json(json!({
        "ok": true,
        "service": SERVICE_NAME,
        "nodeCount": store.nodes.len(),
        "edgeCount": store.edges.len(),
        "pipelineJobCount": store.pipeline_jobs.len()
    }))
}

async fn readyz(State(state): State<AppState>) -> impl IntoResponse {
    Json(json!({
        "ok": true,
        "service": SERVICE_NAME,
        "natsConfigured": state.nats.is_some()
    }))
}

async fn metrics(State(state): State<AppState>) -> Response {
    let m = &state.metrics;
    let body = format!(
        "# HELP dd_knowledge_graph_builder_http_requests_total HTTP requests observed.\n\
         # TYPE dd_knowledge_graph_builder_http_requests_total counter\n\
         dd_knowledge_graph_builder_http_requests_total {}\n\
         # HELP dd_knowledge_graph_builder_upsert_requests_total Graph upsert requests accepted.\n\
         # TYPE dd_knowledge_graph_builder_upsert_requests_total counter\n\
         dd_knowledge_graph_builder_upsert_requests_total {}\n\
         # HELP dd_knowledge_graph_builder_extract_requests_total Text extraction requests accepted.\n\
         # TYPE dd_knowledge_graph_builder_extract_requests_total counter\n\
         dd_knowledge_graph_builder_extract_requests_total {}\n\
         # HELP dd_knowledge_graph_builder_query_requests_total Subgraph query requests accepted.\n\
         # TYPE dd_knowledge_graph_builder_query_requests_total counter\n\
         dd_knowledge_graph_builder_query_requests_total {}\n\
         # HELP dd_knowledge_graph_builder_path_requests_total Shortest-path requests accepted.\n\
         # TYPE dd_knowledge_graph_builder_path_requests_total counter\n\
         dd_knowledge_graph_builder_path_requests_total {}\n\
         # HELP dd_knowledge_graph_builder_centrality_requests_total Centrality requests accepted.\n\
         # TYPE dd_knowledge_graph_builder_centrality_requests_total counter\n\
         dd_knowledge_graph_builder_centrality_requests_total {}\n\
         # HELP dd_knowledge_graph_builder_nodes_upserted_total Nodes created or updated.\n\
         # TYPE dd_knowledge_graph_builder_nodes_upserted_total counter\n\
         dd_knowledge_graph_builder_nodes_upserted_total {}\n\
         # HELP dd_knowledge_graph_builder_edges_upserted_total Edges created or updated.\n\
         # TYPE dd_knowledge_graph_builder_edges_upserted_total counter\n\
         dd_knowledge_graph_builder_edges_upserted_total {}\n\
         # HELP dd_knowledge_graph_builder_pipeline_jobs_total Pipeline job intents queued.\n\
         # TYPE dd_knowledge_graph_builder_pipeline_jobs_total counter\n\
         dd_knowledge_graph_builder_pipeline_jobs_total {}\n\
         # HELP dd_knowledge_graph_builder_auth_failures_total Rejected requests with missing or invalid auth.\n\
         # TYPE dd_knowledge_graph_builder_auth_failures_total counter\n\
         dd_knowledge_graph_builder_auth_failures_total {}\n\
         # HELP dd_knowledge_graph_builder_errors_total Request or publish errors.\n\
         # TYPE dd_knowledge_graph_builder_errors_total counter\n\
         dd_knowledge_graph_builder_errors_total {}\n\
         # HELP dd_knowledge_graph_builder_nats_messages_total NATS build messages consumed.\n\
         # TYPE dd_knowledge_graph_builder_nats_messages_total counter\n\
         dd_knowledge_graph_builder_nats_messages_total {}\n\
         # HELP dd_knowledge_graph_builder_nats_published_total NATS messages published.\n\
         # TYPE dd_knowledge_graph_builder_nats_published_total counter\n\
         dd_knowledge_graph_builder_nats_published_total {}\n",
        m.http_requests_total.load(Ordering::Relaxed),
        m.upsert_requests_total.load(Ordering::Relaxed),
        m.extract_requests_total.load(Ordering::Relaxed),
        m.query_requests_total.load(Ordering::Relaxed),
        m.path_requests_total.load(Ordering::Relaxed),
        m.centrality_requests_total.load(Ordering::Relaxed),
        m.nodes_upserted_total.load(Ordering::Relaxed),
        m.edges_upserted_total.load(Ordering::Relaxed),
        m.pipeline_jobs_total.load(Ordering::Relaxed),
        m.auth_failures_total.load(Ordering::Relaxed),
        m.errors_total.load(Ordering::Relaxed),
        m.nats_messages_total.load(Ordering::Relaxed),
        m.nats_published_total.load(Ordering::Relaxed),
    );
    (
        [(axum::http::header::CONTENT_TYPE, "text/plain; version=0.0.4")],
        body,
    )
        .into_response()
}

async fn api_docs_html() -> Html<&'static str> {
    Html(include_str!("../generated/api-docs.html"))
}

async fn api_docs_json() -> impl IntoResponse {
    (
        [("content-type", "application/json; charset=utf-8")],
        include_str!("../generated/api-docs.json"),
    )
}

async fn run_nats_loop(state: AppState) {
    let Some(nats) = state.nats.clone() else {
        tracing::info!("knowledge-graph nats loop disabled: NATS_URL is not configured");
        return;
    };
    tracing::info!(
        "knowledge-graph nats loop starting: subject={} queueGroup={}",
        state.config.build_request_subject, state.config.queue_group
    );
    loop {
        let mut subscription = match nats
            .queue_subscribe(state.config.build_request_subject.clone(), state.config.queue_group.clone())
            .await
        {
            Ok(subscription) => subscription,
            Err(error) => {
                tracing::error!("knowledge-graph nats subscribe failed: {error}; retrying in 5s");
                tokio::time::sleep(Duration::from_secs(5)).await;
                continue;
            }
        };
        while let Some(message) = subscription.next().await {
            state.metrics.nats_messages_total.fetch_add(1, Ordering::Relaxed);
            let payload = message.payload.to_vec();
            if payload.len() > MAX_NATS_PAYLOAD_BYTES {
                state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
                tracing::error!(
                    "knowledge-graph rejected oversize nats request: bytes={} max={MAX_NATS_PAYLOAD_BYTES}",
                    payload.len()
                );
                continue;
            }
            let task_state = state.clone();
            tokio::spawn(async move {
                match serde_json::from_slice::<Value>(&payload) {
                    Ok(value) => {
                        let result = if value.get("records").is_some() {
                            match serde_json::from_value::<ExtractRequest>(value) {
                                Ok(request) => process_extract(&task_state, request).await,
                                Err(error) => Err(error.to_string()),
                            }
                        } else {
                            match serde_json::from_value::<UpsertRequest>(value) {
                                Ok(request) => process_upsert(&task_state, request).await,
                                Err(error) => Err(error.to_string()),
                            }
                        };
                        if let Err(error) = result {
                            task_state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
                            tracing::error!("knowledge-graph nats request failed: {error}");
                        }
                    }
                    Err(error) => {
                        task_state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
                        tracing::error!("knowledge-graph invalid nats payload: {error}");
                    }
                }
            });
        }
        tracing::error!("knowledge-graph nats subscription ended; re-subscribing in 5s");
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    let _otel = dd_telemetry::init("dd-knowledge-graph-builder");

    let host = env_value("HOST", "0.0.0.0");
    let port = env_value("PORT", "8137").parse::<u16>()?;
    let config = config_from_env();
    // Fail closed at startup: a deploy that forgot the operator secret should
    // crash loudly here instead of silently 401-ing every request, and an
    // unauthenticated deployment must be an explicit, visible opt-in.
    if config.server_auth_secret.is_none() && !config.allow_unauthenticated {
        tracing::error!(
            "{SERVICE_NAME} refusing to start: set SERVER_AUTH_SECRET, or explicitly opt into \
             unauthenticated mode with KNOWLEDGE_GRAPH_ALLOW_UNAUTHENTICATED=true"
        );
        return Err("operator auth is not configured".into());
    }
    if config.allow_unauthenticated {
        tracing::error!(
            "{SERVICE_NAME} WARNING: KNOWLEDGE_GRAPH_ALLOW_UNAUTHENTICATED=true; operator endpoints are UNAUTHENTICATED"
        );
    }
    let nats = match optional_env("NATS_URL") {
        // Degrade gracefully if the broker is down at boot: the HTTP API must come
        // up even when messaging is unavailable. async-nats reconnects on recovery.
        Some(url) => match async_nats::connect(&url).await {
            Ok(client) => Some(client),
            Err(error) => {
                tracing::error!("{SERVICE_NAME} NATS connect failed ({url}): {error}");
                None
            }
        },
        None => None,
    };
    let state = AppState {
        config: Arc::new(config),
        metrics: Arc::new(Metrics::default()),
        nats,
        store: Arc::new(RwLock::new(GraphStore::default())),
    };
    tokio::spawn(run_nats_loop(state.clone()));

    let app = Router::new()
        .route("/", get(root))
        .route("/descriptor", get(descriptor))
        .route("/schema", get(schema))
        .route("/example", get(example))
        .route("/graph/stats", get(graph_stats))
        .route("/graph/export", get(graph_export))
        .route("/graph/upsert", post(upsert_http))
        .route("/graph/extract", post(extract_http))
        .route("/graph/query", post(query_http))
        .route("/graph/paths", post(paths_http))
        .route("/graph/centrality", post(centrality_http))
        .route("/pipeline/jobs", post(pipeline_jobs_http))
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .route("/metrics", get(metrics))
        .route("/docs/api", get(api_docs_html))
        .route("/api/docs", get(api_docs_html))
        .route("/api/docs.json", get(api_docs_json))
        .layer(DefaultBodyLimit::max(MAX_HTTP_BODY_BYTES))
        .with_state(state);

    let addr: SocketAddr = format!("{host}:{port}").parse()?;
    tracing::info!("{SERVICE_NAME} listening on http://{addr}");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app.layer(dd_telemetry::http_trace_layer()))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_state() -> GraphStore {
        GraphStore::default()
    }

    fn add_edge(store: &mut GraphStore, from: &str, to: &str, relation: &str) {
        let mut summary = MutationSummary::default();
        upsert_edge(
            store,
            IncomingEdge {
                id: None,
                from: from.to_string(),
                from_type: Some("person".to_string()),
                to: to.to_string(),
                to_type: Some("person".to_string()),
                relation: relation.to_string(),
                weight: Some(1.0),
                directed: Some(false),
                properties: None,
                source: Some("test".to_string()),
            },
            "test",
            &mut summary,
        )
        .expect("edge upsert should succeed");
    }

    #[test]
    fn node_id_is_deterministic_and_slugged() {
        assert_eq!(node_id_for(None, "person", "Ada Lovelace").unwrap(), "person:ada-lovelace");
        assert_eq!(
            node_id_for(Some(&"Explicit ID".to_string()), "person", "ignored").unwrap(),
            "explicit-id"
        );
        assert!(node_id_for(None, "person", "  !!!  ").is_err());
    }

    #[test]
    fn undirected_edges_collapse_regardless_of_order() {
        let mut store = empty_state();
        add_edge(&mut store, "Ada", "Babbage", "collaborates");
        add_edge(&mut store, "Babbage", "Ada", "collaborates");
        assert_eq!(store.edges.len(), 1);
        let edge = store.edges.values().next().unwrap();
        assert_eq!(edge.observations, 2);
        assert_eq!(edge.weight, 2.0);
    }

    #[test]
    fn shortest_path_finds_chain_and_respects_depth() {
        let mut store = empty_state();
        add_edge(&mut store, "A", "B", "rel");
        add_edge(&mut store, "B", "C", "rel");
        add_edge(&mut store, "C", "D", "rel");
        let a = node_id_for(None, "person", "A").unwrap();
        let d = node_id_for(None, "person", "D").unwrap();
        let path = shortest_path(&store, &a, &d, 6).expect("path should exist");
        assert_eq!(path.len(), 4);
        assert_eq!(path.first().unwrap(), &a);
        assert_eq!(path.last().unwrap(), &d);
        assert!(shortest_path(&store, &a, &d, 2).is_none());
    }

    #[test]
    fn centrality_ranks_hub_first() {
        let mut store = empty_state();
        add_edge(&mut store, "Hub", "A", "rel");
        add_edge(&mut store, "Hub", "B", "rel");
        add_edge(&mut store, "Hub", "C", "rel");
        add_edge(&mut store, "A", "B", "rel");
        let ranking = centrality(&store, 5, None);
        assert_eq!(ranking.first().unwrap().node_id, node_id_for(None, "person", "Hub").unwrap());
        assert_eq!(ranking.first().unwrap().degree, 3);
    }

    #[test]
    fn constant_time_eq_matches_only_identical_strings() {
        assert!(constant_time_eq("super-secret", "super-secret"));
        assert!(!constant_time_eq("super-secret", "super-secres"));
        assert!(!constant_time_eq("super-secret", "super-secret-longer"));
        assert!(!constant_time_eq("", "x"));
    }

    #[test]
    fn bounded_map_rejects_oversized_blobs() {
        let mut map = BTreeMap::new();
        map.insert("k".to_string(), Value::String("x".repeat(MAX_JSON_VALUE_BYTES + 10)));
        assert!(bounded_map(map, "properties").is_err());
        assert!(bounded_map(BTreeMap::new(), "properties").is_ok());
    }

    #[test]
    fn text_extraction_collects_capitalized_phrases() {
        let entities = extract_entities_from_text("Ada Lovelace and Charles Babbage built the Analytical Engine.");
        assert!(entities.iter().any(|e| e == "Ada Lovelace"));
        assert!(entities.iter().any(|e| e == "Charles Babbage"));
        assert!(entities.iter().any(|e| e == "Analytical Engine"));
        assert!(!entities.iter().any(|e| e.to_ascii_lowercase() == "and"));
    }

    #[test]
    fn cooccurrence_extraction_links_entities_in_a_record() {
        let store = Arc::new(RwLock::new(empty_state()));
        let state = AppState {
            config: Arc::new(config_from_env()),
            metrics: Arc::new(Metrics::default()),
            nats: None,
            store: store.clone(),
        };
        let request = ExtractRequest {
            request_id: Some("t".to_string()),
            graph_id: Some("g".to_string()),
            source: Some("test".to_string()),
            cooccurrence: Some(true),
            cooccurrence_relation: None,
            records: vec![ExtractRecord {
                text: Some("Ada Lovelace worked with Charles Babbage.".to_string()),
                entities: None,
                relations: None,
                source: None,
            }],
            pipeline: None,
        };
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(process_extract(&state, request))
            .expect("extract should succeed");
        let guard = store.read().unwrap();
        assert!(guard.nodes.len() >= 2);
        assert!(!guard.edges.is_empty());
    }
}
