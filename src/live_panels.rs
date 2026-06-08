use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::{
    util::{clean_identifier, now_ms},
    Column, Dataset,
};

const DEFAULT_LIVE_TICKS: usize = 5;
const MAX_LIVE_TICKS: usize = 50;
const DEFAULT_LIVE_INTERVAL_MS: u64 = 1_000;
const MIN_LIVE_INTERVAL_MS: u64 = 100;
const MAX_LIVE_INTERVAL_MS: u64 = 10_000;
const DEFAULT_LIVE_ROWS: usize = 100;
const MAX_LIVE_ROWS: usize = 500;
const MAX_LIVE_FIELDS: usize = 16;
const MAX_PANEL_ID_BYTES: usize = 80;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LivePanelStreamRequest {
    panel_id: Option<String>,
    fields: Option<String>,
    limit: Option<usize>,
    interval_ms: Option<u64>,
    ticks: Option<usize>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LivePanelSubscription {
    pub(crate) dataset_id: String,
    pub(crate) panel_id: String,
    pub(crate) fields: Vec<String>,
    pub(crate) limit: usize,
    pub(crate) interval_ms: u64,
    pub(crate) ticks: usize,
    pub(crate) panel_kind: &'static str,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct LivePanelField {
    name: String,
    data_type: String,
    semantic_role: &'static str,
}

pub(crate) fn subscribe(
    dataset: &Dataset,
    dataset_id: String,
    request: LivePanelStreamRequest,
) -> Result<LivePanelSubscription, String> {
    if dataset_id != dataset.dataset_id {
        return Err(format!(
            "dataset `{dataset_id}` does not match loaded dataset"
        ));
    }
    let panel_id = normalize_panel_id(request.panel_id)?;
    let fields = normalize_fields(dataset, request.fields)?;
    let limit = request
        .limit
        .unwrap_or(DEFAULT_LIVE_ROWS)
        .clamp(1, MAX_LIVE_ROWS);
    let interval_ms = request
        .interval_ms
        .unwrap_or(DEFAULT_LIVE_INTERVAL_MS)
        .clamp(MIN_LIVE_INTERVAL_MS, MAX_LIVE_INTERVAL_MS);
    let ticks = request
        .ticks
        .unwrap_or(DEFAULT_LIVE_TICKS)
        .clamp(1, MAX_LIVE_TICKS);
    let panel_kind = infer_panel_kind(dataset, &fields);

    Ok(LivePanelSubscription {
        dataset_id,
        panel_id,
        fields,
        limit,
        interval_ms,
        ticks,
        panel_kind,
    })
}

pub(crate) fn open_event(subscription: &LivePanelSubscription) -> Value {
    json!({
        "ok": true,
        "schemaVersion": "data-viz.live-panel-stream.v1",
        "event": "stream-open",
        "subscription": subscription,
        "transport": {
            "protocol": "websocket",
            "frameSchema": "data-viz.live-panel-frame.v1",
            "mode": "bounded-snapshot-stream"
        }
    })
}

pub(crate) fn frame(
    dataset: &Dataset,
    subscription: &LivePanelSubscription,
    sequence: usize,
) -> Result<Value, String> {
    if dataset.dataset_id != subscription.dataset_id {
        return Err(format!(
            "dataset `{}` no longer matches subscription `{}`",
            dataset.dataset_id, subscription.dataset_id
        ));
    }
    let fields = validate_subscription_fields(dataset, &subscription.fields)?;
    let start = dataset.row_count.saturating_sub(subscription.limit);
    let rows = (start..dataset.row_count)
        .map(|row_index| {
            let mut row = BTreeMap::new();
            row.insert("__rowNumber".to_string(), Value::from(row_index + 1));
            for field in &fields {
                row.insert(field.clone(), dataset.value(field, row_index));
            }
            row
        })
        .collect::<Vec<_>>();

    Ok(json!({
        "ok": true,
        "schemaVersion": "data-viz.live-panel-frame.v1",
        "event": "panel-frame",
        "stream": {
            "datasetId": subscription.dataset_id,
            "panelId": subscription.panel_id,
            "sequence": sequence,
            "emittedAtMs": now_ms(),
            "intervalMs": subscription.interval_ms,
            "maxTicks": subscription.ticks,
            "remainingTicks": subscription.ticks.saturating_sub(sequence.saturating_add(1))
        },
        "panel": {
            "kind": subscription.panel_kind,
            "refresh": "bounded-websocket-snapshot",
            "diffStrategy": "full-frame snapshot now; row-level diffs can replace this payload shape later"
        },
        "data": {
            "datasetUpdatedAtMs": dataset.updated_at_ms,
            "totalRows": dataset.row_count,
            "returnedRows": rows.len(),
            "fields": fields
                .iter()
                .map(|field| LivePanelField {
                    name: field.clone(),
                    data_type: dataset.field_type(field),
                    semantic_role: semantic_role(field, dataset.columns.get(field)),
                })
                .collect::<Vec<_>>(),
            "rows": rows
        }
    }))
}

pub(crate) fn close_event(subscription: &LivePanelSubscription, sent_frames: usize) -> Value {
    json!({
        "ok": true,
        "schemaVersion": "data-viz.live-panel-stream.v1",
        "event": "stream-complete",
        "datasetId": subscription.dataset_id,
        "panelId": subscription.panel_id,
        "sentFrames": sent_frames
    })
}

pub(crate) fn error_event(message: impl Into<String>) -> Value {
    json!({
        "ok": false,
        "schemaVersion": "data-viz.live-panel-stream.v1",
        "event": "stream-error",
        "error": message.into()
    })
}

pub(crate) fn limits_payload() -> Value {
    json!({
        "defaultTicks": DEFAULT_LIVE_TICKS,
        "maxTicks": MAX_LIVE_TICKS,
        "defaultIntervalMs": DEFAULT_LIVE_INTERVAL_MS,
        "minIntervalMs": MIN_LIVE_INTERVAL_MS,
        "maxIntervalMs": MAX_LIVE_INTERVAL_MS,
        "defaultRows": DEFAULT_LIVE_ROWS,
        "maxRows": MAX_LIVE_ROWS,
        "maxFields": MAX_LIVE_FIELDS,
        "maxPanelIdBytes": MAX_PANEL_ID_BYTES
    })
}

fn normalize_panel_id(panel_id: Option<String>) -> Result<String, String> {
    let Some(panel_id) = panel_id
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
    else {
        return Ok("live-panel".to_string());
    };
    if panel_id.len() > MAX_PANEL_ID_BYTES {
        return Err(format!("panelId exceeds max {MAX_PANEL_ID_BYTES} bytes"));
    }
    clean_identifier(&panel_id).ok_or_else(|| "panelId must be a safe identifier".to_string())
}

fn normalize_fields(dataset: &Dataset, fields: Option<String>) -> Result<Vec<String>, String> {
    let fields = match fields {
        Some(fields) => fields
            .split(',')
            .map(str::trim)
            .filter(|field| !field.is_empty())
            .map(|field| {
                clean_identifier(field).ok_or_else(|| format!("invalid live panel field `{field}`"))
            })
            .collect::<Result<Vec<_>, _>>()?,
        None => default_fields(dataset),
    };
    if fields.is_empty() {
        return Err("live panel fields cannot be empty".to_string());
    }
    if fields.len() > MAX_LIVE_FIELDS {
        return Err(format!("live panel fields exceeds max {MAX_LIVE_FIELDS}"));
    }
    validate_subscription_fields(dataset, &fields)
}

fn validate_subscription_fields(
    dataset: &Dataset,
    fields: &[String],
) -> Result<Vec<String>, String> {
    let mut normalized = Vec::with_capacity(fields.len());
    for field in fields {
        let field =
            clean_identifier(field).ok_or_else(|| format!("invalid live panel field `{field}`"))?;
        if !dataset.columns.contains_key(&field) {
            return Err(format!("live panel field `{field}` does not exist"));
        }
        if !normalized.contains(&field) {
            normalized.push(field);
        }
    }
    Ok(normalized)
}

fn default_fields(dataset: &Dataset) -> Vec<String> {
    let mut fields = Vec::new();
    push_matching(dataset, &mut fields, |name, _| is_time_field(name));
    push_matching(dataset, &mut fields, |_, column| {
        matches!(column, Column::Number(_))
    });
    push_matching(dataset, &mut fields, |_, column| {
        matches!(column, Column::Dictionary { .. } | Column::Boolean(_))
    });
    fields.truncate(MAX_LIVE_FIELDS.min(6));
    fields
}

fn push_matching(
    dataset: &Dataset,
    fields: &mut Vec<String>,
    predicate: impl Fn(&str, &Column) -> bool,
) {
    for (name, column) in &dataset.columns {
        if fields.len() >= MAX_LIVE_FIELDS {
            return;
        }
        if predicate(name, column) && !fields.contains(name) {
            fields.push(name.clone());
        }
    }
}

fn infer_panel_kind(dataset: &Dataset, fields: &[String]) -> &'static str {
    let has_time = fields.iter().any(|field| is_time_field(field));
    let has_numeric = fields
        .iter()
        .filter_map(|field| dataset.columns.get(field))
        .any(|column| matches!(column, Column::Number(_)));
    match (has_time, has_numeric) {
        (true, true) => "time-series",
        (false, true) => "stat-table",
        _ => "log-table",
    }
}

