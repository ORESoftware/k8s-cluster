use std::{
    collections::BTreeMap,
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
    DATASET_LABELING_LABEL_EVENTS_SUBJECT, DATASET_LABELING_PIPELINE_JOBS_SUBJECT,
    DATASET_LABELING_RESULTS_SUBJECT, DATASET_LABELING_TASK_REQUESTS_QUEUE_GROUP,
    DATASET_LABELING_TASK_REQUESTS_SUBJECT, RUNTIME_EVENTS_SUBJECT,
};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

const SERVICE_NAME: &str = "dd-dataset-labeling";
const SCHEMA_VERSION: &str = "dataset_labeling.v1";
const MAX_HTTP_BODY_BYTES: usize = 4 * 1024 * 1024;
const MAX_NATS_PAYLOAD_BYTES: usize = 4 * 1024 * 1024;
const MAX_ITEMS_PER_REQUEST: usize = 1_000;
const MAX_LABELS_PER_REQUEST: usize = 1_000;
const MAX_FUNCTIONS_PER_REQUEST: usize = 64;
const MAX_DATASET_STORE: usize = 2_000;
const MAX_ITEMS_PER_DATASET: usize = 100_000;
const MAX_ANNOTATIONS_PER_ITEM: usize = 256;
const MAX_PIPELINE_JOBS: usize = 2_000;
const MAX_SCHEMA_CLASSES: usize = 256;
const MAX_TEXT_LEN: usize = 4_096;
const MAX_LONG_TEXT_LEN: usize = 24_000;
const MAX_TOKEN_LEN: usize = 160;
const MAX_KEYWORDS_PER_FUNCTION: usize = 64;
const DEFAULT_EXPORT_LIMIT: usize = 5_000;
const MAX_EXPORT_LIMIT: usize = 50_000;
// Cap on the serialized size of any caller-supplied arbitrary-JSON blob we retain
// (item `payload`, pipeline `parameters`), so a stream of bounded requests cannot
// exhaust the in-process store.
const MAX_JSON_VALUE_BYTES: usize = 16 * 1024;
// A single weak-supervision apply scans every item in the dataset while holding the
// global write lock; cap the work so one request cannot stall the whole service.
const MAX_FUNCTION_SCAN_ITEMS: usize = 50_000;

#[derive(Clone)]
struct AppState {
    config: Arc<Config>,
    metrics: Arc<Metrics>,
    nats: Option<async_nats::Client>,
    store: Arc<RwLock<LabelStore>>,
}

#[derive(Clone)]
struct Config {
    server_auth_secret: Option<String>,
    allow_unauthenticated: bool,
    task_request_subject: String,
    label_event_subject: String,
    result_subject: String,
    pipeline_job_subject: String,
    runtime_event_subject: String,
    queue_group: String,
}

#[derive(Default)]
struct Metrics {
    http_requests_total: AtomicU64,
    datasets_total: AtomicU64,
    items_created_total: AtomicU64,
    labels_submitted_total: AtomicU64,
    function_labels_total: AtomicU64,
    aggregations_total: AtomicU64,
    exports_total: AtomicU64,
    pipeline_jobs_total: AtomicU64,
    auth_failures_total: AtomicU64,
    errors_total: AtomicU64,
    nats_messages_total: AtomicU64,
    nats_published_total: AtomicU64,
}

