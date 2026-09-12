use std::{cmp::Ordering, collections::BTreeMap};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::util::{clean_identifier, scalar_to_label};

const MAX_LOKI_STREAMS: usize = 128;
const MAX_LOKI_ENTRIES: usize = 5_000;
const DEFAULT_LOKI_ENTRIES: usize = 1_000;
const MAX_LOKI_LABELS: usize = 32;
const MAX_LOKI_LABEL_BYTES: usize = 256;
const MAX_LOKI_LINE_BYTES: usize = 4 * 1024;
const MAX_LOKI_QUERY_BYTES: usize = 4 * 1024;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LokiFrameRequest {
    pub query: Option<String>,
    pub streams: Option<Vec<LokiStreamInput>>,
    pub loki_response: Option<Value>,
    pub max_entries: Option<usize>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LokiStreamInput {
    pub labels: Option<BTreeMap<String, String>>,
    pub stream: Option<BTreeMap<String, String>>,
    pub values: Option<Vec<Value>>,
    pub entries: Option<Vec<LokiEntryInput>>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LokiEntryInput {
    pub timestamp: Value,
    pub line: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LokiFrameResponse {
    ok: bool,
    schema_version: &'static str,
    query: Option<String>,
    stream_count: usize,
    entry_count: usize,
    dropped_entries: usize,
    label_keys: Vec<String>,
    frame: LokiFrame,
    level_counts: BTreeMap<String, usize>,
    limits: Value,
    warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct LokiFrame {
    name: &'static str,
    fields: Vec<LokiField>,
    rows: Vec<LokiFrameRow>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct LokiField {
    name: &'static str,
    data_type: &'static str,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct LokiFrameRow {
    stream_index: usize,
    timestamp_ns: String,
    timestamp_ms: Option<u128>,
    level: String,
    line: String,
    labels: BTreeMap<String, String>,
}

#[derive(Debug, Clone)]
struct NormalizedStream {
    labels: BTreeMap<String, String>,
    entries: Vec<NormalizedEntry>,
}

#[derive(Debug, Clone)]
struct NormalizedEntry {
    timestamp_ns: String,
    line: String,
}

pub(crate) fn frame(request: LokiFrameRequest) -> Result<LokiFrameResponse, String> {
    let query = normalize_query(request.query)?;
    let max_entries = request
        .max_entries
        .unwrap_or(DEFAULT_LOKI_ENTRIES)
        .clamp(1, MAX_LOKI_ENTRIES);
    let mut warnings = Vec::new();
    let mut streams = Vec::new();

    if let Some(input_streams) = request.streams {
        if input_streams.len() > MAX_LOKI_STREAMS {
            return Err(format!("Loki streams exceeds max {MAX_LOKI_STREAMS}"));
        }
        for stream in input_streams {
            streams.push(normalize_input_stream(stream, &mut warnings)?);
        }
    }
    if let Some(response) = request.loki_response {
        streams.extend(normalize_loki_response(&response, &mut warnings)?);
    }
    if streams.is_empty() {
        return Err("Loki frame request requires at least one stream".to_string());
    }
    if streams.len() > MAX_LOKI_STREAMS {
        return Err(format!("Loki streams exceeds max {MAX_LOKI_STREAMS}"));
    }

    let label_keys = label_keys(&streams);
    let mut rows = Vec::new();
    let mut dropped_entries = 0;
    let mut level_counts = BTreeMap::<String, usize>::new();
    for (stream_index, stream) in streams.iter().enumerate() {
        for entry in &stream.entries {
            if rows.len() >= max_entries {
                dropped_entries += 1;
                continue;
            }
            let line = sanitize_line(&entry.line, &mut warnings);
            let level = infer_level(&stream.labels, &line);
            *level_counts.entry(level.clone()).or_default() += 1;
            rows.push(LokiFrameRow {
                stream_index,
                timestamp_ns: entry.timestamp_ns.clone(),
                timestamp_ms: timestamp_ns_to_ms(&entry.timestamp_ns),
                level,
                line,
                labels: stream.labels.clone(),
            });
        }
    }
    rows.sort_by(compare_frame_rows);

    Ok(LokiFrameResponse {
        ok: true,
        schema_version: "data-viz.loki-frame.v1",
        query,
        stream_count: streams.len(),
        entry_count: rows.len(),
        dropped_entries,
        label_keys,
        frame: LokiFrame {
            name: "loki-log-frame",
            fields: vec![
                LokiField {
                    name: "streamIndex",
                    data_type: "u64",
                },
                LokiField {
                    name: "timestampNs",
                    data_type: "string",
                },
                LokiField {
                    name: "timestampMs",
                    data_type: "time",
                },
                LokiField {
                    name: "level",
                    data_type: "string",
                },
                LokiField {
                    name: "line",
                    data_type: "string",
                },
                LokiField {
                    name: "labels",
                    data_type: "json",
                },
            ],
            rows,
        },
        level_counts,
        limits: limits_payload(),
        warnings,
    })
}

pub(crate) fn limits_payload() -> Value {
    json!({
        "maxStreams": MAX_LOKI_STREAMS,
        "defaultEntries": DEFAULT_LOKI_ENTRIES,
        "maxEntries": MAX_LOKI_ENTRIES,
        "maxLabels": MAX_LOKI_LABELS,
        "maxLabelBytes": MAX_LOKI_LABEL_BYTES,
        "maxLineBytes": MAX_LOKI_LINE_BYTES,
        "maxQueryBytes": MAX_LOKI_QUERY_BYTES
    })
}

fn normalize_query(query: Option<String>) -> Result<Option<String>, String> {
    let Some(query) = query
        .map(|query| query.trim().to_string())
        .filter(|query| !query.is_empty())
    else {
        return Ok(None);
    };
    if query.len() > MAX_LOKI_QUERY_BYTES {
        return Err(format!(
            "Loki query exceeds max {MAX_LOKI_QUERY_BYTES} bytes"
        ));
    }
    if query.contains(';') || query.contains("/*") {
        return Err("Loki query cannot contain statement separators or comments".to_string());
    }
    Ok(Some(query))
}

fn normalize_loki_response(
    response: &Value,
    warnings: &mut Vec<String>,
) -> Result<Vec<NormalizedStream>, String> {
    let result = response
        .pointer("/data/result")
        .or_else(|| response.get("result"))
        .and_then(Value::as_array)
        .ok_or_else(|| "Loki response requires data.result array".to_string())?;
    if result.len() > MAX_LOKI_STREAMS {
        return Err(format!("Loki streams exceeds max {MAX_LOKI_STREAMS}"));
    }
    result
        .iter()
        .map(|stream| {
            let labels = stream
                .get("stream")
                .and_then(Value::as_object)
                .ok_or_else(|| "Loki result stream requires stream labels".to_string())?;
            let values = stream
                .get("values")
                .and_then(Value::as_array)
                .ok_or_else(|| "Loki result stream requires values array".to_string())?;
            Ok(NormalizedStream {
                labels: normalize_labels(
                    labels
                        .iter()
                        .map(|(key, value)| (key.clone(), scalar_to_label(value)))
                        .collect(),
                    warnings,
                )?,
                entries: values
                    .iter()
                    .map(normalize_loki_value)
                    .collect::<Result<Vec<_>, _>>()?,
            })
        })
        .collect()
}

fn normalize_input_stream(
    stream: LokiStreamInput,
    warnings: &mut Vec<String>,
) -> Result<NormalizedStream, String> {
    let labels = match (stream.labels, stream.stream) {
        (Some(labels), _) if !labels.is_empty() => labels,
        (_, Some(stream)) => stream,
        (Some(labels), _) => labels,
        _ => BTreeMap::new(),
    };
    let mut entries = Vec::new();
    for entry in stream.entries.unwrap_or_default() {
        entries.push(NormalizedEntry {
            timestamp_ns: scalar_to_label(&entry.timestamp),
            line: entry.line,
        });
    }
    for value in stream.values.unwrap_or_default() {
        entries.push(normalize_loki_value(&value)?);
    }
    if entries.is_empty() {
        return Err("Loki stream requires at least one log entry".to_string());
    }
    Ok(NormalizedStream {
        labels: normalize_labels(labels, warnings)?,
        entries,
    })
}

fn normalize_loki_value(value: &Value) -> Result<NormalizedEntry, String> {
    let values = value
        .as_array()
        .ok_or_else(|| "Loki values must be [timestamp, line] arrays".to_string())?;
    if values.len() != 2 {
        return Err("Loki values must contain timestamp and line".to_string());
    }
    Ok(NormalizedEntry {
        timestamp_ns: scalar_to_label(&values[0]),
        line: scalar_to_label(&values[1]),
    })
}

fn normalize_labels(
    labels: BTreeMap<String, String>,
    warnings: &mut Vec<String>,
) -> Result<BTreeMap<String, String>, String> {
    if labels.len() > MAX_LOKI_LABELS {
        return Err(format!("Loki labels exceeds max {MAX_LOKI_LABELS}"));
    }
    let mut normalized = BTreeMap::new();
    for (key, value) in labels {
        let key = clean_identifier(&key).ok_or_else(|| format!("invalid Loki label `{key}`"))?;
        let value = value.trim().to_string();
        if value.len() > MAX_LOKI_LABEL_BYTES {
            return Err(format!(
                "Loki label `{key}` exceeds max {MAX_LOKI_LABEL_BYTES} bytes"
            ));
        }
        if looks_secret_bearing(&key) || looks_secret_bearing(&value) {
            warnings.push(format!("redacted secret-looking Loki label `{key}`"));
            normalized.insert(key, "[redacted]".to_string());
        } else {
            normalized.insert(key, value);
        }
    }
    Ok(normalized)
}

fn sanitize_line(line: &str, warnings: &mut Vec<String>) -> String {
    let mut line = line.replace(['\n', '\r'], " ");
    if line.len() > MAX_LOKI_LINE_BYTES {
        let boundary = byte_boundary(&line, MAX_LOKI_LINE_BYTES);
        line.truncate(boundary);
        warnings.push(format!("truncated log line to {MAX_LOKI_LINE_BYTES} bytes"));
    }
    let redacted = redact_secret_fragments(&line);
    if redacted != line {
        warnings.push("redacted secret-looking log line fragment".to_string());
    }
    redacted
}

fn redact_secret_fragments(line: &str) -> String {
    let mut output = Vec::new();
    let mut redact_next = false;
    for token in line.split_whitespace() {
        let lower = token.to_ascii_lowercase();
        if redact_next {
            output.push("[redacted]");
            redact_next = false;
            continue;
        }
        if lower == "bearer"
            || lower == "authorization"
            || lower == "authorization:"
            || lower.ends_with("=bearer")
            || lower.ends_with(":bearer")
        {
            output.push("[redacted]");
            redact_next = true;
        } else if lower.starts_with("password=")
            || lower.starts_with("token=")
            || lower.starts_with("secret=")
            || lower.starts_with("authorization=")
            || lower.starts_with("authorization:")
            || lower.starts_with("api_key=")
            || lower.starts_with("private_key=")
            || lower.starts_with("bearer ")
        {
            output.push("[redacted]");
        } else {
            output.push(token);
        }
    }
    output.join(" ")
}

fn byte_boundary(value: &str, max_bytes: usize) -> usize {
    if value.len() <= max_bytes {
        return value.len();
    }
    let mut boundary = max_bytes;
    while boundary > 0 && !value.is_char_boundary(boundary) {
        boundary -= 1;
    }
    boundary
}

fn infer_level(labels: &BTreeMap<String, String>, line: &str) -> String {
    for key in ["level", "severity"] {
        if let Some(level) = labels.get(key).map(|value| value.to_ascii_lowercase()) {
            if !level.is_empty() {
                return normalize_level(&level).to_string();
            }
        }
    }
    let lower = line.to_ascii_lowercase();
    if lower.contains("panic") || lower.contains("fatal") {
        "critical".to_string()
    } else if lower.contains("error") {
        "error".to_string()
    } else if lower.contains("warn") {
        "warning".to_string()
    } else if lower.contains("debug") {
        "debug".to_string()
    } else {
        "info".to_string()
    }
}

fn normalize_level(level: &str) -> &str {
    match level {
        "warn" => "warning",
        "err" => "error",
        "fatal" | "panic" => "critical",
        "trace" => "debug",
        other => other,
    }
}

fn timestamp_ns_to_ms(timestamp_ns: &str) -> Option<u128> {
    let trimmed = timestamp_ns.trim();
    if !trimmed.chars().all(|ch| ch.is_ascii_digit()) {
        return None;
    }
    let value = trimmed.parse::<u128>().ok()?;
    match trimmed.len() {
        0..=10 => Some(value * 1_000),
        11..=13 => Some(value),
        _ => Some(value / 1_000_000),
    }
}

fn compare_frame_rows(left: &LokiFrameRow, right: &LokiFrameRow) -> Ordering {
    match (
        timestamp_ns_to_ms(&left.timestamp_ns),
        timestamp_ns_to_ms(&right.timestamp_ns),
    ) {
        (Some(left_ms), Some(right_ms)) => left_ms
            .cmp(&right_ms)
            .then_with(|| left.stream_index.cmp(&right.stream_index)),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => left
            .timestamp_ns
            .cmp(&right.timestamp_ns)
            .then_with(|| left.stream_index.cmp(&right.stream_index)),
    }
}

fn label_keys(streams: &[NormalizedStream]) -> Vec<String> {
    let mut keys = BTreeMap::<String, ()>::new();
    for stream in streams {
        for key in stream.labels.keys() {
            keys.insert(key.clone(), ());
        }
    }
    keys.into_keys().collect()
}

fn looks_secret_bearing(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    [
        "secret",
        "token",
        "password",
        "authorization",
        "bearer",
        "api_key",
        "private_key",
        "access_key",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loki_response_becomes_redacted_log_frame() {
        let response = frame(LokiFrameRequest {
            query: Some("{job=\"api\"}".to_string()),
            streams: None,
            loki_response: Some(json!({
                "status": "success",
                "data": {
                    "result": [
                        {
                            "stream": { "job": "api", "level": "error" },
                            "values": [
                                ["1710000000000000000", "error request failed token=abc123"],
                                ["1710000001000000000", "warn slow request"]
                            ]
                        }
                    ]
                }
            })),
            max_entries: Some(10),
        })
        .expect("frame builds");

        assert_eq!(response.stream_count, 1);
        assert_eq!(response.entry_count, 2);
        assert_eq!(response.level_counts.get("error"), Some(&2));
        assert_eq!(response.label_keys, vec!["job", "level"]);
        assert!(response.frame.rows[0].line.contains("[redacted]"));
        assert_eq!(response.frame.rows[0].timestamp_ms, Some(1_710_000_000_000));
    }

    #[test]
    fn structured_stream_caps_entries() {
        let response = frame(LokiFrameRequest {
            query: None,
            streams: Some(vec![LokiStreamInput {
                labels: Some(BTreeMap::from([("job".to_string(), "worker".to_string())])),
                stream: None,
                values: None,
                entries: Some(vec![
                    LokiEntryInput {
                        timestamp: Value::from("1"),
                        line: "first".to_string(),
                    },
                    LokiEntryInput {
                        timestamp: Value::from("2"),
                        line: "second".to_string(),
                    },
                ]),
            }]),
            loki_response: None,
            max_entries: Some(1),
        })
        .expect("frame builds");

        assert_eq!(response.entry_count, 1);
        assert_eq!(response.dropped_entries, 1);
    }

    #[test]
    fn loki_frame_rejects_bad_values_shape() {
        let error = frame(LokiFrameRequest {
            query: None,
            streams: Some(vec![LokiStreamInput {
                labels: None,
                stream: None,
                values: Some(vec![json!(["1"])]),
                entries: None,
            }]),
            loki_response: None,
            max_entries: None,
        })
        .expect_err("bad shape rejected");

        assert!(error.contains("timestamp and line"));
    }

    #[test]
    fn loki_frame_redacts_bearer_sequences_and_truncates_utf8_safely() {
        let mut warnings = Vec::new();
        let line = format!("Bearer abc {}", "ø".repeat(MAX_LOKI_LINE_BYTES));
        let sanitized = sanitize_line(&line, &mut warnings);

        assert!(sanitized.starts_with("[redacted] [redacted]"));
        assert!(sanitized.is_char_boundary(sanitized.len()));
        assert!(!warnings.is_empty());
    }
}
