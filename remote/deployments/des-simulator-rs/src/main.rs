use std::{
    cmp::Ordering as CmpOrdering,
    collections::{BinaryHeap, HashMap, HashSet},
    env,
    error::Error,
    net::SocketAddr,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use axum::{
    extract::{DefaultBodyLimit, Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

const MODEL_SCHEMA_VERSION: &str = "des.v1";
const MAX_HTTP_BODY_BYTES: usize = 512 * 1024;
const MAX_NATS_PAYLOAD_BYTES: usize = 512 * 1024;
const MAX_RETAINED_JOBS: usize = 512;
const MAX_ACTIVE_JOBS: u64 = 8;
const MAX_TOKEN_LEN: usize = 96;
const MAX_REQUEST_ID_LEN: usize = 128;
const MAX_EVENT_TYPES: usize = 128;
const MAX_RESOURCES: usize = 64;
const MAX_INITIAL_EVENTS: usize = 2_048;
const MAX_TRANSITIONS: usize = 1_024;
const MAX_METRICS: usize = 128;
const DEFAULT_MAX_EVENTS: usize = 50_000;
const MAX_EVENTS: usize = 500_000;
const MAX_TRACE_ENTRIES: usize = 5_000;
const MAX_SIMULATION_TIME: f64 = 365.0 * 24.0 * 60.0 * 60.0;

#[derive(Clone)]
struct AppState {
    nats: Option<async_nats::Client>,
    result_subject: String,
    event_subject: String,
    jobs: Arc<Mutex<HashMap<String, SimulationJobSnapshot>>>,
    metrics: Arc<Metrics>,
    job_sequence: Arc<AtomicU64>,
}

#[derive(Default)]
struct Metrics {
    requests_total: AtomicU64,
    validation_requests_total: AtomicU64,
    jobs_started_total: AtomicU64,
    jobs_completed_total: AtomicU64,
    jobs_failed_total: AtomicU64,
    jobs_running: AtomicU64,
    errors_total: AtomicU64,
    validation_errors_total: AtomicU64,
    nats_messages_total: AtomicU64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct SimulationRequest {
    request_id: Option<String>,
    model: SimulationModel,
    options: Option<SimulationOptions>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct SimulationModel {
    schema_version: String,
    name: Option<String>,
    time_unit: Option<String>,
    start_time: Option<f64>,
    seed: Option<u64>,
    event_types: Vec<EventTypeDefinition>,
    resources: Option<Vec<ResourceDefinition>>,
    initial_events: Vec<ScheduledEventInput>,
    transitions: Vec<TransitionRule>,
    metrics: Option<Vec<MetricDefinition>>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct SimulationOptions {
    until: Option<f64>,
    max_events: Option<usize>,
    trace: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct EventTypeDefinition {
    name: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct ResourceDefinition {
    name: String,
    capacity: u32,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct ScheduledEventInput {
    at: f64,
    event_type: String,
    entity_id: Option<String>,
    attributes: Option<Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct TransitionRule {
    name: Option<String>,
    from: String,
    to: Option<String>,
    delay: DelaySpec,
    probability: Option<f64>,
    resource: Option<ResourceUsage>,
    limit: Option<usize>,
    attributes: Option<Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "distribution", rename_all = "camelCase")]
enum DelaySpec {
    Fixed { value: f64 },
    Uniform { min: f64, max: f64 },
    Exponential { mean: f64 },
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct ResourceUsage {
    name: String,
    units: u32,
    duration: DelaySpec,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct MetricDefinition {
    name: String,
    event_type: String,
    kind: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ValidationResponse {
    ok: bool,
    schema_version: &'static str,
    errors: Vec<String>,
    warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SimulationAcceptedResponse {
    ok: bool,
    job_id: String,
    request_id: String,
    status: String,
    status_url: String,
    result_subject: String,
    submitted_at_ms: u128,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SimulationJobSnapshot {
    ok: bool,
    job_id: String,
    request_id: String,
    status: String,
    submitted_at_ms: u128,
    started_at_ms: Option<u128>,
    finished_at_ms: Option<u128>,
    source: String,
    result: Option<SimulationResult>,
    error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SimulationResult {
    ok: bool,
    job_id: String,
    request_id: String,
    model_name: Option<String>,
    schema_version: String,
    start_time: f64,
    until: f64,
    simulated_until: f64,
    processed_events: usize,
    generated_events: usize,
    truncated: bool,
    event_counts: Vec<EventCount>,
    metric_values: Vec<MetricValue>,
    resources: Vec<ResourceSummary>,
    trace: Vec<TraceEntry>,
    warnings: Vec<String>,
    generated_at_ms: u128,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct EventCount {
    event_type: String,
    count: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct MetricValue {
    name: String,
    kind: String,
    event_type: String,
    value: f64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ResourceSummary {
    name: String,
    capacity: u32,
    allocations: u64,
    busy_time: f64,
    utilization: f64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct TraceEntry {
    kind: String,
    at: f64,
    event_type: String,
    entity_id: Option<String>,
    transition: Option<String>,
    resource: Option<String>,
}

#[derive(Debug, Clone)]
struct QueuedEvent {
    at: f64,
    sequence: u64,
    event_type: String,
    entity_id: Option<String>,
    attributes: Option<Value>,
}

impl PartialEq for QueuedEvent {
    fn eq(&self, other: &Self) -> bool {
        self.sequence == other.sequence && self.at == other.at
    }
}

impl Eq for QueuedEvent {}

impl PartialOrd for QueuedEvent {
    fn partial_cmp(&self, other: &Self) -> Option<CmpOrdering> {
        Some(self.cmp(other))
    }
}

impl Ord for QueuedEvent {
    fn cmp(&self, other: &Self) -> CmpOrdering {
        other
            .at
            .total_cmp(&self.at)
            .then_with(|| other.sequence.cmp(&self.sequence))
    }
}

#[derive(Debug, Clone)]
struct ResourceState {
    capacity: u32,
    available_at: Vec<f64>,
    busy_time: f64,
    allocations: u64,
}

impl ResourceState {
    fn new(capacity: u32) -> Self {
        Self {
            capacity,
            available_at: vec![0.0; capacity as usize],
            busy_time: 0.0,
            allocations: 0,
        }
    }

    fn plan(&self, earliest_start: f64, units: u32, duration: f64) -> ResourceReservation {
        let mut indexed = self
            .available_at
            .iter()
            .copied()
            .enumerate()
            .collect::<Vec<_>>();
        indexed.sort_by(|left, right| {
            left.1
                .total_cmp(&right.1)
                .then_with(|| left.0.cmp(&right.0))
        });

        let selected = indexed
            .iter()
            .take(units as usize)
            .map(|(index, _)| *index)
            .collect::<Vec<_>>();
        let start = indexed
            .iter()
            .take(units as usize)
            .map(|(_, available_at)| *available_at)
            .fold(earliest_start, f64::max);
        ResourceReservation {
            selected,
            finish: start + duration,
        }
    }

    fn commit(&mut self, reservation: &ResourceReservation, duration: f64, units: u32) {
        for index in &reservation.selected {
            self.available_at[*index] = reservation.finish;
        }
        self.busy_time += duration * f64::from(units);
        self.allocations += 1;
    }
}

struct ResourceReservation {
    selected: Vec<usize>,
    finish: f64,
}

#[derive(Debug)]
struct SimulationConfig {
    start_time: f64,
    until: f64,
    max_events: usize,
    trace: bool,
}

#[derive(Debug)]
enum StartJobError {
    Invalid(String),
    Busy(String),
}

impl StartJobError {
    fn message(&self) -> &str {
        match self {
            StartJobError::Invalid(message) | StartJobError::Busy(message) => message,
        }
    }

    fn status_code(&self) -> StatusCode {
        match self {
            StartJobError::Invalid(_) => StatusCode::BAD_REQUEST,
            StartJobError::Busy(_) => StatusCode::TOO_MANY_REQUESTS,
        }
    }
}

#[derive(Clone)]
struct LcgRng {
    state: u64,
}

impl LcgRng {
    fn new(seed: u64) -> Self {
        let state = if seed == 0 {
            0x9e37_79b9_7f4a_7c15
        } else {
            seed
        };
        Self { state }
    }

    fn next_u64(&mut self) -> u64 {
        self.state = self
            .state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.state
    }

    fn next_f64(&mut self) -> f64 {
        let value = self.next_u64() >> 11;
        (value as f64) / ((1u64 << 53) as f64)
    }
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn env_value(key: &str, fallback: &str) -> String {
    env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| fallback.to_string())
}

fn finite_non_negative(value: f64, label: &str) -> Result<f64, String> {
    if !value.is_finite() || value < 0.0 {
        return Err(format!("{label} must be finite and non-negative"));
    }
    Ok(value)
}

fn finite_positive(value: f64, label: &str) -> Result<f64, String> {
    if !value.is_finite() || value <= 0.0 {
        return Err(format!("{label} must be finite and positive"));
    }
    Ok(value)
}

fn finite_probability(value: f64, label: &str) -> Result<f64, String> {
    if !value.is_finite() || !(0.0..=1.0).contains(&value) {
        return Err(format!("{label} must be finite and in [0, 1]"));
    }
    Ok(value)
}

fn validate_token(value: &str, label: &str) -> Result<String, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(format!("{label} must not be empty"));
    }
    if trimmed.len() > MAX_TOKEN_LEN {
        return Err(format!("{label} must be at most {MAX_TOKEN_LEN} bytes"));
    }
    if trimmed.chars().any(|character| character.is_control()) {
        return Err(format!("{label} must not contain control characters"));
    }
    Ok(trimmed.to_string())
}

fn validate_request_id(value: Option<&String>) -> Result<Option<String>, String> {
    value
        .map(|request_id| {
            let trimmed = request_id.trim();
            if trimmed.is_empty() {
                return Err("requestId must not be empty when provided".to_string());
            }
            if trimmed.len() > MAX_REQUEST_ID_LEN {
                return Err(format!(
                    "requestId must be at most {MAX_REQUEST_ID_LEN} bytes"
                ));
            }
            if trimmed.chars().any(|character| character.is_control()) {
                return Err("requestId must not contain control characters".to_string());
            }
            Ok(trimmed.to_string())
        })
        .transpose()
}

fn validate_unique(values: &[String], label: &str) -> Result<(), String> {
    let mut seen = HashSet::with_capacity(values.len());
    for value in values {
        if !seen.insert(value.as_str()) {
            return Err(format!("{label} must be unique; duplicate {value}"));
        }
    }
    Ok(())
}

fn validate_delay(delay: &DelaySpec, label: &str) -> Result<(), String> {
    match delay {
        DelaySpec::Fixed { value } => {
            finite_non_negative(*value, &format!("{label}.value"))?;
        }
        DelaySpec::Uniform { min, max } => {
            finite_non_negative(*min, &format!("{label}.min"))?;
            finite_non_negative(*max, &format!("{label}.max"))?;
            if min > max {
                return Err(format!("{label}.min must be <= {label}.max"));
            }
        }
        DelaySpec::Exponential { mean } => {
            finite_positive(*mean, &format!("{label}.mean"))?;
        }
    }
    Ok(())
}

fn sample_delay(delay: &DelaySpec, rng: &mut LcgRng) -> f64 {
    match delay {
        DelaySpec::Fixed { value } => *value,
        DelaySpec::Uniform { min, max } => min + (max - min) * rng.next_f64(),
        DelaySpec::Exponential { mean } => {
            let draw = (1.0 - rng.next_f64()).max(f64::MIN_POSITIVE);
            -mean * draw.ln()
        }
    }
}

fn validate_simulation_request(request: &SimulationRequest) -> ValidationResponse {
    let mut errors = Vec::new();
    let mut warnings = Vec::new();

    if let Err(error) = validate_request_id(request.request_id.as_ref()) {
        errors.push(error);
    }

    let model = &request.model;
    if model.schema_version != MODEL_SCHEMA_VERSION {
        errors.push(format!(
            "model.schemaVersion must be {MODEL_SCHEMA_VERSION}"
        ));
    }
    if let Some(name) = &model.name {
        if let Err(error) = validate_token(name, "model.name") {
            errors.push(error);
        }
    }
    if let Some(time_unit) = &model.time_unit {
        if let Err(error) = validate_token(time_unit, "model.timeUnit") {
            errors.push(error);
        }
    }
    let start_time = match model.start_time {
        Some(value) => match finite_non_negative(value, "model.startTime") {
            Ok(value) => value,
            Err(error) => {
                errors.push(error);
                0.0
            }
        },
        None => 0.0,
    };
    if model.event_types.is_empty() {
        errors.push("model.eventTypes must not be empty".to_string());
    }
    if model.event_types.len() > MAX_EVENT_TYPES {
        errors.push(format!(
            "model.eventTypes must contain at most {MAX_EVENT_TYPES} entries"
        ));
    }

    let event_names = model
        .event_types
        .iter()
        .map(
            |event| match validate_token(&event.name, "eventTypes[].name") {
                Ok(name) => name,
                Err(error) => {
                    errors.push(error);
                    event.name.clone()
                }
            },
        )
        .collect::<Vec<_>>();
    if let Err(error) = validate_unique(&event_names, "model.eventTypes.name") {
        errors.push(error);
    }
    let event_set = event_names.iter().cloned().collect::<HashSet<_>>();

    let resources = model.resources.as_deref().unwrap_or(&[]);
    if resources.len() > MAX_RESOURCES {
        errors.push(format!(
            "model.resources must contain at most {MAX_RESOURCES} entries"
        ));
    }
    let mut resource_capacity = HashMap::new();
    let resource_names = resources
        .iter()
        .map(|resource| {
            let name = match validate_token(&resource.name, "resources[].name") {
                Ok(name) => name,
                Err(error) => {
                    errors.push(error);
                    resource.name.clone()
                }
            };
            if resource.capacity == 0 {
                errors.push(format!("resource {name} capacity must be positive"));
            }
            resource_capacity.insert(name.clone(), resource.capacity);
            name
        })
        .collect::<Vec<_>>();
    if let Err(error) = validate_unique(&resource_names, "model.resources.name") {
        errors.push(error);
    }

    if model.initial_events.is_empty() {
        errors.push("model.initialEvents must not be empty".to_string());
    }
    if model.initial_events.len() > MAX_INITIAL_EVENTS {
        errors.push(format!(
            "model.initialEvents must contain at most {MAX_INITIAL_EVENTS} entries"
        ));
    }
    for (index, event) in model.initial_events.iter().enumerate() {
        match finite_non_negative(event.at, &format!("initialEvents[{index}].at")) {
            Ok(at) if at < start_time => errors.push(format!(
                "initialEvents[{index}].at must be >= model.startTime"
            )),
            Ok(_) => {}
            Err(error) => errors.push(error),
        }
        if !event_set.contains(&event.event_type) {
            errors.push(format!(
                "initialEvents[{index}].eventType references unknown event type {}",
                event.event_type
            ));
        }
        if let Some(entity_id) = &event.entity_id {
            if let Err(error) =
                validate_token(entity_id, &format!("initialEvents[{index}].entityId"))
            {
                errors.push(error);
            }
        }
    }

    if model.transitions.is_empty() {
        warnings.push(
            "model.transitions is empty; simulation will only process initial events".to_string(),
        );
    }
    if model.transitions.len() > MAX_TRANSITIONS {
        errors.push(format!(
            "model.transitions must contain at most {MAX_TRANSITIONS} entries"
        ));
    }
    for (index, transition) in model.transitions.iter().enumerate() {
        if let Some(name) = &transition.name {
            if let Err(error) = validate_token(name, &format!("transitions[{index}].name")) {
                errors.push(error);
            }
        }
        if !event_set.contains(&transition.from) {
            errors.push(format!(
                "transitions[{index}].from references unknown event type {}",
                transition.from
            ));
        }
        if let Some(to) = &transition.to {
            if !event_set.contains(to) {
                errors.push(format!(
                    "transitions[{index}].to references unknown event type {to}"
                ));
            }
        }
        if let Err(error) =
            validate_delay(&transition.delay, &format!("transitions[{index}].delay"))
        {
            errors.push(error);
        }
        if let Some(probability) = transition.probability {
            if let Err(error) =
                finite_probability(probability, &format!("transitions[{index}].probability"))
            {
                errors.push(error);
            }
        }
        if let Some(limit) = transition.limit {
            if limit == 0 {
                errors.push(format!("transitions[{index}].limit must be positive"));
            }
            if limit > MAX_EVENTS {
                errors.push(format!(
                    "transitions[{index}].limit must be at most {MAX_EVENTS}"
                ));
            }
        }
        if let Some(resource) = &transition.resource {
            if let Err(error) = validate_token(
                &resource.name,
                &format!("transitions[{index}].resource.name"),
            ) {
                errors.push(error);
            }
            match resource_capacity.get(&resource.name) {
                Some(_) if resource.units == 0 => errors.push(format!(
                    "transitions[{index}].resource.units must be positive"
                )),
                Some(capacity) if resource.units > *capacity => errors.push(format!(
                    "transitions[{index}].resource.units must be <= resource {} capacity {}",
                    resource.name, capacity
                )),
                Some(_) => {}
                None => errors.push(format!(
                    "transitions[{index}].resource.name references unknown resource {}",
                    resource.name
                )),
            }
            if let Err(error) = validate_delay(
                &resource.duration,
                &format!("transitions[{index}].resource.duration"),
            ) {
                errors.push(error);
            }
        }
    }

    let metrics = model.metrics.as_deref().unwrap_or(&[]);
    if metrics.len() > MAX_METRICS {
        errors.push(format!(
            "model.metrics must contain at most {MAX_METRICS} entries"
        ));
    }
    let metric_names = metrics
        .iter()
        .map(
            |metric| match validate_token(&metric.name, "metrics[].name") {
                Ok(name) => name,
                Err(error) => {
                    errors.push(error);
                    metric.name.clone()
                }
            },
        )
        .collect::<Vec<_>>();
    if let Err(error) = validate_unique(&metric_names, "model.metrics.name") {
        errors.push(error);
    }
    for (index, metric) in metrics.iter().enumerate() {
        if !event_set.contains(&metric.event_type) {
            errors.push(format!(
                "metrics[{index}].eventType references unknown event type {}",
                metric.event_type
            ));
        }
        let kind = metric.kind.as_deref().unwrap_or("count");
        if kind != "count" {
            errors.push(format!("metrics[{index}].kind must be count; got {kind}"));
        }
    }

    let options = request.options.as_ref();
    if let Some(until) = options.and_then(|options| options.until) {
        match finite_non_negative(until, "options.until") {
            Ok(until) => {
                if until < start_time {
                    errors.push("options.until must be >= model.startTime".to_string());
                }
                if until > MAX_SIMULATION_TIME {
                    errors.push(format!(
                        "options.until must be at most {MAX_SIMULATION_TIME}"
                    ));
                }
                for (index, event) in model.initial_events.iter().enumerate() {
                    if event.at > until {
                        errors.push(format!(
                            "initialEvents[{index}].at must be <= options.until"
                        ));
                    }
                }
            }
            Err(error) => errors.push(error),
        }
    }
    if let Some(max_events) = options.and_then(|options| options.max_events) {
        if max_events == 0 {
            errors.push("options.maxEvents must be positive".to_string());
        }
        if max_events > MAX_EVENTS {
            errors.push(format!("options.maxEvents must be at most {MAX_EVENTS}"));
        }
    }

    ValidationResponse {
        ok: errors.is_empty(),
        schema_version: MODEL_SCHEMA_VERSION,
        errors,
        warnings,
    }
}

fn simulation_config(request: &SimulationRequest) -> Result<SimulationConfig, String> {
    let validation = validate_simulation_request(request);
    if !validation.ok {
        return Err(validation.errors.join("; "));
    }
    let options = request.options.as_ref();
    let start_time = request.model.start_time.unwrap_or(0.0);
    Ok(SimulationConfig {
        start_time,
        until: options
            .and_then(|options| options.until)
            .unwrap_or(MAX_SIMULATION_TIME),
        max_events: options
            .and_then(|options| options.max_events)
            .unwrap_or(DEFAULT_MAX_EVENTS),
        trace: options.and_then(|options| options.trace).unwrap_or(false),
    })
}

fn model_schema() -> Value {
    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "$id": "https://dd.local/schemas/des-simulation-request.v1.json",
        "title": "dd DES Simulation Request",
        "type": "object",
        "required": ["model"],
        "additionalProperties": false,
        "properties": {
            "requestId": {
                "type": "string",
                "minLength": 1,
                "maxLength": MAX_REQUEST_ID_LEN
            },
            "model": {
                "type": "object",
                "required": ["schemaVersion", "eventTypes", "initialEvents", "transitions"],
                "additionalProperties": false,
                "properties": {
                    "schemaVersion": { "const": MODEL_SCHEMA_VERSION },
                    "name": { "type": "string", "minLength": 1, "maxLength": MAX_TOKEN_LEN },
                    "timeUnit": { "type": "string", "minLength": 1, "maxLength": MAX_TOKEN_LEN },
                    "startTime": { "type": "number", "minimum": 0 },
                    "seed": { "type": "integer", "minimum": 0 },
                    "eventTypes": {
                        "type": "array",
                        "minItems": 1,
                        "maxItems": MAX_EVENT_TYPES,
                        "items": {
                            "type": "object",
                            "required": ["name"],
                            "additionalProperties": false,
                            "properties": {
                                "name": { "type": "string", "minLength": 1, "maxLength": MAX_TOKEN_LEN }
                            }
                        }
                    },
                    "resources": {
                        "type": "array",
                        "maxItems": MAX_RESOURCES,
                        "items": {
                            "type": "object",
                            "required": ["name", "capacity"],
                            "additionalProperties": false,
                            "properties": {
                                "name": { "type": "string", "minLength": 1, "maxLength": MAX_TOKEN_LEN },
                                "capacity": { "type": "integer", "minimum": 1 }
                            }
                        }
                    },
                    "initialEvents": {
                        "type": "array",
                        "minItems": 1,
                        "maxItems": MAX_INITIAL_EVENTS,
                        "items": { "$ref": "#/$defs/scheduledEvent" }
                    },
                    "transitions": {
                        "type": "array",
                        "maxItems": MAX_TRANSITIONS,
                        "items": { "$ref": "#/$defs/transition" }
                    },
                    "metrics": {
                        "type": "array",
                        "maxItems": MAX_METRICS,
                        "items": {
                            "type": "object",
                            "required": ["name", "eventType"],
                            "additionalProperties": false,
                            "properties": {
                                "name": { "type": "string", "minLength": 1, "maxLength": MAX_TOKEN_LEN },
                                "eventType": { "type": "string", "minLength": 1, "maxLength": MAX_TOKEN_LEN },
                                "kind": { "enum": ["count"] }
                            }
                        }
                    }
                }
            },
            "options": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "until": { "type": "number", "minimum": 0, "maximum": MAX_SIMULATION_TIME },
                    "maxEvents": { "type": "integer", "minimum": 1, "maximum": MAX_EVENTS },
                    "trace": { "type": "boolean" }
                }
            }
        },
        "$defs": {
            "scheduledEvent": {
                "type": "object",
                "required": ["at", "eventType"],
                "additionalProperties": false,
                "properties": {
                    "at": { "type": "number", "minimum": 0 },
                    "eventType": { "type": "string", "minLength": 1, "maxLength": MAX_TOKEN_LEN },
                    "entityId": { "type": "string", "minLength": 1, "maxLength": MAX_TOKEN_LEN },
                    "attributes": {}
                }
            },
            "transition": {
                "type": "object",
                "required": ["from", "delay"],
                "additionalProperties": false,
                "properties": {
                    "name": { "type": "string", "minLength": 1, "maxLength": MAX_TOKEN_LEN },
                    "from": { "type": "string", "minLength": 1, "maxLength": MAX_TOKEN_LEN },
                    "to": { "type": "string", "minLength": 1, "maxLength": MAX_TOKEN_LEN },
                    "delay": { "$ref": "#/$defs/delay" },
                    "probability": { "type": "number", "minimum": 0, "maximum": 1 },
                    "resource": {
                        "type": "object",
                        "required": ["name", "units", "duration"],
                        "additionalProperties": false,
                        "properties": {
                            "name": { "type": "string", "minLength": 1, "maxLength": MAX_TOKEN_LEN },
                            "units": { "type": "integer", "minimum": 1 },
                            "duration": { "$ref": "#/$defs/delay" }
                        }
                    },
                    "limit": { "type": "integer", "minimum": 1, "maximum": MAX_EVENTS },
                    "attributes": {}
                }
            },
            "delay": {
                "oneOf": [
                    {
                        "type": "object",
                        "required": ["distribution", "value"],
                        "additionalProperties": false,
                        "properties": {
                            "distribution": { "const": "fixed" },
                            "value": { "type": "number", "minimum": 0 }
                        }
                    },
                    {
                        "type": "object",
                        "required": ["distribution", "min", "max"],
                        "additionalProperties": false,
                        "properties": {
                            "distribution": { "const": "uniform" },
                            "min": { "type": "number", "minimum": 0 },
                            "max": { "type": "number", "minimum": 0 }
                        }
                    },
                    {
                        "type": "object",
                        "required": ["distribution", "mean"],
                        "additionalProperties": false,
                        "properties": {
                            "distribution": { "const": "exponential" },
                            "mean": { "type": "number", "exclusiveMinimum": 0 }
                        }
                    }
                ]
            }
        }
    })
}

fn example_request() -> Value {
    json!({
        "requestId": "clinic-demo",
        "model": {
            "schemaVersion": MODEL_SCHEMA_VERSION,
            "name": "clinic-intake",
            "timeUnit": "minutes",
            "startTime": 0,
            "seed": 42,
            "eventTypes": [
                { "name": "arrival" },
                { "name": "triageComplete" },
                { "name": "discharged" }
            ],
            "resources": [
                { "name": "triageNurse", "capacity": 1 },
                { "name": "examRoom", "capacity": 2 }
            ],
            "initialEvents": [
                { "at": 0, "eventType": "arrival", "entityId": "patient-1" },
                { "at": 3, "eventType": "arrival", "entityId": "patient-2" }
            ],
            "transitions": [
                {
                    "name": "triage",
                    "from": "arrival",
                    "to": "triageComplete",
                    "delay": { "distribution": "fixed", "value": 0 },
                    "resource": {
                        "name": "triageNurse",
                        "units": 1,
                        "duration": { "distribution": "uniform", "min": 4, "max": 6 }
                    }
                },
                {
                    "name": "exam",
                    "from": "triageComplete",
                    "to": "discharged",
                    "delay": { "distribution": "fixed", "value": 1 },
                    "probability": 0.95,
                    "resource": {
                        "name": "examRoom",
                        "units": 1,
                        "duration": { "distribution": "exponential", "mean": 12 }
                    }
                }
            ],
            "metrics": [
                { "name": "arrivals", "eventType": "arrival", "kind": "count" },
                { "name": "discharges", "eventType": "discharged", "kind": "count" }
            ]
        },
        "options": {
            "until": 120,
            "maxEvents": 10000,
            "trace": true
        }
    })
}

fn fibonacci_example_request() -> Value {
    let event_types = (0..=8)
        .map(|index| json!({ "name": format!("fib{index}") }))
        .collect::<Vec<_>>();
    let mut transitions = Vec::new();
    for index in 0..=6 {
        transitions.push(json!({
            "name": format!("fib{index}.advance-one"),
            "from": format!("fib{index}"),
            "to": format!("fib{}", index + 1),
            "delay": { "distribution": "fixed", "value": 1 },
            "attributes": {
                "control": "advance-one",
                "sourceIndex": index
            }
        }));
        transitions.push(json!({
            "name": format!("fib{index}.advance-two"),
            "from": format!("fib{index}"),
            "to": format!("fib{}", index + 2),
            "delay": { "distribution": "fixed", "value": 2 },
            "attributes": {
                "control": "advance-two",
                "sourceIndex": index
            }
        }));
    }
    transitions.push(json!({
        "name": "fib7.advance-one",
        "from": "fib7",
        "to": "fib8",
        "delay": { "distribution": "fixed", "value": 1 },
        "attributes": {
            "control": "advance-one",
            "sourceIndex": 7
        }
    }));
    let metrics = (0..=8)
        .map(|index| {
            json!({
                "name": format!("fib{index}_count"),
                "eventType": format!("fib{index}"),
                "kind": "count"
            })
        })
        .collect::<Vec<_>>();

    json!({
        "requestId": "fibonacci-control-demo",
        "model": {
            "schemaVersion": MODEL_SCHEMA_VERSION,
            "name": "fibonacci-discrete-control",
            "timeUnit": "step",
            "startTime": 0,
            "seed": 13,
            "eventTypes": event_types,
            "initialEvents": [
                {
                    "at": 0,
                    "eventType": "fib0",
                    "entityId": "sequence-root",
                    "attributes": { "index": 0 }
                }
            ],
            "transitions": transitions,
            "metrics": metrics
        },
        "options": {
            "until": 12,
            "maxEvents": 200,
            "trace": true
        }
    })
}

fn temperature_control_example_request() -> Value {
    json!({
        "requestId": "temperature-control-demo",
        "model": {
            "schemaVersion": MODEL_SCHEMA_VERSION,
            "name": "temperature-bang-bang-control",
            "timeUnit": "seconds",
            "startTime": 0,
            "seed": 21,
            "eventTypes": [
                { "name": "sampleCold" },
                { "name": "commandHeat" },
                { "name": "sampleComfort" },
                { "name": "commandHold" },
                { "name": "sampleHot" },
                { "name": "commandCool" }
            ],
            "resources": [
                { "name": "heater", "capacity": 1 },
                { "name": "cooler", "capacity": 1 }
            ],
            "initialEvents": [
                {
                    "at": 0,
                    "eventType": "sampleCold",
                    "entityId": "zone-a",
                    "attributes": { "temperatureC": 18, "setpointC": 22 }
                }
            ],
            "transitions": [
                {
                    "name": "cold.control-heat",
                    "from": "sampleCold",
                    "to": "commandHeat",
                    "delay": { "distribution": "fixed", "value": 0 },
                    "limit": 2,
                    "attributes": { "controllerState": "heating" }
                },
                {
                    "name": "heat.plant-response",
                    "from": "commandHeat",
                    "to": "sampleComfort",
                    "delay": { "distribution": "fixed", "value": 0 },
                    "resource": {
                        "name": "heater",
                        "units": 1,
                        "duration": { "distribution": "fixed", "value": 4 }
                    },
                    "limit": 2,
                    "attributes": { "temperatureC": 22, "controllerState": "comfort" }
                },
                {
                    "name": "comfort.control-hold",
                    "from": "sampleComfort",
                    "to": "commandHold",
                    "delay": { "distribution": "fixed", "value": 0 },
                    "limit": 3,
                    "attributes": { "controllerState": "holding" }
                },
                {
                    "name": "hold.disturb-hot",
                    "from": "commandHold",
                    "to": "sampleHot",
                    "delay": { "distribution": "fixed", "value": 5 },
                    "limit": 2,
                    "attributes": { "temperatureC": 25, "disturbance": "solar-gain" }
                },
                {
                    "name": "hot.control-cool",
                    "from": "sampleHot",
                    "to": "commandCool",
                    "delay": { "distribution": "fixed", "value": 0 },
                    "limit": 2,
                    "attributes": { "controllerState": "cooling" }
                },
                {
                    "name": "cool.plant-response",
                    "from": "commandCool",
                    "to": "sampleComfort",
                    "delay": { "distribution": "fixed", "value": 0 },
                    "resource": {
                        "name": "cooler",
                        "units": 1,
                        "duration": { "distribution": "fixed", "value": 3 }
                    },
                    "limit": 2,
                    "attributes": { "temperatureC": 22, "controllerState": "comfort" }
                },
                {
                    "name": "hold.disturb-cold",
                    "from": "commandHold",
                    "to": "sampleCold",
                    "delay": { "distribution": "fixed", "value": 7 },
                    "limit": 1,
                    "attributes": { "temperatureC": 18, "disturbance": "door-open" }
                }
            ],
            "metrics": [
                { "name": "heat_commands", "eventType": "commandHeat", "kind": "count" },
                { "name": "cool_commands", "eventType": "commandCool", "kind": "count" },
                { "name": "comfort_samples", "eventType": "sampleComfort", "kind": "count" }
            ]
        },
        "options": {
            "until": 30,
            "maxEvents": 100,
            "trace": true
        }
    })
}

fn examples_index() -> Value {
    json!({
        "ok": true,
        "schemaVersion": MODEL_SCHEMA_VERSION,
        "examples": [
            {
                "name": "clinic",
                "description": "Queueing DES with bounded nurse and exam-room resources.",
                "url": "/model/examples/clinic"
            },
            {
                "name": "fibonacci",
                "description": "Deterministic branching DES where discrete advance controls produce Fibonacci event counts.",
                "url": "/model/examples/fibonacci"
            },
            {
                "name": "temperature-control",
                "description": "Bang-bang temperature controller with discrete heat, hold, and cool commands.",
                "url": "/model/examples/temperature-control"
            }
        ]
    })
}

fn example_request_by_name(name: &str) -> Option<Value> {
    match name {
        "clinic" | "clinic-intake" | "default" => Some(example_request()),
        "fibonacci" | "fibonacci-control" => Some(fibonacci_example_request()),
        "temperature" | "temperature-control" | "bang-bang-temperature-control" => {
            Some(temperature_control_example_request())
        }
        _ => None,
    }
}

fn request_identifier(request: &SimulationRequest, job_id: &str) -> String {
    request
        .request_id
        .as_ref()
        .map(|request_id| request_id.trim().to_string())
        .filter(|request_id| !request_id.is_empty())
        .unwrap_or_else(|| job_id.to_string())
}

fn transition_name(index: usize, transition: &TransitionRule) -> String {
    transition
        .name
        .clone()
        .unwrap_or_else(|| format!("transition-{index}"))
}

fn simulate(request: SimulationRequest, job_id: String) -> Result<SimulationResult, String> {
    let config = simulation_config(&request)?;
    let request_id = request_identifier(&request, &job_id);
    let mut warnings = validate_simulation_request(&request).warnings;
    let mut rng = LcgRng::new(request.model.seed.unwrap_or(0x5eed_5eed));
    let mut sequence = 0u64;
    let mut queue = BinaryHeap::new();
    for event in &request.model.initial_events {
        queue.push(QueuedEvent {
            at: event.at,
            sequence,
            event_type: event.event_type.clone(),
            entity_id: event.entity_id.clone(),
            attributes: event.attributes.clone(),
        });
        sequence += 1;
    }

    let mut transitions_by_event: HashMap<String, Vec<usize>> = HashMap::new();
    for (index, transition) in request.model.transitions.iter().enumerate() {
        transitions_by_event
            .entry(transition.from.clone())
            .or_default()
            .push(index);
    }

    let mut resources = request
        .model
        .resources
        .as_deref()
        .unwrap_or(&[])
        .iter()
        .map(|resource| (resource.name.clone(), ResourceState::new(resource.capacity)))
        .collect::<HashMap<_, _>>();
    for resource in resources.values_mut() {
        for available_at in &mut resource.available_at {
            *available_at = config.start_time;
        }
    }

    let mut event_counts = request
        .model
        .event_types
        .iter()
        .map(|event| (event.name.clone(), 0u64))
        .collect::<HashMap<_, _>>();
    let mut transition_fires = vec![0usize; request.model.transitions.len()];
    let mut processed_events = 0usize;
    let mut generated_events = 0usize;
    let mut simulated_until = config.start_time;
    let mut trace = Vec::new();
    let mut truncated = false;

    while let Some(event) = queue.pop() {
        if event.at > config.until {
            break;
        }
        processed_events += 1;
        simulated_until = simulated_until.max(event.at);
        *event_counts.entry(event.event_type.clone()).or_insert(0) += 1;
        if config.trace && trace.len() < MAX_TRACE_ENTRIES {
            trace.push(TraceEntry {
                kind: "processed".to_string(),
                at: event.at,
                event_type: event.event_type.clone(),
                entity_id: event.entity_id.clone(),
                transition: None,
                resource: None,
            });
        }

        if processed_events >= config.max_events {
            truncated = !queue.is_empty();
            if truncated {
                warnings.push(format!(
                    "simulation stopped after maxEvents={} with {} queued events remaining",
                    config.max_events,
                    queue.len()
                ));
            }
            break;
        }

        for transition_index in transitions_by_event
            .get(&event.event_type)
            .cloned()
            .unwrap_or_default()
        {
            let transition = &request.model.transitions[transition_index];
            if transition
                .limit
                .map(|limit| transition_fires[transition_index] >= limit)
                .unwrap_or(false)
            {
                continue;
            }
            let probability = transition.probability.unwrap_or(1.0);
            if probability < 1.0 && rng.next_f64() > probability {
                continue;
            }
            transition_fires[transition_index] += 1;

            let Some(to) = transition.to.as_ref() else {
                continue;
            };
            let base_delay = sample_delay(&transition.delay, &mut rng);
            let earliest_start = event.at + base_delay;
            let mut scheduled_at = earliest_start;
            let mut resource_name = None;
            if let Some(usage) = &transition.resource {
                let duration = sample_delay(&usage.duration, &mut rng);
                let resource = resources
                    .get_mut(&usage.name)
                    .ok_or_else(|| format!("missing resource state {}", usage.name))?;
                let reservation = resource.plan(earliest_start, usage.units, duration);
                scheduled_at = reservation.finish;
                if scheduled_at <= config.until {
                    resource.commit(&reservation, duration, usage.units);
                }
                resource_name = Some(usage.name.clone());
            }

            if scheduled_at > config.until {
                continue;
            }

            queue.push(QueuedEvent {
                at: scheduled_at,
                sequence,
                event_type: to.clone(),
                entity_id: event.entity_id.clone(),
                attributes: transition
                    .attributes
                    .clone()
                    .or_else(|| event.attributes.clone()),
            });
            sequence += 1;
            generated_events += 1;
            simulated_until = simulated_until.max(scheduled_at);
            if config.trace && trace.len() < MAX_TRACE_ENTRIES {
                trace.push(TraceEntry {
                    kind: "scheduled".to_string(),
                    at: scheduled_at,
                    event_type: to.clone(),
                    entity_id: event.entity_id.clone(),
                    transition: Some(transition_name(transition_index, transition)),
                    resource: resource_name,
                });
            }
        }
    }

    if config.trace && trace.len() == MAX_TRACE_ENTRIES {
        warnings.push(format!(
            "trace truncated at {MAX_TRACE_ENTRIES} entries; result counters include all processed events"
        ));
    }

    let mut event_count_entries = event_counts
        .into_iter()
        .map(|(event_type, count)| EventCount { event_type, count })
        .collect::<Vec<_>>();
    event_count_entries.sort_by(|left, right| left.event_type.cmp(&right.event_type));

    let metric_values = match request.model.metrics.as_ref() {
        Some(metrics) => metrics
            .iter()
            .map(|metric| MetricValue {
                name: metric.name.clone(),
                kind: metric.kind.clone().unwrap_or_else(|| "count".to_string()),
                event_type: metric.event_type.clone(),
                value: event_count_entries
                    .iter()
                    .find(|entry| entry.event_type == metric.event_type)
                    .map(|entry| entry.count as f64)
                    .unwrap_or(0.0),
            })
            .collect(),
        None => event_count_entries
            .iter()
            .map(|entry| MetricValue {
                name: format!("{}_count", entry.event_type),
                kind: "count".to_string(),
                event_type: entry.event_type.clone(),
                value: entry.count as f64,
            })
            .collect(),
    };

    let horizon = (simulated_until - config.start_time).max(f64::MIN_POSITIVE);
    let mut resource_entries = resources
        .into_iter()
        .map(|(name, resource)| ResourceSummary {
            name,
            capacity: resource.capacity,
            allocations: resource.allocations,
            busy_time: resource.busy_time,
            utilization: (resource.busy_time / (horizon * f64::from(resource.capacity))).min(1.0),
        })
        .collect::<Vec<_>>();
    resource_entries.sort_by(|left, right| left.name.cmp(&right.name));

    Ok(SimulationResult {
        ok: true,
        job_id,
        request_id,
        model_name: request.model.name.clone(),
        schema_version: MODEL_SCHEMA_VERSION.to_string(),
        start_time: config.start_time,
        until: config.until,
        simulated_until,
        processed_events,
        generated_events,
        truncated,
        event_counts: event_count_entries,
        metric_values,
        resources: resource_entries,
        trace,
        warnings,
        generated_at_ms: now_ms(),
    })
}

fn prune_jobs(jobs: &mut HashMap<String, SimulationJobSnapshot>) {
    if jobs.len() < MAX_RETAINED_JOBS {
        return;
    }
    let mut oldest = jobs
        .iter()
        .map(|(job_id, snapshot)| (job_id.clone(), snapshot.submitted_at_ms))
        .collect::<Vec<_>>();
    oldest.sort_by(|left, right| left.1.cmp(&right.1));
    let remove_count = oldest.len().saturating_sub(MAX_RETAINED_JOBS - 1);
    for (job_id, _) in oldest.into_iter().take(remove_count) {
        jobs.remove(&job_id);
    }
}

fn update_job<F>(state: &AppState, job_id: &str, update: F)
where
    F: FnOnce(&mut SimulationJobSnapshot),
{
    let mut jobs = state.jobs.lock().expect("job store mutex poisoned");
    if let Some(snapshot) = jobs.get_mut(job_id) {
        update(snapshot);
    }
}

async fn publish_job_event(state: &AppState, snapshot: &SimulationJobSnapshot) {
    let Some(nats) = &state.nats else {
        return;
    };
    let payload = json!({
        "type": "des.simulation.job",
        "source": "dd-des-simulator",
        "jobId": snapshot.job_id,
        "requestId": snapshot.request_id,
        "status": snapshot.status,
        "atMs": now_ms(),
    });
    if let Err(error) = nats
        .publish(state.event_subject.clone(), payload.to_string().into())
        .await
    {
        eprintln!("failed to publish des job event: {error}");
    }
}

async fn publish_result(state: &AppState, snapshot: &SimulationJobSnapshot) {
    let Some(nats) = &state.nats else {
        return;
    };
    let payload = match serde_json::to_vec(&json!({
        "messageKind": "des.simulation.result",
        "source": "dd-des-simulator",
        "job": snapshot,
    })) {
        Ok(payload) => payload,
        Err(error) => {
            eprintln!("failed to encode des result: {error}");
            return;
        }
    };
    if let Err(error) = nats
        .publish(state.result_subject.clone(), payload.into())
        .await
    {
        eprintln!("failed to publish des result: {error}");
    }
    publish_job_event(state, snapshot).await;
}

fn insert_pending_job(
    state: &AppState,
    job_id: String,
    request_id: String,
    source: String,
) -> SimulationJobSnapshot {
    let snapshot = SimulationJobSnapshot {
        ok: true,
        job_id: job_id.clone(),
        request_id,
        status: "queued".to_string(),
        submitted_at_ms: now_ms(),
        started_at_ms: None,
        finished_at_ms: None,
        source,
        result: None,
        error: None,
    };
    let mut jobs = state.jobs.lock().expect("job store mutex poisoned");
    prune_jobs(&mut jobs);
    jobs.insert(job_id, snapshot.clone());
    snapshot
}

fn next_job_id(state: &AppState) -> String {
    let sequence = state.job_sequence.fetch_add(1, Ordering::Relaxed) + 1;
    format!("des-{}-{sequence}", now_ms())
}

fn reserve_active_job_slot(state: &AppState) -> Result<(), StartJobError> {
    state
        .metrics
        .jobs_running
        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |running| {
            (running < MAX_ACTIVE_JOBS).then_some(running + 1)
        })
        .map(|_| ())
        .map_err(|running| {
            StartJobError::Busy(format!(
                "too many active DES jobs; maxActiveJobs={MAX_ACTIVE_JOBS} running={running}"
            ))
        })
}

fn start_simulation_job(
    state: AppState,
    request: SimulationRequest,
    source: &str,
) -> Result<SimulationAcceptedResponse, StartJobError> {
    let validation = validate_simulation_request(&request);
    if !validation.ok {
        state
            .metrics
            .validation_errors_total
            .fetch_add(1, Ordering::Relaxed);
        return Err(StartJobError::Invalid(validation.errors.join("; ")));
    }

    reserve_active_job_slot(&state)?;
    let job_id = next_job_id(&state);
    let request_id = request_identifier(&request, &job_id);
    let snapshot = insert_pending_job(
        &state,
        job_id.clone(),
        request_id.clone(),
        source.to_string(),
    );
    state
        .metrics
        .jobs_started_total
        .fetch_add(1, Ordering::Relaxed);

    let worker_state = state.clone();
    let worker_job_id = job_id.clone();
    tokio::spawn(async move {
        update_job(&worker_state, &worker_job_id, |snapshot| {
            snapshot.status = "running".to_string();
            snapshot.started_at_ms = Some(now_ms());
        });
        let result = tokio::task::spawn_blocking({
            let request = request.clone();
            let job_id = worker_job_id.clone();
            move || simulate(request, job_id)
        })
        .await
        .map_err(|error| format!("simulation task join failed: {error}"))
        .and_then(|result| result);

        let final_snapshot = {
            let mut jobs = worker_state.jobs.lock().expect("job store mutex poisoned");
            let snapshot = jobs
                .get_mut(&worker_job_id)
                .expect("simulation job missing from store");
            snapshot.finished_at_ms = Some(now_ms());
            match result {
                Ok(result) => {
                    snapshot.status = "succeeded".to_string();
                    snapshot.result = Some(result);
                    snapshot.error = None;
                    worker_state
                        .metrics
                        .jobs_completed_total
                        .fetch_add(1, Ordering::Relaxed);
                }
                Err(error) => {
                    snapshot.ok = false;
                    snapshot.status = "failed".to_string();
                    snapshot.error = Some(error);
                    worker_state
                        .metrics
                        .jobs_failed_total
                        .fetch_add(1, Ordering::Relaxed);
                    worker_state
                        .metrics
                        .errors_total
                        .fetch_add(1, Ordering::Relaxed);
                }
            }
            snapshot.clone()
        };
        worker_state
            .metrics
            .jobs_running
            .fetch_sub(1, Ordering::Relaxed);
        publish_result(&worker_state, &final_snapshot).await;
    });

    Ok(SimulationAcceptedResponse {
        ok: true,
        job_id,
        request_id,
        status: snapshot.status,
        status_url: format!("/simulations/{}", snapshot.job_id),
        result_subject: state.result_subject,
        submitted_at_ms: snapshot.submitted_at_ms,
    })
}

async fn root() -> impl IntoResponse {
    Json(json!({
        "ok": true,
        "service": "dd-des-simulator",
        "mode": "async-discrete-event-simulation",
        "schemaVersion": MODEL_SCHEMA_VERSION,
        "endpoints": {
            "healthz": "GET /healthz",
            "metrics": "GET /metrics",
            "schema": "GET /model/schema",
            "example": "GET /model/example",
            "examples": "GET /model/examples",
            "namedExample": "GET /model/examples/:name",
            "validate": "POST /validate",
            "simulate": "POST /simulate",
            "jobStatus": "GET /simulations/:jobId"
        },
        "nats": {
            "simulateSubject": "dd.remote.des.simulate",
            "resultSubject": "dd.remote.des.results"
        },
        "atMs": now_ms()
    }))
}

async fn healthz() -> impl IntoResponse {
    Json(json!({
        "ok": true,
        "service": "dd-des-simulator",
        "mode": "async-des-nats",
        "schemaVersion": MODEL_SCHEMA_VERSION,
        "atMs": now_ms()
    }))
}

async fn schema_http() -> impl IntoResponse {
    Json(model_schema())
}

async fn example_http() -> impl IntoResponse {
    Json(example_request())
}

async fn examples_http() -> impl IntoResponse {
    Json(examples_index())
}

async fn named_example_http(Path(name): Path<String>) -> Response {
    match example_request_by_name(&name) {
        Some(example) => Json(example).into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(json!({
                "ok": false,
                "schemaVersion": MODEL_SCHEMA_VERSION,
                "error": format!("unknown DES example {name}"),
                "examples": examples_index()["examples"].clone()
            })),
        )
            .into_response(),
    }
}

async fn simulate_http(
    State(state): State<AppState>,
    Json(request): Json<SimulationRequest>,
) -> Response {
    state.metrics.requests_total.fetch_add(1, Ordering::Relaxed);
    match start_simulation_job(state.clone(), request, "http") {
        Ok(accepted) => (StatusCode::ACCEPTED, Json(accepted)).into_response(),
        Err(error) => {
            state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
            (
                error.status_code(),
                Json(json!({
                    "ok": false,
                    "schemaVersion": MODEL_SCHEMA_VERSION,
                    "error": error.message()
                })),
            )
                .into_response()
        }
    }
}

async fn validate_with_metrics(
    State(state): State<AppState>,
    Json(request): Json<SimulationRequest>,
) -> Response {
    state
        .metrics
        .validation_requests_total
        .fetch_add(1, Ordering::Relaxed);
    let validation = validate_simulation_request(&request);
    if !validation.ok {
        state
            .metrics
            .validation_errors_total
            .fetch_add(1, Ordering::Relaxed);
    }
    let status = if validation.ok {
        StatusCode::OK
    } else {
        StatusCode::BAD_REQUEST
    };
    (status, Json(validation)).into_response()
}

async fn job_status(State(state): State<AppState>, Path(job_id): Path<String>) -> Response {
    let jobs = state.jobs.lock().expect("job store mutex poisoned");
    match jobs.get(&job_id) {
        Some(snapshot) => Json(snapshot).into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(json!({
                "ok": false,
                "error": format!("unknown simulation job {job_id}")
            })),
        )
            .into_response(),
    }
}

async fn metrics(State(state): State<AppState>) -> Response {
    let body = format!(
        "# HELP dd_des_simulator_requests_total HTTP simulation requests accepted for validation.\n\
         # TYPE dd_des_simulator_requests_total counter\n\
         dd_des_simulator_requests_total {}\n\
         # HELP dd_des_simulator_validation_requests_total HTTP model validation requests.\n\
         # TYPE dd_des_simulator_validation_requests_total counter\n\
         dd_des_simulator_validation_requests_total {}\n\
         # HELP dd_des_simulator_jobs_started_total Simulation jobs started.\n\
         # TYPE dd_des_simulator_jobs_started_total counter\n\
         dd_des_simulator_jobs_started_total {}\n\
         # HELP dd_des_simulator_jobs_completed_total Simulation jobs completed successfully.\n\
         # TYPE dd_des_simulator_jobs_completed_total counter\n\
         dd_des_simulator_jobs_completed_total {}\n\
         # HELP dd_des_simulator_jobs_failed_total Simulation jobs failed.\n\
         # TYPE dd_des_simulator_jobs_failed_total counter\n\
         dd_des_simulator_jobs_failed_total {}\n\
         # HELP dd_des_simulator_jobs_running Current in-process simulation jobs.\n\
         # TYPE dd_des_simulator_jobs_running gauge\n\
         dd_des_simulator_jobs_running {}\n\
         # HELP dd_des_simulator_max_active_jobs Configured active simulation job limit.\n\
         # TYPE dd_des_simulator_max_active_jobs gauge\n\
         dd_des_simulator_max_active_jobs {}\n\
         # HELP dd_des_simulator_errors_total Runtime, validation, or queue errors.\n\
         # TYPE dd_des_simulator_errors_total counter\n\
         dd_des_simulator_errors_total {}\n\
         # HELP dd_des_simulator_validation_errors_total Rejected model validation attempts.\n\
         # TYPE dd_des_simulator_validation_errors_total counter\n\
         dd_des_simulator_validation_errors_total {}\n\
         # HELP dd_des_simulator_nats_messages_total NATS simulation messages received.\n\
         # TYPE dd_des_simulator_nats_messages_total counter\n\
         dd_des_simulator_nats_messages_total {}\n",
        state.metrics.requests_total.load(Ordering::Relaxed),
        state
            .metrics
            .validation_requests_total
            .load(Ordering::Relaxed),
        state.metrics.jobs_started_total.load(Ordering::Relaxed),
        state.metrics.jobs_completed_total.load(Ordering::Relaxed),
        state.metrics.jobs_failed_total.load(Ordering::Relaxed),
        state.metrics.jobs_running.load(Ordering::Relaxed),
        MAX_ACTIVE_JOBS,
        state.metrics.errors_total.load(Ordering::Relaxed),
        state
            .metrics
            .validation_errors_total
            .load(Ordering::Relaxed),
        state.metrics.nats_messages_total.load(Ordering::Relaxed),
    );
    (
        [(
            axum::http::header::CONTENT_TYPE,
            "text/plain; version=0.0.4",
        )],
        body,
    )
        .into_response()
}

async fn run_nats_loop(state: AppState, subject: String, queue_group: String) {
    let Some(nats) = state.nats.clone() else {
        println!("des simulator nats loop disabled: NATS_URL is not configured");
        return;
    };
    println!(
        "des simulator nats loop starting: subject={subject} queue_group={queue_group} resultSubject={}",
        state.result_subject
    );
    let mut subscription = match nats.queue_subscribe(subject, queue_group).await {
        Ok(subscription) => subscription,
        Err(error) => {
            eprintln!("des simulator nats subscribe failed: {error}");
            return;
        }
    };
    while let Some(message) = subscription.next().await {
        state
            .metrics
            .nats_messages_total
            .fetch_add(1, Ordering::Relaxed);
        let payload = message.payload.to_vec();
        if payload.len() > MAX_NATS_PAYLOAD_BYTES {
            state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
            eprintln!(
                "des simulator rejected oversize nats request: bytes={} max={MAX_NATS_PAYLOAD_BYTES}",
                payload.len()
            );
            continue;
        }

        match serde_json::from_slice::<SimulationRequest>(&payload) {
            Ok(request) => {
                if let Err(error) = start_simulation_job(state.clone(), request, "nats") {
                    state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
                    eprintln!(
                        "des simulator rejected nats simulation: {}",
                        error.message()
                    );
                }
            }
            Err(error) => {
                state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
                eprintln!("des simulator invalid nats request: {error}");
            }
        }
    }
}

async fn api_docs_html() -> axum::response::Html<&'static str> {
    axum::response::Html(include_str!("../generated/api-docs.html"))
}

async fn api_docs_json() -> impl axum::response::IntoResponse {
    (
        [("content-type", "application/json; charset=utf-8")],
        include_str!("../generated/api-docs.json"),
    )
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    let host = env_value("HOST", "0.0.0.0");
    let port = env_value("PORT", "8099").parse::<u16>()?;
    let nats = match env::var("NATS_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
    {
        Some(url) => Some(async_nats::connect(url).await?),
        None => None,
    };
    let state = AppState {
        nats,
        result_subject: env_value("DES_RESULT_SUBJECT", "dd.remote.des.results"),
        event_subject: env_value("DES_EVENT_SUBJECT", "dd.remote.events"),
        jobs: Arc::new(Mutex::new(HashMap::new())),
        metrics: Arc::new(Metrics::default()),
        job_sequence: Arc::new(AtomicU64::new(0)),
    };
    let nats_subject = env_value("DES_SIMULATE_SUBJECT", "dd.remote.des.simulate");
    let queue_group = env_value("DES_QUEUE_GROUP", "dd-des-simulator");
    tokio::spawn(run_nats_loop(state.clone(), nats_subject, queue_group));

    let app = Router::new()
        .route("/", get(root))
        .route("/healthz", get(healthz))
        .route("/docs/api", get(api_docs_html))
        .route("/api/docs", get(api_docs_html))
        .route("/api/docs.json", get(api_docs_json))
        .route("/metrics", get(metrics))
        .route("/model/schema", get(schema_http))
        .route("/model/example", get(example_http))
        .route("/model/examples", get(examples_http))
        .route("/model/examples/:name", get(named_example_http))
        .route("/validate", post(validate_with_metrics))
        .route("/simulate", post(simulate_http))
        .route("/simulations/:job_id", get(job_status))
        .layer(DefaultBodyLimit::max(MAX_HTTP_BODY_BYTES))
        .with_state(state)
        .merge(dd_runtime_config_client::router());

    tokio::spawn(dd_runtime_config_client::register_with_control_plane());

    let addr: SocketAddr = format!("{host}:{port}").parse()?;
    println!("dd-des-simulator listening on http://{addr}");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    tokio::time::sleep(Duration::from_millis(10)).await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixed(value: f64) -> DelaySpec {
        DelaySpec::Fixed { value }
    }

    fn event_type(name: &str) -> EventTypeDefinition {
        EventTypeDefinition {
            name: name.to_string(),
        }
    }

    fn resource(name: &str, capacity: u32) -> ResourceDefinition {
        ResourceDefinition {
            name: name.to_string(),
            capacity,
        }
    }

    fn initial(at: f64, event_type: &str, entity_id: &str) -> ScheduledEventInput {
        ScheduledEventInput {
            at,
            event_type: event_type.to_string(),
            entity_id: Some(entity_id.to_string()),
            attributes: None,
        }
    }

    fn transition(from: &str, to: Option<&str>) -> TransitionRule {
        TransitionRule {
            name: Some("service".to_string()),
            from: from.to_string(),
            to: to.map(|value| value.to_string()),
            delay: fixed(0.0),
            probability: None,
            resource: Some(ResourceUsage {
                name: "server".to_string(),
                units: 1,
                duration: fixed(5.0),
            }),
            limit: None,
            attributes: None,
        }
    }

    fn request() -> SimulationRequest {
        SimulationRequest {
            request_id: Some("unit-des".to_string()),
            model: SimulationModel {
                schema_version: MODEL_SCHEMA_VERSION.to_string(),
                name: Some("unit-queue".to_string()),
                time_unit: Some("minutes".to_string()),
                start_time: Some(0.0),
                seed: Some(7),
                event_types: vec![event_type("arrival"), event_type("done")],
                resources: Some(vec![resource("server", 1)]),
                initial_events: vec![initial(0.0, "arrival", "a"), initial(0.0, "arrival", "b")],
                transitions: vec![transition("arrival", Some("done"))],
                metrics: Some(vec![MetricDefinition {
                    name: "completed".to_string(),
                    event_type: "done".to_string(),
                    kind: Some("count".to_string()),
                }]),
            },
            options: Some(SimulationOptions {
                until: Some(20.0),
                max_events: Some(20),
                trace: Some(true),
            }),
        }
    }

    fn event_count(result: &SimulationResult, event_type: &str) -> u64 {
        result
            .event_counts
            .iter()
            .find(|entry| entry.event_type == event_type)
            .unwrap_or_else(|| panic!("missing event count {event_type}"))
            .count
    }

    fn resource_summary<'a>(result: &'a SimulationResult, name: &str) -> &'a ResourceSummary {
        result
            .resources
            .iter()
            .find(|entry| entry.name == name)
            .unwrap_or_else(|| panic!("missing resource summary {name}"))
    }

    #[test]
    fn validates_declared_model_format() {
        let validation = validate_simulation_request(&request());

        assert!(validation.ok);
        assert_eq!(validation.schema_version, MODEL_SCHEMA_VERSION);
        assert!(validation.errors.is_empty());
    }

    #[test]
    fn rejects_unknown_transition_targets() {
        let mut request = request();
        request.model.transitions[0].to = Some("missing".to_string());

        let validation = validate_simulation_request(&request);

        assert!(!validation.ok);
        assert!(
            validation
                .errors
                .iter()
                .any(|error| error
                    .contains("transitions[0].to references unknown event type missing"))
        );
    }

    #[test]
    fn rejects_invalid_probabilities_before_running() {
        let mut request = request();
        request.model.transitions[0].probability = Some(1.4);

        let error = simulation_config(&request).expect_err("invalid probability should fail");

        assert!(error.contains("transitions[0].probability must be finite and in [0, 1]"));
    }

    #[test]
    fn runs_resource_queued_discrete_event_simulation() {
        let result = simulate(request(), "unit-job".to_string()).expect("simulation succeeds");

        assert_eq!(result.request_id, "unit-des");
        assert_eq!(result.processed_events, 4);
        assert_eq!(result.generated_events, 2);
        assert_eq!(event_count(&result, "arrival"), 2);
        assert_eq!(event_count(&result, "done"), 2);
        assert_eq!(result.metric_values[0].value, 2.0);
        assert_eq!(result.resources[0].allocations, 2);
        assert!((result.resources[0].utilization - 1.0).abs() < 1e-9);
        assert!(result
            .trace
            .iter()
            .any(|entry| entry.kind == "scheduled" && (entry.at - 10.0).abs() < 1e-9));
    }

    #[test]
    fn zero_probability_transition_does_not_schedule_followup() {
        let mut request = request();
        request.model.transitions[0].probability = Some(0.0);

        let result = simulate(request, "unit-job".to_string()).expect("simulation succeeds");

        assert_eq!(event_count(&result, "arrival"), 2);
        assert_eq!(event_count(&result, "done"), 0);
        assert_eq!(result.generated_events, 0);
    }

    #[test]
    fn fibonacci_example_runs_as_discrete_control_sequence() {
        let request = serde_json::from_value::<SimulationRequest>(fibonacci_example_request())
            .expect("example should deserialize");
        let validation = validate_simulation_request(&request);
        assert!(validation.ok, "{:?}", validation.errors);

        let result = simulate(request, "fib-job".to_string()).expect("simulation succeeds");
        let expected_counts = [
            ("fib0", 1),
            ("fib1", 1),
            ("fib2", 2),
            ("fib3", 3),
            ("fib4", 5),
            ("fib5", 8),
            ("fib6", 13),
            ("fib7", 21),
            ("fib8", 34),
        ];

        assert_eq!(result.processed_events, 88);
        assert_eq!(result.generated_events, 87);
        assert!(!result.truncated);
        for (event_type, count) in expected_counts {
            assert_eq!(event_count(&result, event_type), count);
        }
        assert_eq!(
            result
                .metric_values
                .iter()
                .find(|metric| metric.name == "fib8_count")
                .expect("fib8 metric")
                .value,
            34.0
        );
    }

    #[test]
    fn temperature_control_example_runs_heat_hold_cool_events() {
        let request =
            serde_json::from_value::<SimulationRequest>(temperature_control_example_request())
                .expect("example should deserialize");
        let validation = validate_simulation_request(&request);
        assert!(validation.ok, "{:?}", validation.errors);

        let result = simulate(request, "temp-job".to_string()).expect("simulation succeeds");

        assert_eq!(event_count(&result, "sampleCold"), 2);
        assert_eq!(event_count(&result, "commandHeat"), 2);
        assert_eq!(event_count(&result, "sampleComfort"), 4);
        assert_eq!(event_count(&result, "commandHold"), 3);
        assert_eq!(event_count(&result, "sampleHot"), 2);
        assert_eq!(event_count(&result, "commandCool"), 2);
        assert_eq!(resource_summary(&result, "heater").allocations, 2);
        assert_eq!(resource_summary(&result, "cooler").allocations, 2);
        assert_eq!(
            result
                .metric_values
                .iter()
                .find(|metric| metric.name == "comfort_samples")
                .expect("comfort metric")
                .value,
            4.0
        );
        assert!(!result.truncated);
    }

    #[test]
    fn schema_declares_des_v1_and_async_payload_shape() {
        let schema = model_schema();

        assert_eq!(
            schema["properties"]["model"]["properties"]["schemaVersion"]["const"],
            MODEL_SCHEMA_VERSION
        );
        assert!(
            schema["$defs"]["transition"]["properties"]["resource"]["required"]
                .as_array()
                .unwrap()
                .iter()
                .any(|value| value == "duration")
        );
        assert_eq!(
            schema["properties"]["options"]["properties"]["maxEvents"]["maximum"],
            MAX_EVENTS
        );
    }
}