#[derive(Default)]
struct LabelStore {
    datasets: BTreeMap<String, Dataset>,
    // dataset_id -> item_id -> item
    items: BTreeMap<String, BTreeMap<String, LabelItem>>,
    pipeline_jobs: Vec<PipelineJob>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Dataset {
    dataset_id: String,
    name: String,
    task_type: String,
    classes: Vec<String>,
    description: Option<String>,
    created_at_ms: u128,
    updated_at_ms: u128,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct LabelItem {
    dataset_id: String,
    item_id: String,
    text: Option<String>,
    payload: Option<Value>,
    tags: Vec<String>,
    created_at_ms: u128,
    annotations: Vec<Annotation>,
    gold: Option<GoldLabel>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Annotation {
    label: String,
    annotator: String,
    annotator_type: String,
    confidence: f64,
    at_ms: u128,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct GoldLabel {
    label: String,
    method: String,
    agreement: f64,
    votes: usize,
    total: usize,
    distribution: BTreeMap<String, usize>,
    at_ms: u128,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DatasetRequest {
    dataset_id: Option<String>,
    name: Option<String>,
    task_type: Option<String>,
    classes: Option<Vec<String>>,
    description: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IncomingItem {
    item_id: Option<String>,
    text: Option<String>,
    payload: Option<Value>,
    tags: Option<Vec<String>>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TasksRequest {
    request_id: Option<String>,
    dataset_id: String,
    items: Vec<IncomingItem>,
    pipeline: Option<PipelineOptions>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IncomingLabel {
    item_id: String,
    label: String,
    annotator: Option<String>,
    annotator_type: Option<String>,
    confidence: Option<f64>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LabelsRequest {
    request_id: Option<String>,
    dataset_id: String,
    labels: Vec<IncomingLabel>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LabelingFunction {
    name: String,
    label: String,
    keywords: Option<Vec<String>>,
    case_sensitive: Option<bool>,
    confidence: Option<f64>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FunctionsRequest {
    request_id: Option<String>,
    dataset_id: String,
    functions: Vec<LabelingFunction>,
    only_unlabeled: Option<bool>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AggregateRequest {
    request_id: Option<String>,
    dataset_id: String,
    annotator_types: Option<Vec<String>>,
    min_votes: Option<usize>,
    pipeline: Option<PipelineOptions>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExportQuery {
    dataset_id: String,
    limit: Option<usize>,
    gold_only: Option<bool>,
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
    dataset_ids: Option<Vec<String>>,
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
    dataset_ids: Vec<String>,
    sink: String,
    airflow_dag: Option<String>,
    spark_app: Option<String>,
    parameters: Value,
    submitted_at_ms: u128,
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
            .or_else(|| optional_env("DATASET_LABELING_SERVER_AUTH_SECRET")),
        allow_unauthenticated: env_bool("DATASET_LABELING_ALLOW_UNAUTHENTICATED", false),
        task_request_subject: env_value(
            "DATASET_LABELING_TASK_REQUEST_SUBJECT",
            DATASET_LABELING_TASK_REQUESTS_SUBJECT,
        ),
        label_event_subject: env_value(
            "DATASET_LABELING_LABEL_EVENT_SUBJECT",
            DATASET_LABELING_LABEL_EVENTS_SUBJECT,
        ),
        result_subject: env_value("DATASET_LABELING_RESULT_SUBJECT", DATASET_LABELING_RESULTS_SUBJECT),
        pipeline_job_subject: env_value(
            "DATASET_LABELING_PIPELINE_JOB_SUBJECT",
            DATASET_LABELING_PIPELINE_JOBS_SUBJECT,
        ),
        runtime_event_subject: env_value("DATASET_LABELING_RUNTIME_EVENT_SUBJECT", RUNTIME_EVENTS_SUBJECT),
        queue_group: env_value(
            "DATASET_LABELING_QUEUE_GROUP",
            DATASET_LABELING_TASK_REQUESTS_QUEUE_GROUP,
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

fn slug(value: &str) -> String {
    let lowered = value.trim().to_ascii_lowercase();
    let collapsed = lowered
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
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

fn clean_token(value: &str, label: &str) -> Result<String, String> {
    let token = slug(value);
    if token.is_empty() {
        return Err(format!("{label} must contain at least one alphanumeric character"));
    }
    Ok(token)
}

fn clean_classes(values: Option<Vec<String>>) -> Vec<String> {
    let mut out = Vec::new();
    for value in values.unwrap_or_default() {
        if let Some(text) = clean_text(Some(&value), MAX_TOKEN_LEN) {
            if !out.contains(&text) {
                out.push(text);
            }
        }
        if out.len() >= MAX_SCHEMA_CLASSES {
            break;
        }
    }
    out
}

fn clean_tags(values: Option<Vec<String>>) -> Vec<String> {
    let mut out = Vec::new();
    for value in values.unwrap_or_default() {
        let token = slug(&value);
        if !token.is_empty() && !out.contains(&token) {
            out.push(token);
        }
        if out.len() >= 32 {
            break;
        }
    }
    out
}

fn durable_token(prefix: &str, source: &str, suffix: &str) -> String {
    let source = slug(source);
    let source = if source.is_empty() { "unknown".to_string() } else { source };
    format!("{prefix}-{source}-{suffix}")
        .chars()
        .take(MAX_TOKEN_LEN)
        .collect()
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

fn bounded_value(value: Value, label: &str) -> Result<Value, String> {
    let size = serde_json::to_vec(&value).map(|bytes| bytes.len()).unwrap_or(usize::MAX);
    if size > MAX_JSON_VALUE_BYTES {
        return Err(format!("{label} exceeds {MAX_JSON_VALUE_BYTES} serialized bytes"));
    }
    Ok(value)
}

fn auth_failure_response(state: &AppState, failure: AuthFailure) -> Response {
    state.metrics.auth_failures_total.fetch_add(1, Ordering::Relaxed);
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

fn bad_request(state: &AppState, error: String) -> Response {
    state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
    (
        StatusCode::BAD_REQUEST,
        Json(json!({ "ok": false, "error": error })),
    )
        .into_response()
}

// Validate a label against a dataset's class schema. An empty schema is open
// (any label accepted), which is useful for free-form or bootstrap labeling.
fn validate_label(classes: &[String], label: &str) -> Result<String, String> {
    let label = clean_required(label, "label")?;
    if classes.is_empty() || classes.iter().any(|c| c == &label) {
        Ok(label)
    } else {
        Err(format!("label '{label}' is not in the dataset class schema"))
    }
}

fn normalize_annotator_type(value: Option<&String>) -> String {
    let raw = value.map(|v| v.trim().to_ascii_lowercase()).unwrap_or_default();
    match raw.as_str() {
        "human" | "model" | "function" | "weak" | "import" => raw,
        "" => "human".to_string(),
        _ => "human".to_string(),
    }
}

// ---- aggregation ----------------------------------------------------------

// Majority-vote aggregation over an item's annotations, restricted to the given
// annotator types when provided. Agreement is the winning vote share.
fn aggregate_item(item: &LabelItem, annotator_types: Option<&[String]>, min_votes: usize) -> Option<GoldLabel> {
    let mut distribution: BTreeMap<String, usize> = BTreeMap::new();
    let mut total = 0usize;
    for annotation in &item.annotations {
        if let Some(types) = annotator_types {
            if !types.iter().any(|t| t == &annotation.annotator_type) {
                continue;
            }
        }
        *distribution.entry(annotation.label.clone()).or_default() += 1;
        total += 1;
    }
    if total < min_votes.max(1) {
        return None;
    }
    // Deterministic winner: highest count, ties broken by label order.
    let (label, votes) = distribution
        .iter()
        .max_by(|a, b| a.1.cmp(b.1).then(b.0.cmp(a.0)))
        .map(|(label, votes)| (label.clone(), *votes))?;
    let agreement = if total == 0 { 0.0 } else { votes as f64 / total as f64 };
    Some(GoldLabel {
        label,
        method: "majority-vote".to_string(),
        votes,
        total,
        agreement,
        distribution,
        at_ms: now_ms(),
    })
}

// ---- request processing ---------------------------------------------------

fn upsert_dataset(store: &mut LabelStore, request: DatasetRequest) -> Result<Dataset, String> {
    let name = request
        .name
        .as_deref()
        .map(|n| clean_required(n, "name"))
        .transpose()?;
    let dataset_id = match request.dataset_id.as_ref() {
        Some(id) => clean_token(id, "datasetId")?,
        None => clean_token(name.as_deref().unwrap_or("dataset"), "datasetId")?,
    };
    let classes = clean_classes(request.classes);
    let task_type = request
        .task_type
        .as_deref()
        .map(|t| slug(t))
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| "classification".to_string());
    let description = clean_text(request.description.as_ref(), MAX_LONG_TEXT_LEN);
    let now = now_ms();
    let dataset = match store.datasets.get_mut(&dataset_id) {
        Some(existing) => {
            if let Some(name) = name {
                existing.name = name;
            }
            existing.task_type = task_type;
            for class in classes {
                if existing.classes.len() < MAX_SCHEMA_CLASSES && !existing.classes.contains(&class) {
                    existing.classes.push(class);
                }
            }
            if description.is_some() {
                existing.description = description;
            }
            existing.updated_at_ms = now;
            existing.clone()
        }
        None => {
            if store.datasets.len() >= MAX_DATASET_STORE {
                return Err(format!("dataset store is full (max {MAX_DATASET_STORE})"));
            }
            let dataset = Dataset {
                dataset_id: dataset_id.clone(),
                name: name.unwrap_or_else(|| dataset_id.clone()),
                task_type,
                classes,
                description,
                created_at_ms: now,
                updated_at_ms: now,
            };
            store.datasets.insert(dataset_id.clone(), dataset.clone());
            store.items.entry(dataset_id).or_default();
            dataset
        }
    };
    Ok(dataset)
}

fn process_tasks(store: &mut LabelStore, request: TasksRequest) -> Result<(String, usize, usize), String> {
    if request.items.len() > MAX_ITEMS_PER_REQUEST {
        return Err(format!("items length must be at most {MAX_ITEMS_PER_REQUEST}"));
    }
    if request.items.is_empty() {
        return Err("tasks request must include at least one item".to_string());
    }
    let dataset_id = clean_token(&request.dataset_id, "datasetId")?;
    if !store.datasets.contains_key(&dataset_id) {
        return Err(format!("dataset '{dataset_id}' is not registered; POST /datasets first"));
    }
    let now = now_ms();
    let mut created = 0usize;
    let mut updated = 0usize;
    let items = store.items.entry(dataset_id.clone()).or_default();
    for (index, incoming) in request.items.into_iter().enumerate() {
        if items.len() >= MAX_ITEMS_PER_DATASET {
            return Err(format!("dataset item limit reached (max {MAX_ITEMS_PER_DATASET})"));
        }
        let item_id = match incoming.item_id.as_ref() {
            Some(id) => clean_token(id, "itemId")?,
            None => durable_token("item", &dataset_id, &format!("{now}-{index}")),
        };
        let text = clean_text(incoming.text.as_ref(), MAX_LONG_TEXT_LEN);
        let tags = clean_tags(incoming.tags);
        let payload = match incoming.payload {
            Some(value) => Some(bounded_value(value, "item payload")?),
            None => None,
        };
        match items.get_mut(&item_id) {
            Some(existing) => {
                if text.is_some() {
                    existing.text = text;
                }
                if payload.is_some() {
                    existing.payload = payload;
                }
                if !tags.is_empty() {
                    existing.tags = tags;
                }
                updated += 1;
            }
            None => {
                items.insert(
                    item_id.clone(),
                    LabelItem {
                        dataset_id: dataset_id.clone(),
                        item_id,
                        text,
                        payload,
                        tags,
                        created_at_ms: now,
                        annotations: Vec::new(),
                        gold: None,
                    },
                );
                created += 1;
            }
        }
    }
    Ok((dataset_id, created, updated))
}

fn record_annotation(item: &mut LabelItem, annotation: Annotation) {
    // Replace any prior annotation from the same annotator (idempotent re-label).
    if let Some(existing) = item
        .annotations
        .iter_mut()
        .find(|a| a.annotator == annotation.annotator && a.annotator_type == annotation.annotator_type)
    {
        *existing = annotation;
        return;
    }
    if item.annotations.len() < MAX_ANNOTATIONS_PER_ITEM {
        item.annotations.push(annotation);
    }
}

fn process_labels(store: &mut LabelStore, request: LabelsRequest) -> Result<(String, usize, Vec<Value>), String> {
    if request.labels.len() > MAX_LABELS_PER_REQUEST {
        return Err(format!("labels length must be at most {MAX_LABELS_PER_REQUEST}"));
    }
    if request.labels.is_empty() {
        return Err("labels request must include at least one label".to_string());
    }
    let dataset_id = clean_token(&request.dataset_id, "datasetId")?;
    let classes = store
        .datasets
        .get(&dataset_id)
        .ok_or_else(|| format!("dataset '{dataset_id}' is not registered"))?
        .classes
        .clone();
    let items = store
        .items
        .get_mut(&dataset_id)
        .ok_or_else(|| format!("dataset '{dataset_id}' has no items"))?;
    let now = now_ms();
    let mut applied = 0usize;
    let mut events = Vec::new();
    for incoming in request.labels {
        let item_id = clean_token(&incoming.item_id, "itemId")?;
        let label = validate_label(&classes, &incoming.label)?;
        let Some(item) = items.get_mut(&item_id) else {
            return Err(format!("item '{item_id}' not found in dataset '{dataset_id}'"));
        };
        let annotator_type = normalize_annotator_type(incoming.annotator_type.as_ref());
        let annotator = incoming
            .annotator
            .as_deref()
            .map(|a| clean_token(a, "annotator"))
            .transpose()?
            .unwrap_or_else(|| format!("{annotator_type}-anon"));
        let confidence = incoming.confidence.filter(|c| c.is_finite()).unwrap_or(1.0).clamp(0.0, 1.0);
        record_annotation(
            item,
            Annotation {
                label: label.clone(),
                annotator: annotator.clone(),
                annotator_type: annotator_type.clone(),
                confidence,
                at_ms: now,
            },
        );
        applied += 1;
        events.push(json!({
            "datasetId": dataset_id,
            "itemId": item_id,
            "label": label,
            "annotator": annotator,
            "annotatorType": annotator_type
        }));
    }
    Ok((dataset_id, applied, events))
}

fn process_functions(store: &mut LabelStore, request: FunctionsRequest) -> Result<(String, usize, usize, bool), String> {
    if request.functions.len() > MAX_FUNCTIONS_PER_REQUEST {
        return Err(format!("functions length must be at most {MAX_FUNCTIONS_PER_REQUEST}"));
    }
    if request.functions.is_empty() {
        return Err("functions request must include at least one labeling function".to_string());
    }
    let dataset_id = clean_token(&request.dataset_id, "datasetId")?;
    let classes = store
        .datasets
        .get(&dataset_id)
        .ok_or_else(|| format!("dataset '{dataset_id}' is not registered"))?
        .classes
        .clone();

    // Pre-validate and normalize the labeling functions.
    struct CompiledFn {
        name: String,
        label: String,
        keywords: Vec<String>,
        case_sensitive: bool,
        confidence: f64,
    }
    let mut compiled = Vec::new();
    for function in request.functions {
        let name = clean_token(&function.name, "function name")?;
        let label = validate_label(&classes, &function.label)?;
        let case_sensitive = function.case_sensitive.unwrap_or(false);
        let keywords = function
            .keywords
            .unwrap_or_default()
            .into_iter()
            .filter_map(|kw| clean_text(Some(&kw), MAX_TOKEN_LEN))
            .map(|kw| if case_sensitive { kw } else { kw.to_ascii_lowercase() })
            .take(MAX_KEYWORDS_PER_FUNCTION)
            .collect::<Vec<_>>();
        if keywords.is_empty() {
            return Err(format!("labeling function '{name}' must include at least one keyword"));
        }
        compiled.push(CompiledFn {
            name,
            label,
            keywords,
            case_sensitive,
            confidence: function.confidence.filter(|c| c.is_finite()).unwrap_or(0.7).clamp(0.0, 1.0),
        });
    }

    let only_unlabeled = request.only_unlabeled.unwrap_or(false);
    let items = store
        .items
        .get_mut(&dataset_id)
        .ok_or_else(|| format!("dataset '{dataset_id}' has no items"))?;
    let now = now_ms();
    let mut labels_applied = 0usize;
    let mut items_touched = 0usize;
    let mut scanned = 0usize;
    let mut truncated = false;
    for item in items.values_mut() {
        if scanned >= MAX_FUNCTION_SCAN_ITEMS {
            truncated = true;
            break;
        }
        scanned += 1;
        let Some(text) = item.text.clone() else {
            continue;
        };
        if only_unlabeled && !item.annotations.is_empty() {
            continue;
        }
        let haystack = text.to_ascii_lowercase();
        let mut touched = false;
        for function in &compiled {
            let hit = function.keywords.iter().any(|kw| {
                if function.case_sensitive {
                    text.contains(kw)
                } else {
                    haystack.contains(kw)
                }
            });
            if hit {
                record_annotation(
                    item,
                    Annotation {
                        label: function.label.clone(),
                        annotator: function.name.clone(),
                        annotator_type: "function".to_string(),
                        confidence: function.confidence,
                        at_ms: now,
                    },
                );
                labels_applied += 1;
                touched = true;
            }
        }
        if touched {
            items_touched += 1;
        }
    }
    Ok((dataset_id, labels_applied, items_touched, truncated))
}

fn process_aggregate(store: &mut LabelStore, request: &AggregateRequest) -> Result<Value, String> {
    let dataset_id = clean_token(&request.dataset_id, "datasetId")?;
    if !store.datasets.contains_key(&dataset_id) {
        return Err(format!("dataset '{dataset_id}' is not registered"));
    }
    let annotator_types = request.annotator_types.as_ref().map(|types| {
        types
            .iter()
            .map(|t| normalize_annotator_type(Some(t)))
            .collect::<Vec<_>>()
    });
    let min_votes = request.min_votes.unwrap_or(1).max(1);
    let items = store
        .items
        .get_mut(&dataset_id)
        .ok_or_else(|| format!("dataset '{dataset_id}' has no items"))?;
    let mut resolved = 0usize;
    let mut unresolved = 0usize;
    let mut agreement_sum = 0.0;
    let mut class_distribution: BTreeMap<String, usize> = BTreeMap::new();
    for item in items.values_mut() {
        match aggregate_item(item, annotator_types.as_deref(), min_votes) {
            Some(gold) => {
                agreement_sum += gold.agreement;
                *class_distribution.entry(gold.label.clone()).or_default() += 1;
                item.gold = Some(gold);
                resolved += 1;
            }
            None => {
                unresolved += 1;
            }
        }
    }
    let mean_agreement = if resolved == 0 { 0.0 } else { agreement_sum / resolved as f64 };
    Ok(json!({
        "datasetId": dataset_id,
        "resolved": resolved,
        "unresolved": unresolved,
        "meanAgreement": mean_agreement,
        "classDistribution": class_distribution
    }))
}

async fn create_pipeline_job(state: &AppState, request: PipelineRequest) -> Result<PipelineJob, String> {
    let request_id = request_id(request.request_id.as_ref(), "pipeline");
    let parameters = bounded_value(request.parameters.unwrap_or_else(|| json!({})), "pipeline parameters")?;
    let job_id = durable_token("dataset-labeling-job", &request_id, &now_ms().to_string());
    let job = PipelineJob {
        job_id,
        request_id,
        job_type: request
            .job_type
            .unwrap_or_else(|| "training-data-export".to_string())
            .chars()
            .take(MAX_TOKEN_LEN)
            .collect(),
        status: "queued".to_string(),
        dataset_ids: request.dataset_ids.unwrap_or_default(),
        sink: request
            .sink
            .unwrap_or_else(|| "minio://datasets/gold".to_string())
            .chars()
            .take(MAX_TOKEN_LEN)
            .collect(),
        airflow_dag: request.airflow_dag.map(|v| v.chars().take(MAX_TOKEN_LEN).collect()),
        spark_app: request
            .spark_app
            .or_else(|| Some("dataset-labeling-materialize".to_string()))
            .map(|v| v.chars().take(MAX_TOKEN_LEN).collect()),
        parameters,
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
            "schemaVersion": "dataset_labeling.pipeline.job.v1",
            "source": SERVICE_NAME,
            "job": job
        }),
    )
    .await;
    publish_runtime_event(
        state,
        "dataset_labeling.pipeline.job_queued",
        json!({ "jobId": job.job_id, "jobType": job.job_type }),
    )
    .await;
    Ok(job)
}

async fn maybe_submit_pipeline_job(state: &AppState, request_id: &str, dataset_id: &str, options: Option<PipelineOptions>) -> Option<PipelineJob> {
    let options = options?;
    if options.enabled == Some(false) {
        return None;
    }
    let request = PipelineRequest {
        request_id: Some(request_id.to_string()),
        job_type: options.job_type,
        dataset_ids: Some(vec![dataset_id.to_string()]),
        sink: options.sink,
        airflow_dag: options.airflow_dag,
        spark_app: options.spark_app,
        parameters: options.parameters,
    };
    match create_pipeline_job(state, request).await {
        Ok(job) => Some(job),
        Err(error) => {
            state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
            tracing::error!("dataset-labeling pipeline job creation failed: {error}");
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
            tracing::error!("dataset-labeling failed to encode nats payload: {error}");
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
            "type": format!("dataset_labeling.{kind}"),
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
        "description": "Rust dataset labeling pipeline: dataset/label-schema registration, labeling task creation, human/model annotation, weak-supervision labeling functions, majority-vote aggregation with inter-annotator agreement, gold-label export, and Spark/Airflow training-data handoff.",
        "auth": {
            "operatorAuth": "X-Server-Auth or Auth",
            "allowUnauthenticated": state.config.allow_unauthenticated
        },
        "subjects": {
            "taskRequests": state.config.task_request_subject,
            "labelEvents": state.config.label_event_subject,
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
            "datasets": "GET /datasets",
            "createDataset": "POST /datasets",
            "tasks": "POST /tasks",
            "labels": "POST /labels",
            "functionsApply": "POST /functions/apply",
            "aggregate": "POST /aggregate",
            "export": "GET /datasets/export",
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
            "dataset": {
                "datasetId": "stable token (derived from name when omitted)",
                "name": "human-readable name",
                "taskType": "classification | ner | ranking | ...",
                "classes": ["label class names; empty means open-vocabulary"],
                "description": "optional bounded text"
            },
            "item": {
                "itemId": "optional stable id; generated when omitted",
                "text": "text scanned by labeling functions",
                "payload": "optional bounded JSON for non-text items",
                "tags": ["routing/slice tags"]
            },
            "label": {
                "itemId": "target item id",
                "label": "class label (validated against dataset schema)",
                "annotator": "annotator id",
                "annotatorType": "human | model | function | weak | import",
                "confidence": "0..1"
            },
            "labelingFunction": {
                "name": "function id",
                "label": "label emitted on match",
                "keywords": ["substring triggers"],
                "caseSensitive": false,
                "confidence": "0..1 weak-label confidence"
            }
        },
        "outputs": [
            "per-item annotations from humans, models, and weak-supervision functions",
            "majority-vote gold labels with inter-annotator agreement",
            "class distribution and mean agreement summaries",
            "gold-label dataset export",
            "Spark/Airflow training-data materialization pipeline job intents"
        ]
    })
}

fn example_payload() -> Value {
    json!({
        "createDataset": {
            "datasetId": "support-intent",
            "name": "Support ticket intent",
            "taskType": "classification",
            "classes": ["billing", "bug", "feature-request", "other"]
        },
        "tasks": {
            "datasetId": "support-intent",
            "items": [
                { "itemId": "t-1", "text": "I was charged twice on my invoice this month." },
                { "itemId": "t-2", "text": "The export button crashes the app." }
            ]
        },
        "functionsApply": {
            "datasetId": "support-intent",
            "functions": [
                { "name": "billing-kw", "label": "billing", "keywords": ["invoice", "charged", "refund"] },
                { "name": "bug-kw", "label": "bug", "keywords": ["crash", "error", "broken"] }
            ]
        },
        "labels": {
            "datasetId": "support-intent",
            "labels": [
                { "itemId": "t-1", "label": "billing", "annotator": "alice", "annotatorType": "human" }
            ]
        },
        "aggregate": { "datasetId": "support-intent", "minVotes": 1, "pipeline": { "enabled": true } }
    })
}

// ---- http handlers --------------------------------------------------------

async fn root() -> Html<&'static str> {
    Html(concat!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">",
        "<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">",
        "<title>dd-dataset-labeling</title></head><body>",
        "<h1>dd-dataset-labeling</h1>",
        "<p>Rust dataset labeling pipeline. See <a href=\"/descriptor\">/descriptor</a>, ",
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

async fn create_dataset_http(State(state): State<AppState>, headers: HeaderMap, Json(request): Json<DatasetRequest>) -> Response {
    state.metrics.http_requests_total.fetch_add(1, Ordering::Relaxed);
    if let Err(failure) = require_auth(&headers, &state) {
        return auth_failure_response(&state, failure);
    }
    let result = {
        let mut store = state.store.write().unwrap_or_else(|lock| lock.into_inner());
        upsert_dataset(&mut store, request)
    };
    match result {
        Ok(dataset) => {
            state.metrics.datasets_total.fetch_add(1, Ordering::Relaxed);
            publish_runtime_event(&state, "dataset_labeling.dataset_registered", json!({ "datasetId": dataset.dataset_id })).await;
            Json(json!({ "ok": true, "dataset": dataset })).into_response()
        }
        Err(error) => bad_request(&state, error),
    }
}

async fn list_datasets_http(State(state): State<AppState>, headers: HeaderMap) -> Response {
    state.metrics.http_requests_total.fetch_add(1, Ordering::Relaxed);
    if let Err(failure) = require_auth(&headers, &state) {
        return auth_failure_response(&state, failure);
    }
    let store = state.store.read().unwrap_or_else(|lock| lock.into_inner());
    let datasets = store
        .datasets
        .values()
        .map(|dataset| {
            let items = store.items.get(&dataset.dataset_id);
            let item_count = items.map(|m| m.len()).unwrap_or(0);
            let labeled = items
                .map(|m| m.values().filter(|i| !i.annotations.is_empty()).count())
                .unwrap_or(0);
            let gold = items.map(|m| m.values().filter(|i| i.gold.is_some()).count()).unwrap_or(0);
            json!({
                "dataset": dataset,
                "itemCount": item_count,
                "annotatedCount": labeled,
                "goldCount": gold
            })
        })
        .collect::<Vec<_>>();
    Json(json!({ "ok": true, "datasets": datasets })).into_response()
}

async fn tasks_http(State(state): State<AppState>, headers: HeaderMap, Json(request): Json<TasksRequest>) -> Response {
    state.metrics.http_requests_total.fetch_add(1, Ordering::Relaxed);
    if let Err(failure) = require_auth(&headers, &state) {
        return auth_failure_response(&state, failure);
    }
    let pipeline = request.pipeline.clone();
    let rid = request_id(request.request_id.as_ref(), "tasks");
    let result = {
        let mut store = state.store.write().unwrap_or_else(|lock| lock.into_inner());
        process_tasks(&mut store, request)
    };
    match result {
        Ok((dataset_id, created, updated)) => {
            state.metrics.items_created_total.fetch_add(created as u64, Ordering::Relaxed);
            let job = maybe_submit_pipeline_job(&state, &rid, &dataset_id, pipeline).await;
            let response = json!({
                "ok": true,
                "requestId": rid,
                "datasetId": dataset_id,
                "itemsCreated": created,
                "itemsUpdated": updated,
                "pipelineJob": job
            });
            publish_runtime_event(&state, "dataset_labeling.tasks", json!({ "datasetId": response["datasetId"], "created": created })).await;
            Json(response).into_response()
        }
        Err(error) => bad_request(&state, error),
    }
}

async fn labels_http(State(state): State<AppState>, headers: HeaderMap, Json(request): Json<LabelsRequest>) -> Response {
    state.metrics.http_requests_total.fetch_add(1, Ordering::Relaxed);
    if let Err(failure) = require_auth(&headers, &state) {
        return auth_failure_response(&state, failure);
    }
    let rid = request_id(request.request_id.as_ref(), "labels");
    let result = {
        let mut store = state.store.write().unwrap_or_else(|lock| lock.into_inner());
        process_labels(&mut store, request)
    };
    match result {
        Ok((dataset_id, applied, events)) => {
            state.metrics.labels_submitted_total.fetch_add(applied as u64, Ordering::Relaxed);
            for event in &events {
                publish_json(&state, &state.config.label_event_subject, &json!({
                    "type": "dataset_labeling.label",
                    "source": SERVICE_NAME,
                    "label": event
                }))
                .await;
            }
            Json(json!({ "ok": true, "requestId": rid, "datasetId": dataset_id, "labelsApplied": applied })).into_response()
        }
        Err(error) => bad_request(&state, error),
    }
}

async fn functions_apply_http(State(state): State<AppState>, headers: HeaderMap, Json(request): Json<FunctionsRequest>) -> Response {
    state.metrics.http_requests_total.fetch_add(1, Ordering::Relaxed);
    if let Err(failure) = require_auth(&headers, &state) {
        return auth_failure_response(&state, failure);
    }
    let rid = request_id(request.request_id.as_ref(), "functions");
    let result = {
        let mut store = state.store.write().unwrap_or_else(|lock| lock.into_inner());
        process_functions(&mut store, request)
    };
    match result {
        Ok((dataset_id, labels_applied, items_touched, truncated)) => {
            state.metrics.function_labels_total.fetch_add(labels_applied as u64, Ordering::Relaxed);
            let response = json!({
                "ok": true,
                "requestId": rid,
                "datasetId": dataset_id,
                "weakLabelsApplied": labels_applied,
                "itemsTouched": items_touched,
                "scanTruncated": truncated
            });
            publish_result(&state, "weak_supervision", &response).await;
            Json(response).into_response()
        }
        Err(error) => bad_request(&state, error),
    }
}

async fn aggregate_http(State(state): State<AppState>, headers: HeaderMap, Json(request): Json<AggregateRequest>) -> Response {
    state.metrics.http_requests_total.fetch_add(1, Ordering::Relaxed);
    if let Err(failure) = require_auth(&headers, &state) {
        return auth_failure_response(&state, failure);
    }
    let pipeline = request.pipeline.clone();
    let rid = request_id(request.request_id.as_ref(), "aggregate");
    let result = {
        let mut store = state.store.write().unwrap_or_else(|lock| lock.into_inner());
        process_aggregate(&mut store, &request)
    };
    match result {
        Ok(mut summary) => {
            state.metrics.aggregations_total.fetch_add(1, Ordering::Relaxed);
            let dataset_id = summary["datasetId"].as_str().unwrap_or_default().to_string();
            let job = maybe_submit_pipeline_job(&state, &rid, &dataset_id, pipeline).await;
            summary["pipelineJob"] = json!(job);
            summary["requestId"] = json!(rid);
            let result = json!({ "ok": true, "aggregation": summary });
            publish_result(&state, "aggregation", &result).await;
            Json(result).into_response()
        }
        Err(error) => bad_request(&state, error),
    }
}

async fn export_http(State(state): State<AppState>, headers: HeaderMap, Query(query): Query<ExportQuery>) -> Response {
    state.metrics.http_requests_total.fetch_add(1, Ordering::Relaxed);
    if let Err(failure) = require_auth(&headers, &state) {
        return auth_failure_response(&state, failure);
    }
    let dataset_id = match clean_token(&query.dataset_id, "datasetId") {
        Ok(id) => id,
        Err(error) => return bad_request(&state, error),
    };
    let limit = query.limit.unwrap_or(DEFAULT_EXPORT_LIMIT).clamp(1, MAX_EXPORT_LIMIT);
    let gold_only = query.gold_only.unwrap_or(true);
    let store = state.store.read().unwrap_or_else(|lock| lock.into_inner());
    let Some(dataset) = store.datasets.get(&dataset_id).cloned() else {
        drop(store);
        return bad_request(&state, format!("dataset '{dataset_id}' is not registered"));
    };
    let rows = store
        .items
        .get(&dataset_id)
        .map(|items| {
            items
                .values()
                .filter(|item| !gold_only || item.gold.is_some())
                .take(limit)
                .map(|item| {
                    json!({
                        "itemId": item.item_id,
                        "text": item.text,
                        "payload": item.payload,
                        "tags": item.tags,
                        "gold": item.gold,
                        "annotationCount": item.annotations.len()
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    drop(store);
    state.metrics.exports_total.fetch_add(1, Ordering::Relaxed);
    Json(json!({
        "ok": true,
        "dataset": dataset,
        "goldOnly": gold_only,
        "rowCount": rows.len(),
        "rows": rows
    }))
    .into_response()
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

async fn healthz(State(state): State<AppState>) -> impl IntoResponse {
    let store = state.store.read().unwrap_or_else(|lock| lock.into_inner());
    let item_count: usize = store.items.values().map(|m| m.len()).sum();
    Json(json!({
        "ok": true,
        "service": SERVICE_NAME,
        "datasetCount": store.datasets.len(),
        "itemCount": item_count,
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
        "# HELP dd_dataset_labeling_http_requests_total HTTP requests observed.\n\
         # TYPE dd_dataset_labeling_http_requests_total counter\n\
         dd_dataset_labeling_http_requests_total {}\n\
         # HELP dd_dataset_labeling_datasets_total Dataset registrations accepted.\n\
         # TYPE dd_dataset_labeling_datasets_total counter\n\
         dd_dataset_labeling_datasets_total {}\n\
         # HELP dd_dataset_labeling_items_created_total Labeling task items created.\n\
         # TYPE dd_dataset_labeling_items_created_total counter\n\
         dd_dataset_labeling_items_created_total {}\n\
         # HELP dd_dataset_labeling_labels_submitted_total Human/model labels submitted.\n\
         # TYPE dd_dataset_labeling_labels_submitted_total counter\n\
         dd_dataset_labeling_labels_submitted_total {}\n\
         # HELP dd_dataset_labeling_function_labels_total Weak-supervision labels applied.\n\
         # TYPE dd_dataset_labeling_function_labels_total counter\n\
         dd_dataset_labeling_function_labels_total {}\n\
         # HELP dd_dataset_labeling_aggregations_total Aggregation runs executed.\n\
         # TYPE dd_dataset_labeling_aggregations_total counter\n\
         dd_dataset_labeling_aggregations_total {}\n\
         # HELP dd_dataset_labeling_exports_total Gold-label exports served.\n\
         # TYPE dd_dataset_labeling_exports_total counter\n\
         dd_dataset_labeling_exports_total {}\n\
         # HELP dd_dataset_labeling_pipeline_jobs_total Pipeline job intents queued.\n\
         # TYPE dd_dataset_labeling_pipeline_jobs_total counter\n\
         dd_dataset_labeling_pipeline_jobs_total {}\n\
         # HELP dd_dataset_labeling_auth_failures_total Rejected requests with missing or invalid auth.\n\
         # TYPE dd_dataset_labeling_auth_failures_total counter\n\
         dd_dataset_labeling_auth_failures_total {}\n\
         # HELP dd_dataset_labeling_errors_total Request or publish errors.\n\
         # TYPE dd_dataset_labeling_errors_total counter\n\
         dd_dataset_labeling_errors_total {}\n\
         # HELP dd_dataset_labeling_nats_messages_total NATS task messages consumed.\n\
         # TYPE dd_dataset_labeling_nats_messages_total counter\n\
         dd_dataset_labeling_nats_messages_total {}\n\
         # HELP dd_dataset_labeling_nats_published_total NATS messages published.\n\
         # TYPE dd_dataset_labeling_nats_published_total counter\n\
         dd_dataset_labeling_nats_published_total {}\n",
        m.http_requests_total.load(Ordering::Relaxed),
        m.datasets_total.load(Ordering::Relaxed),
        m.items_created_total.load(Ordering::Relaxed),
        m.labels_submitted_total.load(Ordering::Relaxed),
        m.function_labels_total.load(Ordering::Relaxed),
        m.aggregations_total.load(Ordering::Relaxed),
        m.exports_total.load(Ordering::Relaxed),
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
        tracing::info!("dataset-labeling nats loop disabled: NATS_URL is not configured");
        return;
    };
    tracing::info!(
        "dataset-labeling nats loop starting: subject={} queueGroup={}",
        state.config.task_request_subject, state.config.queue_group
    );
    loop {
        let mut subscription = match nats
            .queue_subscribe(state.config.task_request_subject.clone(), state.config.queue_group.clone())
            .await
        {
            Ok(subscription) => subscription,
            Err(error) => {
                tracing::error!("dataset-labeling nats subscribe failed: {error}; retrying in 5s");
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
                    "dataset-labeling rejected oversize nats request: bytes={} max={MAX_NATS_PAYLOAD_BYTES}",
                    payload.len()
                );
                continue;
            }
            let task_state = state.clone();
            tokio::spawn(async move {
                if let Err(error) = handle_nats_request(&task_state, &payload).await {
                    task_state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
                    tracing::error!("dataset-labeling nats request failed: {error}");
                }
            });
        }
        tracing::error!("dataset-labeling nats subscription ended; re-subscribing in 5s");
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

// Route a NATS request payload to the matching processor. The discriminator is the
// most specific field present so a single subject can carry every request shape.
async fn handle_nats_request(state: &AppState, payload: &[u8]) -> Result<(), String> {
    let value: Value = serde_json::from_slice(payload).map_err(|e| e.to_string())?;
    if value.get("functions").is_some() {
        let request: FunctionsRequest = serde_json::from_value(value).map_err(|e| e.to_string())?;
        let mut store = state.store.write().unwrap_or_else(|lock| lock.into_inner());
        process_functions(&mut store, request).map(|_| ())
    } else if value.get("labels").is_some() {
        let request: LabelsRequest = serde_json::from_value(value).map_err(|e| e.to_string())?;
        let mut store = state.store.write().unwrap_or_else(|lock| lock.into_inner());
        process_labels(&mut store, request).map(|_| ())
    } else if value.get("items").is_some() {
        let request: TasksRequest = serde_json::from_value(value).map_err(|e| e.to_string())?;
        let mut store = state.store.write().unwrap_or_else(|lock| lock.into_inner());
        process_tasks(&mut store, request).map(|_| ())
    } else if value.get("classes").is_some() || value.get("name").is_some() {
        let request: DatasetRequest = serde_json::from_value(value).map_err(|e| e.to_string())?;
        let mut store = state.store.write().unwrap_or_else(|lock| lock.into_inner());
        upsert_dataset(&mut store, request).map(|_| ())
    } else {
        let request: AggregateRequest = serde_json::from_value(value).map_err(|e| e.to_string())?;
        let mut store = state.store.write().unwrap_or_else(|lock| lock.into_inner());
        process_aggregate(&mut store, &request).map(|_| ())
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    let _otel = dd_telemetry::init("dd-dataset-labeling");

    let host = env_value("HOST", "0.0.0.0");
    let port = env_value("PORT", "8138").parse::<u16>()?;
    let config = config_from_env();
    // Fail closed at startup: a deploy that forgot the operator secret should
    // crash loudly here instead of silently 401-ing every request, and an
    // unauthenticated deployment must be an explicit, visible opt-in.
    if config.server_auth_secret.is_none() && !config.allow_unauthenticated {
        tracing::error!(
            "{SERVICE_NAME} refusing to start: set SERVER_AUTH_SECRET, or explicitly opt into \
             unauthenticated mode with DATASET_LABELING_ALLOW_UNAUTHENTICATED=true"
        );
        return Err("operator auth is not configured".into());
    }
    if config.allow_unauthenticated {
        tracing::error!(
            "{SERVICE_NAME} WARNING: DATASET_LABELING_ALLOW_UNAUTHENTICATED=true; operator endpoints are UNAUTHENTICATED"
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
        store: Arc::new(RwLock::new(LabelStore::default())),
    };
    tokio::spawn(run_nats_loop(state.clone()));

    let app = Router::new()
        .route("/", get(root))
        .route("/descriptor", get(descriptor))
        .route("/schema", get(schema))
        .route("/example", get(example))
        .route("/datasets", get(list_datasets_http).post(create_dataset_http))
        .route("/datasets/export", get(export_http))
        .route("/tasks", post(tasks_http))
        .route("/labels", post(labels_http))
        .route("/functions/apply", post(functions_apply_http))
        .route("/aggregate", post(aggregate_http))
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

    fn seeded_store() -> LabelStore {
        let mut store = LabelStore::default();
        upsert_dataset(
            &mut store,
            DatasetRequest {
                dataset_id: Some("intent".to_string()),
                name: Some("Intent".to_string()),
                task_type: Some("classification".to_string()),
                classes: Some(vec!["billing".to_string(), "bug".to_string(), "other".to_string()]),
                description: None,
            },
        )
        .unwrap();
        process_tasks(
            &mut store,
            TasksRequest {
                request_id: None,
                dataset_id: "intent".to_string(),
                items: vec![
                    IncomingItem { item_id: Some("a".to_string()), text: Some("I was charged twice on my invoice".to_string()), payload: None, tags: None },
                    IncomingItem { item_id: Some("b".to_string()), text: Some("the export button crashes".to_string()), payload: None, tags: None },
                ],
                pipeline: None,
            },
        )
        .unwrap();
        store
    }

    #[test]
    fn constant_time_eq_matches_only_identical_strings() {
        assert!(constant_time_eq("super-secret", "super-secret"));
        assert!(!constant_time_eq("super-secret", "super-secres"));
        assert!(!constant_time_eq("super-secret", "super-secret-longer"));
        assert!(!constant_time_eq("", "x"));
    }

    #[test]
    fn oversized_payload_is_rejected_on_task_creation() {
        let mut store = seeded_store();
        let big = json!({ "blob": "x".repeat(MAX_JSON_VALUE_BYTES + 10) });
        let result = process_tasks(
            &mut store,
            TasksRequest {
                request_id: None,
                dataset_id: "intent".to_string(),
                items: vec![IncomingItem { item_id: Some("big".to_string()), text: None, payload: Some(big), tags: None }],
                pipeline: None,
            },
        );
        assert!(result.is_err());
    }

    #[test]
    fn label_validation_enforces_schema_when_present() {
        let classes = vec!["billing".to_string(), "bug".to_string()];
        assert_eq!(validate_label(&classes, "billing").unwrap(), "billing");
        assert!(validate_label(&classes, "nonsense").is_err());
        // Open schema accepts anything.
        assert_eq!(validate_label(&[], "free-form").unwrap(), "free-form");
    }

    #[test]
    fn labeling_functions_apply_weak_labels_by_keyword() {
        let mut store = seeded_store();
        let (_, applied, touched, _truncated) = process_functions(
            &mut store,
            FunctionsRequest {
                request_id: None,
                dataset_id: "intent".to_string(),
                functions: vec![
                    LabelingFunction { name: "billing-kw".to_string(), label: "billing".to_string(), keywords: Some(vec!["invoice".to_string(), "charged".to_string()]), case_sensitive: None, confidence: None },
                    LabelingFunction { name: "bug-kw".to_string(), label: "bug".to_string(), keywords: Some(vec!["crash".to_string()]), case_sensitive: None, confidence: None },
                ],
                only_unlabeled: None,
            },
        )
        .unwrap();
        assert_eq!(applied, 2);
        assert_eq!(touched, 2);
        let item_a = &store.items["intent"]["a"];
        assert_eq!(item_a.annotations[0].label, "billing");
        assert_eq!(item_a.annotations[0].annotator_type, "function");
    }

    #[test]
    fn aggregation_majority_vote_and_agreement() {
        let mut store = seeded_store();
        process_labels(
            &mut store,
            LabelsRequest {
                request_id: None,
                dataset_id: "intent".to_string(),
                labels: vec![
                    IncomingLabel { item_id: "a".to_string(), label: "billing".to_string(), annotator: Some("ann1".to_string()), annotator_type: Some("human".to_string()), confidence: None },
                    IncomingLabel { item_id: "a".to_string(), label: "billing".to_string(), annotator: Some("ann2".to_string()), annotator_type: Some("human".to_string()), confidence: None },
                    IncomingLabel { item_id: "a".to_string(), label: "bug".to_string(), annotator: Some("ann3".to_string()), annotator_type: Some("human".to_string()), confidence: None },
                ],
            },
        )
        .unwrap();
        let summary = process_aggregate(
            &mut store,
            &AggregateRequest { request_id: None, dataset_id: "intent".to_string(), annotator_types: None, min_votes: Some(1), pipeline: None },
        )
        .unwrap();
        assert_eq!(summary["resolved"], json!(1));
        let gold = store.items["intent"]["a"].gold.as_ref().unwrap();
        assert_eq!(gold.label, "billing");
        assert_eq!(gold.votes, 2);
        assert_eq!(gold.total, 3);
        assert!((gold.agreement - 2.0 / 3.0).abs() < 1e-9);
    }

    #[test]
    fn re_labeling_same_annotator_is_idempotent() {
        let mut store = seeded_store();
        for label in ["billing", "bug", "other"] {
            process_labels(
                &mut store,
                LabelsRequest {
                    request_id: None,
                    dataset_id: "intent".to_string(),
                    labels: vec![IncomingLabel {
                        item_id: "a".to_string(),
                        label: label.to_string(),
                        annotator: Some("ann1".to_string()),
                        annotator_type: Some("human".to_string()),
                        confidence: None,
                    }],
                },
            )
            .unwrap();
        }
        let item = &store.items["intent"]["a"];
        assert_eq!(item.annotations.len(), 1);
        assert_eq!(item.annotations[0].label, "other");
    }

    #[test]
    fn tasks_require_registered_dataset() {
        let mut store = LabelStore::default();
        let result = process_tasks(
            &mut store,
            TasksRequest {
                request_id: None,
                dataset_id: "missing".to_string(),
                items: vec![IncomingItem { item_id: None, text: Some("x".to_string()), payload: None, tags: None }],
                pipeline: None,
            },
        );
        assert!(result.is_err());
    }
}
