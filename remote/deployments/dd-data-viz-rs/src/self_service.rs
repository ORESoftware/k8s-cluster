use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::{AggregationOp, QueryDialect, QueryRequest};
use crate::util::{clean_field, clean_identifier, scalar_to_label};

const MAX_QUESTIONS: usize = 256;
const MAX_FIELDS: usize = 64;
const MAX_FILTERS: usize = 32;
const MAX_AGGREGATIONS: usize = 32;
const MAX_TAGS: usize = 24;
const MAX_CHART_ENCODINGS: usize = 16;
const MAX_QUESTION_LIMIT: usize = 5_000;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SaveQuestionRequest {
    pub question_id: String,
    pub title: String,
    pub description: Option<String>,
    pub dataset_id: String,
    pub owner: Option<String>,
    pub collection: Option<String>,
    pub tags: Option<Vec<String>>,
    pub query: QuestionBuilder,
    pub chart: Option<QuestionChartSpec>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SaveQuestionResponse {
    pub ok: bool,
    pub question: SavedQuestion,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct QuestionBuilder {
    pub fields: Option<Vec<String>>,
    pub filters: Option<Vec<QuestionFilter>>,
    pub group_by: Option<Vec<String>>,
    pub aggregations: Option<Vec<QuestionAggregation>>,
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct QuestionFilter {
    pub field: String,
    pub op: String,
    pub value: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct QuestionAggregation {
    pub alias: String,
    pub op: AggregationOp,
    pub field: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct QuestionChartSpec {
    pub chart_id: Option<String>,
    pub title: Option<String>,
    pub mark: String,
    pub encodings: Vec<QuestionChartEncoding>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct QuestionChartEncoding {
    pub channel: String,
    pub field: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SavedQuestion {
    pub question_id: String,
    pub title: String,
    pub description: Option<String>,
    pub dataset_id: String,
    pub owner: Option<String>,
    pub collection: Option<String>,
    pub tags: Vec<String>,
    pub query: QuestionBuilder,
    pub output_fields: Vec<QuestionOutputField>,
    pub compiled_sql: String,
    pub compiled_query: QueryRequest,
    pub chart: Option<SavedQuestionChart>,
    pub created_at_ms: u128,
    pub updated_at_ms: u128,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct QuestionOutputField {
    pub name: String,
    pub data_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SavedQuestionChart {
    pub chart_id: String,
    pub title: String,
    pub mark: String,
    pub encodings: Vec<QuestionChartEncoding>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct QuestionSummary {
    pub question_id: String,
    pub title: String,
    pub dataset_id: String,
    pub owner: Option<String>,
    pub collection: Option<String>,
    pub tag_count: usize,
    pub output_field_count: usize,
    pub has_chart: bool,
    pub updated_at_ms: u128,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ChartSummary {
    pub chart_id: String,
    pub question_id: String,
    pub title: String,
    pub dataset_id: String,
    pub mark: String,
    pub encoding_count: usize,
    pub owner: Option<String>,
    pub collection: Option<String>,
    pub updated_at_ms: u128,
}

impl SaveQuestionRequest {
    pub(crate) fn into_saved(
        self,
        now_ms: u128,
        available_fields: &BTreeMap<String, String>,
    ) -> Result<SavedQuestion, String> {
        let question_id = clean_identifier(&self.question_id).ok_or_else(|| {
            "questionId must contain letters, numbers, dash, underscore, dot, or colon".to_string()
        })?;
        let title = bounded_label("question title", &self.title, 160)?;
        let dataset_id = clean_identifier(&self.dataset_id).ok_or_else(|| {
            "datasetId must contain letters, numbers, dash, underscore, dot, or colon".to_string()
        })?;
        let owner = self
            .owner
            .as_deref()
            .map(|owner| bounded_label("question owner", owner, 120))
            .transpose()?;
        let collection = self
            .collection
            .as_deref()
            .map(|collection| bounded_label("question collection", collection, 120))
            .transpose()?;
        let tags = normalize_tags(self.tags.unwrap_or_default())?;
        let compiled = compile_builder(&dataset_id, self.query.clone(), available_fields)?;
        let chart = self
            .chart
            .map(|chart| chart.into_saved(&question_id, &compiled.output_field_names))
            .transpose()?;

        Ok(SavedQuestion {
            question_id,
            title,
            description: self.description.map(|value| value.trim().to_string()),
            dataset_id,
            owner,
            collection,
            tags,
            query: self.query,
            output_fields: compiled.output_fields,
            compiled_sql: compiled.sql.clone(),
            compiled_query: QueryRequest {
                dialect: QueryDialect::Sql,
                query: compiled.sql,
                dataset_id: Some(compiled.dataset_id),
                limit: Some(compiled.limit),
            },
            chart,
            created_at_ms: now_ms,
            updated_at_ms: now_ms,
        })
    }
}

impl SavedQuestion {
    pub(crate) fn summary(&self) -> QuestionSummary {
        QuestionSummary {
            question_id: self.question_id.clone(),
            title: self.title.clone(),
            dataset_id: self.dataset_id.clone(),
            owner: self.owner.clone(),
            collection: self.collection.clone(),
            tag_count: self.tags.len(),
            output_field_count: self.output_fields.len(),
            has_chart: self.chart.is_some(),
            updated_at_ms: self.updated_at_ms,
        }
    }

    pub(crate) fn chart_summary(&self) -> Option<ChartSummary> {
        let chart = self.chart.as_ref()?;
        Some(ChartSummary {
            chart_id: chart.chart_id.clone(),
            question_id: self.question_id.clone(),
            title: chart.title.clone(),
            dataset_id: self.dataset_id.clone(),
            mark: chart.mark.clone(),
            encoding_count: chart.encodings.len(),
            owner: self.owner.clone(),
            collection: self.collection.clone(),
            updated_at_ms: self.updated_at_ms,
        })
    }
}

impl QuestionChartSpec {
    fn into_saved(
        self,
        question_id: &str,
        output_fields: &BTreeSet<String>,
    ) -> Result<SavedQuestionChart, String> {
        let chart_id = self
            .chart_id
            .as_deref()
            .and_then(clean_identifier)
            .unwrap_or_else(|| format!("{question_id}:chart"));
        let title = self
            .title
            .as_deref()
            .map(|title| bounded_label("chart title", title, 160))
            .transpose()?
            .unwrap_or_else(|| format!("{question_id} chart"));
        let mark = clean_identifier(&self.mark)
            .ok_or_else(|| "chart mark must be a safe identifier".to_string())?;
        if !matches!(
            mark.as_str(),
            "bar" | "line" | "area" | "scatter" | "stem" | "histogram" | "box" | "violin"
                | "ecdf" | "map" | "choropleth" | "funnel" | "waterfall" | "treemap" | "sunburst"
                | "sankey" | "candlestick" | "bubble" | "gauge" | "density-contour"
                | "density-heatmap" | "contour" | "radar" | "polar-bar" | "ohlc" | "icicle"
                | "funnelarea" | "scatter3d" | "bullet" | "table" | "metric" | "pie" | "heatmap"
        ) {
            return Err(format!("unsupported chart mark `{mark}`"));
        }
        if self.encodings.is_empty() {
            return Err("chart requires at least one encoding".to_string());
        }
        if self.encodings.len() > MAX_CHART_ENCODINGS {
            return Err(format!("chart encodings exceeds max {MAX_CHART_ENCODINGS}"));
        }
        let mut seen_channels = BTreeSet::new();
        let mut encodings = Vec::new();
        for encoding in self.encodings {
            let channel = clean_identifier(&encoding.channel)
                .ok_or_else(|| "chart encoding channel is invalid".to_string())?;
            let field = clean_field(&encoding.field)
                .ok_or_else(|| "chart encoding field is invalid".to_string())?;
            if !output_fields.contains(&field) {
                return Err(format!(
                    "chart encoding `{channel}` references missing output field `{field}`"
                ));
            }
            if !seen_channels.insert(channel.clone()) {
                return Err(format!("duplicate chart encoding channel `{channel}`"));
            }
            encodings.push(QuestionChartEncoding { channel, field });
        }
        Ok(SavedQuestionChart {
            chart_id,
            title,
            mark,
            encodings,
        })
    }
}

pub(crate) fn save_response(
    question: SavedQuestion,
    warnings: Vec<String>,
) -> SaveQuestionResponse {
    SaveQuestionResponse {
        ok: true,
        question,
        warnings,
    }
}

pub(crate) fn question_catalog_payload(questions: Vec<QuestionSummary>) -> Value {
    json!({
        "ok": true,
        "schemaVersion": "data-viz.self-service-questions.v1",
        "questions": questions,
        "limits": limits_payload()
    })
}

pub(crate) fn chart_catalog_payload(charts: Vec<ChartSummary>) -> Value {
    json!({
        "ok": true,
        "schemaVersion": "data-viz.self-service-charts.v1",
        "charts": charts,
        "limits": limits_payload()
    })
}

pub(crate) fn max_questions() -> usize {
    MAX_QUESTIONS
}

fn compile_builder(
    dataset_id: &str,
    query: QuestionBuilder,
    available_fields: &BTreeMap<String, String>,
) -> Result<CompiledQuestion, String> {
    if available_fields.is_empty() {
        return Err("question dataset has no fields".to_string());
    }
    let fields = clean_field_vec(query.fields.unwrap_or_default(), "question fields")?;
    let group_by = clean_field_vec(query.group_by.unwrap_or_default(), "question groupBy")?;
    let filters = normalize_filters(query.filters.unwrap_or_default(), available_fields)?;
    let aggregations =
        normalize_aggregations(query.aggregations.unwrap_or_default(), available_fields)?;
    let limit = query.limit.unwrap_or(1_000).clamp(1, MAX_QUESTION_LIMIT);
    let is_aggregate = !group_by.is_empty() || !aggregations.is_empty();
    let output_fields = if is_aggregate {
        compile_aggregate_outputs(&group_by, &aggregations, available_fields)?
    } else {
        compile_projection_outputs(&fields, available_fields)?
    };
    let output_field_names = output_fields
        .iter()
        .map(|field| field.name.clone())
        .collect::<BTreeSet<_>>();
    let sql = compile_sql(
        dataset_id,
        &fields,
        &group_by,
        &filters,
        &aggregations,
        &output_fields,
        limit,
    )?;
    Ok(CompiledQuestion {
        dataset_id: dataset_id.to_string(),
        sql,
        limit,
        output_fields,
        output_field_names,
    })
}

fn compile_projection_outputs(
    fields: &[String],
    available_fields: &BTreeMap<String, String>,
) -> Result<Vec<QuestionOutputField>, String> {
    let selected = if fields.is_empty() {
        available_fields
            .keys()
            .take(MAX_FIELDS)
            .cloned()
            .collect::<Vec<_>>()
    } else {
        fields.to_vec()
    };
    if selected.len() > MAX_FIELDS {
        return Err(format!("question fields exceeds max {MAX_FIELDS}"));
    }
    selected
        .into_iter()
        .map(|field| output_field(&field, available_fields))
        .collect()
}

fn compile_aggregate_outputs(
    group_by: &[String],
    aggregations: &[QuestionAggregation],
    available_fields: &BTreeMap<String, String>,
) -> Result<Vec<QuestionOutputField>, String> {
    if aggregations.is_empty() {
        return Err("aggregate question requires at least one aggregation".to_string());
    }
    if group_by.len() > MAX_FIELDS {
        return Err(format!("question groupBy exceeds max {MAX_FIELDS}"));
    }
    let mut output = Vec::new();
    let mut seen = BTreeSet::new();
    for field in group_by {
        if !seen.insert(field.clone()) {
            return Err(format!("duplicate output field `{field}`"));
        }
        output.push(output_field(field, available_fields)?);
    }
    for aggregation in aggregations {
        if !seen.insert(aggregation.alias.clone()) {
            return Err(format!(
                "aggregation alias `{}` duplicates an output field",
                aggregation.alias
            ));
        }
        output.push(QuestionOutputField {
            name: aggregation.alias.clone(),
            data_type: "number".to_string(),
        });
    }
    Ok(output)
}

fn compile_sql(
    dataset_id: &str,
    fields: &[String],
    group_by: &[String],
    filters: &[QuestionFilter],
    aggregations: &[QuestionAggregation],
    output_fields: &[QuestionOutputField],
    limit: usize,
) -> Result<String, String> {
    let mut select_parts = Vec::new();
    if aggregations.is_empty() {
        if fields.is_empty() {
            select_parts.extend(output_fields.iter().map(|field| field.name.clone()));
        } else {
            select_parts.extend(fields.iter().cloned());
        }
    } else {
        select_parts.extend(group_by.iter().cloned());
        for aggregation in aggregations {
            select_parts.push(aggregation_sql(aggregation)?);
        }
    }
    if select_parts.is_empty() {
        return Err("question requires at least one field or aggregation".to_string());
    }
    let mut sql = format!("SELECT {} FROM {dataset_id}", select_parts.join(", "));
    if !filters.is_empty() {
        let predicates = filters
            .iter()
            .map(filter_sql)
            .collect::<Result<Vec<_>, _>>()?;
        sql.push_str(" WHERE ");
        sql.push_str(&predicates.join(" AND "));
    }
    if !group_by.is_empty() {
        sql.push_str(" GROUP BY ");
        sql.push_str(&group_by.join(", "));
    }
    sql.push_str(" LIMIT ");
    sql.push_str(&limit.to_string());
    Ok(sql)
}

fn normalize_filters(
    filters: Vec<QuestionFilter>,
    available_fields: &BTreeMap<String, String>,
) -> Result<Vec<QuestionFilter>, String> {
    if filters.len() > MAX_FILTERS {
        return Err(format!("question filters exceeds max {MAX_FILTERS}"));
    }
    filters
        .into_iter()
        .map(|filter| {
            let field = clean_field(&filter.field)
                .ok_or_else(|| "question filter field is invalid".to_string())?;
            ensure_field(&field, available_fields)?;
            let op = filter.op.trim().to_string();
            if !matches!(op.as_str(), "=" | "==" | "!=" | ">" | ">=" | "<" | "<=") {
                return Err(format!("unsupported question filter operator `{op}`"));
            }
            Ok(QuestionFilter {
                field,
                op,
                value: filter.value,
            })
        })
        .collect()
}

fn normalize_aggregations(
    aggregations: Vec<QuestionAggregation>,
    available_fields: &BTreeMap<String, String>,
) -> Result<Vec<QuestionAggregation>, String> {
    if aggregations.len() > MAX_AGGREGATIONS {
        return Err(format!(
            "question aggregations exceeds max {MAX_AGGREGATIONS}"
        ));
    }
    let mut seen = BTreeSet::new();
    let mut normalized = Vec::new();
    for aggregation in aggregations {
        let alias = clean_identifier(&aggregation.alias)
            .ok_or_else(|| "question aggregation alias is invalid".to_string())?;
        if !seen.insert(alias.clone()) {
            return Err(format!("duplicate question aggregation `{alias}`"));
        }
        let field = aggregation.field.as_deref().and_then(clean_field);
        if aggregation.op != AggregationOp::Count && field.is_none() {
            return Err(format!("question aggregation `{alias}` requires a field"));
        }
        if let Some(field) = &field {
            ensure_field(field, available_fields)?;
            if aggregation.op != AggregationOp::Count
                && available_fields.get(field).map(String::as_str) != Some("number")
            {
                return Err(format!(
                    "question aggregation `{alias}` requires numeric field `{field}`"
                ));
            }
        }
        normalized.push(QuestionAggregation {
            alias,
            op: aggregation.op,
            field,
        });
    }
    Ok(normalized)
}

fn clean_field_vec(fields: Vec<String>, label: &str) -> Result<Vec<String>, String> {
    if fields.len() > MAX_FIELDS {
        return Err(format!("{label} exceeds max {MAX_FIELDS}"));
    }
    let mut seen = BTreeSet::new();
    let mut clean = Vec::new();
    for field in fields {
        let field = clean_field(&field).ok_or_else(|| format!("{label} contains invalid field"))?;
        if !seen.insert(field.clone()) {
            return Err(format!("{label} contains duplicate field `{field}`"));
        }
        clean.push(field);
    }
    Ok(clean)
}

fn output_field(
    field: &str,
    available_fields: &BTreeMap<String, String>,
) -> Result<QuestionOutputField, String> {
    ensure_field(field, available_fields)?;
    Ok(QuestionOutputField {
        name: field.to_string(),
        data_type: available_fields
            .get(field)
            .cloned()
            .unwrap_or_else(|| "unknown".to_string()),
    })
}

fn ensure_field(field: &str, available_fields: &BTreeMap<String, String>) -> Result<(), String> {
    if available_fields.contains_key(field) {
        Ok(())
    } else {
        Err(format!("question references missing field `{field}`"))
    }
}

fn aggregation_sql(aggregation: &QuestionAggregation) -> Result<String, String> {
    let expr = match aggregation.op {
        AggregationOp::Count => aggregation
            .field
            .as_deref()
            .map(|field| format!("COUNT({field})"))
            .unwrap_or_else(|| "COUNT(*)".to_string()),
        AggregationOp::Sum => format!("SUM({})", required_aggregation_field(aggregation)?),
        AggregationOp::Avg => format!("AVG({})", required_aggregation_field(aggregation)?),
        AggregationOp::Min => format!("MIN({})", required_aggregation_field(aggregation)?),
        AggregationOp::Max => format!("MAX({})", required_aggregation_field(aggregation)?),
    };
    Ok(format!("{expr} AS {}", aggregation.alias))
}

fn filter_sql(filter: &QuestionFilter) -> Result<String, String> {
    let op = match filter.op.as_str() {
        "==" => "=",
        other => other,
    };
    Ok(format!(
        "{} {} {}",
        filter.field,
        op,
        sql_literal(&filter.value)?
    ))
}

fn sql_literal(value: &Value) -> Result<String, String> {
    match value {
        Value::Null => Ok("NULL".to_string()),
        Value::Bool(value) => Ok(value.to_string()),
        Value::Number(value) => Ok(value.to_string()),
        Value::String(value) => Ok(format!("'{}'", value.replace('\'', "''"))),
        Value::Array(_) | Value::Object(_) => Err(format!(
            "question filter value must be scalar, got {}",
            scalar_to_label(value)
        )),
    }
}

fn required_aggregation_field(aggregation: &QuestionAggregation) -> Result<&str, String> {
    aggregation.field.as_deref().ok_or_else(|| {
        format!(
            "question aggregation `{}` requires a field",
            aggregation.alias
        )
    })
}

fn bounded_label(label: &str, value: &str, max_len: usize) -> Result<String, String> {
    let value = value.trim().to_string();
    if value.is_empty() || value.len() > max_len {
        Err(format!("{label} must be 1-{max_len} characters"))
    } else {
        Ok(value)
    }
}

fn normalize_tags(tags: Vec<String>) -> Result<Vec<String>, String> {
    if tags.len() > MAX_TAGS {
        return Err(format!("question tags exceeds max {MAX_TAGS}"));
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

fn limits_payload() -> Value {
    json!({
        "maxQuestions": MAX_QUESTIONS,
        "maxFields": MAX_FIELDS,
        "maxFilters": MAX_FILTERS,
        "maxAggregations": MAX_AGGREGATIONS,
        "maxTags": MAX_TAGS,
        "maxChartEncodings": MAX_CHART_ENCODINGS,
        "maxLimit": MAX_QUESTION_LIMIT
    })
}

struct CompiledQuestion {
    dataset_id: String,
    sql: String,
    limit: usize,
    output_fields: Vec<QuestionOutputField>,
    output_field_names: BTreeSet<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fields() -> BTreeMap<String, String> {
        BTreeMap::from([
            ("region".to_string(), "category".to_string()),
            ("segment".to_string(), "category".to_string()),
            ("revenue".to_string(), "number".to_string()),
        ])
    }

    #[test]
    fn question_validates_and_compiles_chart_binding() {
        let saved = SaveQuestionRequest {
            question_id: "revenue-by-region".to_string(),
            title: "Revenue by region".to_string(),
            description: None,
            dataset_id: "sales".to_string(),
            owner: Some("analytics".to_string()),
            collection: Some("exec".to_string()),
            tags: Some(vec!["Sales".to_string(), "sales".to_string()]),
            query: QuestionBuilder {
                fields: None,
                filters: Some(vec![QuestionFilter {
                    field: "segment".to_string(),
                    op: "!=".to_string(),
                    value: Value::from("internal"),
                }]),
                group_by: Some(vec!["region".to_string()]),
                aggregations: Some(vec![QuestionAggregation {
                    alias: "total_revenue".to_string(),
                    op: AggregationOp::Sum,
                    field: Some("revenue".to_string()),
                }]),
                limit: Some(25),
            },
            chart: Some(QuestionChartSpec {
                chart_id: None,
                title: Some("Revenue chart".to_string()),
                mark: "bar".to_string(),
                encodings: vec![
                    QuestionChartEncoding {
                        channel: "x".to_string(),
                        field: "region".to_string(),
                    },
                    QuestionChartEncoding {
                        channel: "y".to_string(),
                        field: "total_revenue".to_string(),
                    },
                ],
            }),
        }
        .into_saved(100, &fields())
        .expect("question validates");

        assert_eq!(saved.tags, vec!["sales"]);
        assert!(saved.compiled_sql.contains("SUM(revenue) AS total_revenue"));
        assert_eq!(saved.output_fields.len(), 2);
        assert_eq!(
            saved.chart_summary().expect("chart summary").chart_id,
            "revenue-by-region:chart"
        );
    }

    #[test]
    fn question_accepts_stem_mark() {
        let saved = SaveQuestionRequest {
            question_id: "revenue-by-region".to_string(),
            title: "Revenue by region".to_string(),
            description: None,
            dataset_id: "sales".to_string(),
            owner: None,
            collection: None,
            tags: None,
            query: QuestionBuilder {
                fields: None,
                filters: None,
                group_by: Some(vec!["region".to_string()]),
                aggregations: Some(vec![QuestionAggregation {
                    alias: "total_revenue".to_string(),
                    op: AggregationOp::Sum,
                    field: Some("revenue".to_string()),
                }]),
                limit: Some(25),
            },
            chart: Some(QuestionChartSpec {
                chart_id: None,
                title: Some("Revenue stem".to_string()),
                mark: "stem".to_string(),
                encodings: vec![
                    QuestionChartEncoding {
                        channel: "x".to_string(),
                        field: "region".to_string(),
                    },
                    QuestionChartEncoding {
                        channel: "y".to_string(),
                        field: "total_revenue".to_string(),
                    },
                ],
            }),
        }
        .into_saved(101, &fields())
        .expect("stem chart validates");

        assert_eq!(saved.chart_summary().expect("chart summary").mark, "stem");
    }

    #[test]
    fn chart_accepts_statistical_marks() {
        for mark in [
            "histogram", "box", "violin", "ecdf", "map", "choropleth", "funnel", "waterfall",
            "treemap", "sunburst", "sankey", "candlestick", "bubble", "gauge",
        ] {
            let chart = QuestionChartSpec {
                chart_id: None,
                title: Some(format!("{mark} chart")),
                mark: mark.to_string(),
                encodings: vec![QuestionChartEncoding {
                    channel: "x".to_string(),
                    field: "revenue".to_string(),
                }],
            };
            let output_fields: BTreeSet<String> = ["revenue".to_string()].into_iter().collect();
            chart
                .into_saved("dist", &output_fields)
                .unwrap_or_else(|err| panic!("{mark} mark should validate: {err}"));
        }
    }

    #[test]
    fn question_rejects_missing_chart_fields() {
        let error = SaveQuestionRequest {
            question_id: "bad".to_string(),
            title: "Bad chart".to_string(),
            description: None,
            dataset_id: "sales".to_string(),
            owner: None,
            collection: None,
            tags: None,
            query: QuestionBuilder {
                fields: Some(vec!["region".to_string()]),
                filters: None,
                group_by: None,
                aggregations: None,
                limit: None,
            },
            chart: Some(QuestionChartSpec {
                chart_id: None,
                title: None,
                mark: "bar".to_string(),
                encodings: vec![QuestionChartEncoding {
                    channel: "x".to_string(),
                    field: "missing".to_string(),
                }],
            }),
        }
        .into_saved(100, &fields())
        .expect_err("missing chart field rejected");

        assert!(error.contains("missing output field"));
    }
}
