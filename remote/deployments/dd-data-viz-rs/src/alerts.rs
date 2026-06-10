use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::{
    util::{clean_identifier, now_ms, round4, scalar_to_label},
    QueryRequest,
};

const MAX_ALERT_RULES: usize = 256;
const MAX_LABELS: usize = 32;
const MAX_ANNOTATIONS: usize = 32;
const MAX_ALERT_QUERY_BYTES: usize = 8 * 1024;
const MAX_FOR_SECONDS: u64 = 7 * 24 * 60 * 60;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SaveAlertRuleRequest {
    pub rule_id: String,
    pub title: String,
    pub query: QueryRequest,
    pub condition: AlertCondition,
    pub for_seconds: Option<u64>,
    pub labels: Option<BTreeMap<String, String>>,
    pub annotations: Option<BTreeMap<String, String>>,
    pub dashboard_id: Option<String>,
    pub panel_id: Option<String>,
    pub enabled: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AlertCondition {
    pub field: String,
    pub reducer: AlertReducer,
    pub op: AlertOperator,
    pub threshold: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum AlertReducer {
    First,
    Last,
    Min,
    Max,
    Avg,
    Sum,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum AlertOperator {
    Gt,
    Gte,
    Lt,
    Lte,
    Eq,
    Ne,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum AlertState {
    Normal,
    Alerting,
    NoData,
    Error,
    Disabled,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AlertRule {
    pub rule_id: String,
    pub title: String,
    pub query: QueryRequest,
    pub condition: AlertCondition,
    pub for_seconds: u64,
    pub labels: BTreeMap<String, String>,
    pub annotations: BTreeMap<String, String>,
    pub dashboard_id: Option<String>,
    pub panel_id: Option<String>,
    pub enabled: bool,
    pub created_at_ms: u128,
    pub updated_at_ms: u128,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AlertRuleSummary {
    rule_id: String,
    title: String,
    dataset_id: Option<String>,
    field: String,
    reducer: AlertReducer,
    op: AlertOperator,
    threshold: f64,
    enabled: bool,
    label_count: usize,
    updated_at_ms: u128,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SaveAlertRuleResponse {
    ok: bool,
    rule: AlertRule,
    warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AlertEvaluationResponse {
    ok: bool,
    rule_id: String,
    title: String,
    state: AlertState,
    observed_value: Option<f64>,
    condition: AlertCondition,
    row_count: usize,
    evaluated_at_ms: u128,
    labels: BTreeMap<String, String>,
    annotations: BTreeMap<String, String>,
    evidence: Vec<String>,
}

impl SaveAlertRuleRequest {
    pub(crate) fn into_rule(self, now_ms: u128) -> Result<AlertRule, String> {
        let rule_id = clean_identifier(&self.rule_id).ok_or_else(|| {
            "ruleId must contain letters, numbers, dash, underscore, dot, or colon".to_string()
        })?;
        let title = self.title.trim().to_string();
        if title.is_empty() || title.len() > 160 {
            return Err("alert title must be 1-160 characters".to_string());
        }
        if self.query.query.trim().is_empty() || self.query.query.len() > MAX_ALERT_QUERY_BYTES {
            return Err(format!(
                "alert query must be 1-{MAX_ALERT_QUERY_BYTES} bytes"
            ));
        }
        let condition = self.condition.normalized()?;
        let for_seconds = self.for_seconds.unwrap_or(0).min(MAX_FOR_SECONDS);
        let labels = normalize_map(self.labels.unwrap_or_default(), "label")?;
        let annotations = normalize_map(self.annotations.unwrap_or_default(), "annotation")?;
        let dashboard_id = normalize_optional_identifier(self.dashboard_id, "dashboardId")?;
        let panel_id = normalize_optional_identifier(self.panel_id, "panelId")?;

        Ok(AlertRule {
            rule_id,
            title,
            query: self.query,
            condition,
            for_seconds,
            labels,
            annotations,
            dashboard_id,
            panel_id,
            enabled: self.enabled.unwrap_or(true),
            created_at_ms: now_ms,
            updated_at_ms: now_ms,
        })
    }
}

impl AlertCondition {
    fn normalized(self) -> Result<Self, String> {
        let field = clean_identifier(&self.field)
            .ok_or_else(|| "alert condition field is invalid".to_string())?;
        if !self.threshold.is_finite() {
            return Err("alert threshold must be finite".to_string());
        }
        Ok(Self {
            field,
            reducer: self.reducer,
            op: self.op,
            threshold: self.threshold,
        })
    }
}

impl AlertRule {
    pub(crate) fn summary(&self) -> AlertRuleSummary {
        AlertRuleSummary {
            rule_id: self.rule_id.clone(),
            title: self.title.clone(),
            dataset_id: self.query.dataset_id.clone(),
            field: self.condition.field.clone(),
            reducer: self.condition.reducer,
            op: self.condition.op,
            threshold: self.condition.threshold,
            enabled: self.enabled,
            label_count: self.labels.len(),
            updated_at_ms: self.updated_at_ms,
        }
    }
}

impl AlertOperator {
    fn matches(self, observed: f64, threshold: f64) -> bool {
        match self {
            Self::Gt => observed > threshold,
            Self::Gte => observed >= threshold,
            Self::Lt => observed < threshold,
            Self::Lte => observed <= threshold,
            Self::Eq => (observed - threshold).abs() < f64::EPSILON,
            Self::Ne => (observed - threshold).abs() >= f64::EPSILON,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Gt => ">",
            Self::Gte => ">=",
            Self::Lt => "<",
            Self::Lte => "<=",
            Self::Eq => "==",
            Self::Ne => "!=",
        }
    }
}

pub(crate) fn catalog_payload(rules: Vec<AlertRuleSummary>) -> Value {
    json!({
        "ok": true,
        "schemaVersion": "data-viz.alert-rules.v1",
        "rules": rules,
        "limits": {
            "maxAlertRules": MAX_ALERT_RULES,
            "maxLabels": MAX_LABELS,
            "maxAnnotations": MAX_ANNOTATIONS,
            "maxAlertQueryBytes": MAX_ALERT_QUERY_BYTES,
            "maxForSeconds": MAX_FOR_SECONDS
        }
    })
}

pub(crate) fn save_response(rule: AlertRule, warnings: Vec<String>) -> SaveAlertRuleResponse {
    SaveAlertRuleResponse {
        ok: true,
        rule,
        warnings,
    }
}

pub(crate) fn max_alert_rules() -> usize {
    MAX_ALERT_RULES
}

pub(crate) fn evaluate_rule(
    rule: &AlertRule,
    rows: &[BTreeMap<String, Value>],
) -> AlertEvaluationResponse {
    if !rule.enabled {
        return AlertEvaluationResponse {
            ok: true,
            rule_id: rule.rule_id.clone(),
            title: rule.title.clone(),
            state: AlertState::Disabled,
            observed_value: None,
            condition: rule.condition.clone(),
            row_count: rows.len(),
            evaluated_at_ms: now_ms(),
            labels: rule.labels.clone(),
            annotations: rule.annotations.clone(),
            evidence: vec!["alert rule is disabled".to_string()],
        };
    }

    let mut values = Vec::new();
    let mut non_numeric_examples = Vec::new();
    for row in rows {
        match row.get(&rule.condition.field).and_then(numeric_value) {
            Some(value) if value.is_finite() => values.push(value),
            _ => {
                if let Some(raw) = row.get(&rule.condition.field) {
                    non_numeric_examples.push(scalar_to_label(raw));
                }
            }
        }
    }

    if values.is_empty() {
        let mut evidence = vec![format!(
            "field `{}` produced no numeric values across {} rows",
            rule.condition.field,
            rows.len()
        )];
        if !non_numeric_examples.is_empty() {
            evidence.push(format!(
                "sample non-numeric values: {}",
                non_numeric_examples
                    .into_iter()
                    .take(3)
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        return AlertEvaluationResponse {
            ok: true,
            rule_id: rule.rule_id.clone(),
            title: rule.title.clone(),
            state: if rows.is_empty() {
                AlertState::NoData
            } else {
                AlertState::Error
            },
            observed_value: None,
            condition: rule.condition.clone(),
            row_count: rows.len(),
            evaluated_at_ms: now_ms(),
            labels: rule.labels.clone(),
            annotations: rule.annotations.clone(),
            evidence,
        };
    }

    let observed = reduce(rule.condition.reducer, &values);
    let alerting = rule
        .condition
        .op
        .matches(observed, rule.condition.threshold);
    AlertEvaluationResponse {
        ok: true,
        rule_id: rule.rule_id.clone(),
        title: rule.title.clone(),
        state: if alerting {
            AlertState::Alerting
        } else {
            AlertState::Normal
        },
        observed_value: Some(round4(observed)),
        condition: rule.condition.clone(),
        row_count: rows.len(),
        evaluated_at_ms: now_ms(),
        labels: rule.labels.clone(),
        annotations: rule.annotations.clone(),
        evidence: vec![format!(
            "{}({}) observed {} {} threshold {}",
            reducer_label(rule.condition.reducer),
            rule.condition.field,
            round4(observed),
            rule.condition.op.label(),
            rule.condition.threshold
        )],
    }
}

fn normalize_map(
    values: BTreeMap<String, String>,
    kind: &'static str,
) -> Result<BTreeMap<String, String>, String> {
    let max_items = if kind == "label" {
        MAX_LABELS
    } else {
        MAX_ANNOTATIONS
    };
    if values.len() > max_items {
        return Err(format!("{kind}s exceeds max {max_items}"));
    }
    let mut normalized = BTreeMap::new();
    for (key, value) in values {
        let key = clean_identifier(&key).ok_or_else(|| format!("{kind} key is invalid"))?;
        let value = value.trim().to_string();
        if value.len() > 256 {
            return Err(format!("{kind} value exceeds 256 characters"));
        }
        normalized.insert(key, value);
    }
    Ok(normalized)
}

fn normalize_optional_identifier(
    value: Option<String>,
    field: &'static str,
) -> Result<Option<String>, String> {
    value
        .map(|value| clean_identifier(&value).ok_or_else(|| format!("{field} is invalid")))
        .transpose()
}

fn numeric_value(value: &Value) -> Option<f64> {
    match value {
        Value::Number(number) => number.as_f64(),
        Value::String(value) => value.parse::<f64>().ok(),
        Value::Bool(value) => Some(if *value { 1.0 } else { 0.0 }),
        _ => None,
    }
}

fn reduce(reducer: AlertReducer, values: &[f64]) -> f64 {
    match reducer {
        AlertReducer::First => values.first().copied().unwrap_or(0.0),
        AlertReducer::Last => values.last().copied().unwrap_or(0.0),
        AlertReducer::Min => values.iter().copied().fold(f64::INFINITY, f64::min),
        AlertReducer::Max => values.iter().copied().fold(f64::NEG_INFINITY, f64::max),
        AlertReducer::Avg => values.iter().sum::<f64>() / values.len().max(1) as f64,
        AlertReducer::Sum => values.iter().sum(),
    }
}

fn reducer_label(reducer: AlertReducer) -> &'static str {
    match reducer {
        AlertReducer::First => "first",
        AlertReducer::Last => "last",
        AlertReducer::Min => "min",
        AlertReducer::Max => "max",
        AlertReducer::Avg => "avg",
        AlertReducer::Sum => "sum",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::QueryDialect;

    fn row(field: &str, value: f64) -> BTreeMap<String, Value> {
        BTreeMap::from([(field.to_string(), Value::from(value))])
    }

    fn rule(threshold: f64) -> AlertRule {
        SaveAlertRuleRequest {
            rule_id: "high-revenue".to_string(),
            title: "High revenue".to_string(),
            query: QueryRequest {
                dialect: QueryDialect::Sql,
                query: "SELECT SUM(revenue) AS totalRevenue FROM sales".to_string(),
                dataset_id: Some("sales".to_string()),
                limit: Some(10),
            },
            condition: AlertCondition {
                field: "totalRevenue".to_string(),
                reducer: AlertReducer::Max,
                op: AlertOperator::Gt,
                threshold,
            },
            for_seconds: Some(60),
            labels: Some(BTreeMap::from([(
                "severity".to_string(),
                "warning".to_string(),
            )])),
            annotations: None,
            dashboard_id: Some("exec-sales".to_string()),
            panel_id: Some("revenue".to_string()),
            enabled: Some(true),
        }
        .into_rule(100)
        .expect("rule validates")
    }

    #[test]
    fn alert_rule_validation_normalizes_metadata() {
        let rule = rule(1000.0);
        assert_eq!(rule.rule_id, "high-revenue");
        assert_eq!(rule.condition.field, "totalRevenue");
        assert_eq!(
            rule.labels.get("severity").map(String::as_str),
            Some("warning")
        );
        assert_eq!(rule.for_seconds, 60);
    }

    #[test]
    fn alert_evaluation_reports_alerting_and_normal_states() {
        let high = rule(1000.0);
        let alerting = evaluate_rule(&high, &[row("totalRevenue", 1200.0)]);
        assert_eq!(alerting.state, AlertState::Alerting);
        assert_eq!(alerting.observed_value, Some(1200.0));

        let normal = evaluate_rule(&rule(2000.0), &[row("totalRevenue", 1200.0)]);
        assert_eq!(normal.state, AlertState::Normal);
    }

    #[test]
    fn alert_evaluation_handles_no_data() {
        let response = evaluate_rule(&rule(10.0), &[]);
        assert_eq!(response.state, AlertState::NoData);
        assert_eq!(response.observed_value, None);
    }
}
