use std::collections::BTreeSet;
use std::hash::{Hash, Hasher};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::util::{clean_identifier, now_ms};

const MAX_SQL_HISTORY: usize = 256;
const MAX_SQL_BYTES: usize = 16 * 1024;
const MAX_TITLE_BYTES: usize = 160;
const MAX_NOTE_BYTES: usize = 1_024;
const MAX_TAGS: usize = 24;
const MAX_LAB_LIMIT: usize = 5_000;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SaveSqlLabRequest {
    pub history_id: Option<String>,
    pub title: Option<String>,
    pub dataset_id: Option<String>,
    pub connection_id: Option<String>,
    pub query: String,
    pub limit: Option<usize>,
    pub tags: Option<Vec<String>>,
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SqlLabHistoryEntry {
    pub history_id: String,
    pub title: String,
    pub dataset_id: Option<String>,
    pub connection_id: Option<String>,
    pub query: String,
    pub query_hash: String,
    pub limit: usize,
    pub tags: Vec<String>,
    pub notes: Option<String>,
    pub status: SqlLabStatus,
    pub row_count: Option<usize>,
    pub logical_plan: Option<Value>,
    pub error: Option<String>,
    pub warnings: Vec<String>,
    pub duration_ms: Option<u128>,
    pub created_at_ms: u128,
    pub updated_at_ms: u128,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum SqlLabStatus {
    Pending,
    Succeeded,
    Failed,
    PlannedExternal,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SqlLabHistorySummary {
    history_id: String,
    title: String,
    dataset_id: Option<String>,
    connection_id: Option<String>,
    query_hash: String,
    limit: usize,
    tag_count: usize,
    status: SqlLabStatus,
    row_count: Option<usize>,
    updated_at_ms: u128,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SaveSqlLabResponse {
    ok: bool,
    history: SqlLabHistoryEntry,
}

impl SaveSqlLabRequest {
    pub(crate) fn into_history_entry(self, now_ms: u128) -> Result<SqlLabHistoryEntry, String> {
        let history_id = self
            .history_id
            .as_deref()
            .map(|value| {
                clean_identifier(value)
                    .ok_or_else(|| "historyId must be a safe identifier".to_string())
            })
            .transpose()?
            .unwrap_or_else(|| format!("sql-lab-{now_ms}"));
        let dataset_id = self
            .dataset_id
            .as_deref()
            .map(|value| {
                clean_identifier(value)
                    .ok_or_else(|| "datasetId must be a safe identifier".to_string())
            })
            .transpose()?;
        let connection_id = self
            .connection_id
            .as_deref()
            .map(|value| {
                clean_identifier(value)
                    .ok_or_else(|| "connectionId must be a safe identifier".to_string())
            })
            .transpose()?;
        if dataset_id.is_some() && connection_id.is_some() {
            return Err(
                "SQL Lab requests must target either datasetId or connectionId, not both"
                    .to_string(),
            );
        }
        let query = normalize_sql(&self.query)?;
        let title = self
            .title
            .as_deref()
            .map(|value| bounded_label("SQL Lab title", value, MAX_TITLE_BYTES))
            .transpose()?
            .unwrap_or_else(|| inferred_title(&query));
        let limit = self.limit.unwrap_or(100).clamp(1, MAX_LAB_LIMIT);
        let tags = normalize_tags(self.tags.unwrap_or_default())?;
        let notes = self
            .notes
            .as_deref()
            .map(|value| bounded_label("SQL Lab notes", value, MAX_NOTE_BYTES))
            .transpose()?;
        Ok(SqlLabHistoryEntry {
            history_id,
            title,
            dataset_id,
            connection_id,
            query_hash: query_hash(&query),
            query,
            limit,
            tags,
            notes,
            status: SqlLabStatus::Pending,
            row_count: None,
            logical_plan: None,
            error: None,
            warnings: Vec::new(),
            duration_ms: None,
            created_at_ms: now_ms,
            updated_at_ms: now_ms,
        })
    }
}

impl SqlLabHistoryEntry {
    pub(crate) fn summary(&self) -> SqlLabHistorySummary {
        SqlLabHistorySummary {
            history_id: self.history_id.clone(),
            title: self.title.clone(),
            dataset_id: self.dataset_id.clone(),
            connection_id: self.connection_id.clone(),
            query_hash: self.query_hash.clone(),
            limit: self.limit,
            tag_count: self.tags.len(),
            status: self.status,
            row_count: self.row_count,
            updated_at_ms: self.updated_at_ms,
        }
    }

    pub(crate) fn mark_succeeded(
        &mut self,
        dataset_id: String,
        row_count: usize,
        logical_plan: Value,
        duration_ms: u128,
        warnings: Vec<String>,
    ) {
        self.dataset_id = Some(dataset_id);
        self.status = SqlLabStatus::Succeeded;
        self.row_count = Some(row_count);
        self.logical_plan = Some(logical_plan);
        self.error = None;
        self.duration_ms = Some(duration_ms);
        self.warnings = warnings;
        self.updated_at_ms = now_ms();
    }

    pub(crate) fn mark_failed(&mut self, message: String, duration_ms: u128) {
        self.status = SqlLabStatus::Failed;
        self.row_count = None;
        self.logical_plan = None;
        self.error = Some(bounded_error(message));
        self.duration_ms = Some(duration_ms);
        self.updated_at_ms = now_ms();
    }

    pub(crate) fn mark_planned_external(&mut self, dialect: &str) {
        self.status = SqlLabStatus::PlannedExternal;
        self.row_count = None;
        self.logical_plan = Some(json!({
            "schemaVersion": "data-viz.sql-lab.external-plan.v1",
            "connectionId": self.connection_id,
            "dialectTarget": dialect,
            "queryHash": self.query_hash,
            "dryRunOnly": true
        }));
        self.error = None;
        self.warnings = vec![
            "external connection SQL Lab entries are stored as dry-run plans only".to_string(),
            "connector workers must resolve secretRef and execute outside this service".to_string(),
        ];
        self.duration_ms = Some(0);
        self.updated_at_ms = now_ms();
    }
}

pub(crate) fn save_response(history: SqlLabHistoryEntry) -> SaveSqlLabResponse {
    SaveSqlLabResponse { ok: true, history }
}

pub(crate) fn catalog_payload(history: Vec<SqlLabHistorySummary>) -> Value {
    json!({
        "ok": true,
        "schemaVersion": "data-viz.sql-lab-history.v1",
        "history": history,
        "limits": limits_payload()
    })
}

pub(crate) fn max_sql_history() -> usize {
    MAX_SQL_HISTORY
}

pub(crate) fn limits_payload() -> Value {
    json!({
        "maxHistory": MAX_SQL_HISTORY,
        "maxSqlBytes": MAX_SQL_BYTES,
        "maxTitleBytes": MAX_TITLE_BYTES,
        "maxNoteBytes": MAX_NOTE_BYTES,
        "maxTags": MAX_TAGS,
        "maxLimit": MAX_LAB_LIMIT
    })
}

fn normalize_sql(query: &str) -> Result<String, String> {
    let query = query.trim();
    if query.is_empty() || query.len() > MAX_SQL_BYTES {
        return Err(format!("SQL Lab query must be 1-{MAX_SQL_BYTES} bytes"));
    }
    if query.contains('\0') {
        return Err("SQL Lab query must not contain NUL bytes".to_string());
    }
    if query.contains("--") || query.contains("/*") || query.contains("*/") {
        return Err("SQL Lab query comments are not accepted in stored history".to_string());
    }
    let query = query.trim_end_matches(';').trim();
    if query.contains(';') {
        return Err("SQL Lab accepts one SELECT statement at a time".to_string());
    }
    if !query.to_ascii_lowercase().starts_with("select ") {
        return Err("SQL Lab currently stores SELECT queries only".to_string());
    }
    let tokens = query_tokens(query);
    for keyword in [
        "insert", "update", "delete", "drop", "alter", "create", "grant", "revoke", "truncate",
        "copy", "load", "attach", "detach", "vacuum",
    ] {
        if tokens.iter().any(|token| token == keyword) {
            return Err(format!("SQL Lab rejects `{keyword}` statements"));
        }
    }
    for marker in [
        "password",
        "passwd",
        "secret",
        "token",
        "private_key",
        "credential",
        "api_key",
        "access_key",
    ] {
        if tokens.iter().any(|token| token.contains(marker)) {
            return Err("SQL Lab query looks secret-bearing; do not store credentials".to_string());
        }
    }
    Ok(query.to_string())
}

fn query_tokens(query: &str) -> Vec<String> {
    query
        .split(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_')
        .filter(|part| !part.is_empty())
        .map(|part| part.to_ascii_lowercase())
        .collect()
}

fn inferred_title(query: &str) -> String {
    query
        .split_whitespace()
        .take(8)
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(MAX_TITLE_BYTES)
        .collect()
}

fn query_hash(query: &str) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    query.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

fn normalize_tags(tags: Vec<String>) -> Result<Vec<String>, String> {
    if tags.len() > MAX_TAGS {
        return Err(format!("SQL Lab tags exceeds max {MAX_TAGS}"));
    }
    let mut tags = tags
        .into_iter()
        .filter_map(|tag| clean_identifier(&tag.to_ascii_lowercase()))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    tags.sort();
    Ok(tags)
}

fn bounded_label(label: &str, value: &str, max_len: usize) -> Result<String, String> {
    let value = value.trim().to_string();
    if value.is_empty() || value.len() > max_len {
        Err(format!("{label} must be 1-{max_len} characters"))
    } else {
        Ok(value)
    }
}

fn bounded_error(message: String) -> String {
    message.chars().take(512).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sql_lab_request_rejects_mutating_sql() {
        let error = SaveSqlLabRequest {
            history_id: None,
            title: None,
            dataset_id: Some("sales-lab".to_string()),
            connection_id: None,
            query: "DELETE FROM sales-lab".to_string(),
            limit: None,
            tags: None,
            notes: None,
        }
        .into_history_entry(100)
        .expect_err("mutating SQL rejected");

        assert!(error.contains("SELECT"));
    }

    #[test]
    fn sql_lab_request_rejects_secret_like_tokens() {
        let error = SaveSqlLabRequest {
            history_id: None,
            title: None,
            dataset_id: Some("sales-lab".to_string()),
            connection_id: None,
            query: "SELECT api_key FROM sales-lab".to_string(),
            limit: None,
            tags: None,
            notes: None,
        }
        .into_history_entry(100)
        .expect_err("secret-looking SQL rejected");

        assert!(error.contains("secret-bearing"));
    }

    #[test]
    fn sql_lab_summary_omits_raw_query_text() {
        let mut entry = SaveSqlLabRequest {
            history_id: Some("lab-1".to_string()),
            title: Some("Revenue by region".to_string()),
            dataset_id: Some("sales-lab".to_string()),
            connection_id: None,
            query: "SELECT region, SUM(revenue) FROM sales-lab GROUP BY region".to_string(),
            limit: Some(25),
            tags: Some(vec!["finance".to_string()]),
            notes: None,
        }
        .into_history_entry(100)
        .expect("query validates");

        entry.mark_succeeded(
            "sales-lab".to_string(),
            3,
            json!({"source": "sales-lab"}),
            4,
            vec![],
        );
        let summary = serde_json::to_value(entry.summary()).expect("summary serializes");
        assert_eq!(summary["historyId"], "lab-1");
        assert_eq!(summary["rowCount"], 3);
        assert!(summary.get("query").is_none());
        assert_ne!(entry.query_hash, "");
    }

    #[test]
    fn sql_lab_external_entries_are_dry_run_only() {
        let mut entry = SaveSqlLabRequest {
            history_id: Some("external-1".to_string()),
            title: None,
            dataset_id: None,
            connection_id: Some("bigquery-prod".to_string()),
            query: "SELECT region, revenue FROM analytics.sales".to_string(),
            limit: None,
            tags: None,
            notes: None,
        }
        .into_history_entry(100)
        .expect("query validates");

        entry.mark_planned_external("bigquery-standard-sql");
        assert_eq!(entry.status, SqlLabStatus::PlannedExternal);
        assert!(entry
            .warnings
            .iter()
            .any(|warning| warning.contains("dry-run")));
    }
}