fn semantic_role(field: &str, column: Option<&Column>) -> &'static str {
    if is_time_field(field) {
        "time"
    } else if matches!(column, Some(Column::Number(_))) {
        "metric"
    } else {
        "label"
    }
}

fn is_time_field(field: &str) -> bool {
    let lower = field.to_ascii_lowercase();
    lower == "ts"
        || lower == "time"
        || lower == "timestamp"
        || lower.ends_with("_time")
        || lower.ends_with("_at")
        || lower.ends_with("_date")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::IngestDatasetRequest;

    fn record(pairs: Vec<(&str, Value)>) -> BTreeMap<String, Value> {
        pairs
            .into_iter()
            .map(|(key, value)| (key.to_string(), value))
            .collect()
    }

    fn dataset() -> Dataset {
        Dataset::from_request(IngestDatasetRequest {
            dataset_id: "metrics-live".to_string(),
            display_name: None,
            replace: Some(true),
            records: vec![
                record(vec![
                    ("timestamp", Value::from(1_710_000_000_000u64)),
                    ("cpu", Value::from(0.7)),
                    ("host", Value::from("api-1")),
                ]),
                record(vec![
                    ("timestamp", Value::from(1_710_000_001_000u64)),
                    ("cpu", Value::from(0.8)),
                    ("host", Value::from("api-1")),
                ]),
                record(vec![
                    ("timestamp", Value::from(1_710_000_002_000u64)),
                    ("cpu", Value::from(0.9)),
                    ("host", Value::from("api-2")),
                ]),
            ],
        })
        .expect("dataset builds")
    }

    #[test]
    fn live_panel_subscription_builds_bounded_time_series_frame() {
        let dataset = dataset();
        let subscription = subscribe(
            &dataset,
            "metrics-live".to_string(),
            LivePanelStreamRequest {
                panel_id: Some("cpu-live".to_string()),
                fields: Some("timestamp,cpu,host".to_string()),
                limit: Some(2),
                interval_ms: Some(10),
                ticks: Some(2),
            },
        )
        .expect("subscription");
        let frame = frame(&dataset, &subscription, 0).expect("frame");

        assert_eq!(subscription.interval_ms, MIN_LIVE_INTERVAL_MS);
        assert_eq!(subscription.panel_kind, "time-series");
        assert_eq!(frame["data"]["returnedRows"], 2);
        assert_eq!(frame["data"]["fields"][0]["semanticRole"], "time");
        assert_eq!(frame["data"]["rows"][0]["__rowNumber"], 2);
    }

    #[test]
    fn live_panel_rejects_missing_fields() {
        let error = subscribe(
            &dataset(),
            "metrics-live".to_string(),
            LivePanelStreamRequest {
                panel_id: None,
                fields: Some("timestamp,missing".to_string()),
                limit: None,
                interval_ms: None,
                ticks: None,
            },
        )
        .expect_err("missing field rejected");

        assert!(error.contains("does not exist"));
    }
}
