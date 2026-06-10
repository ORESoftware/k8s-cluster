use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::{
    util::{clean_field, clean_identifier},
    QueryDialect, QueryRequest,
};

const MAX_SEMANTIC_MODELS: usize = 128;
const MAX_DIMENSIONS: usize = 128;
const MAX_MEASURES: usize = 128;
const MAX_TAGS: usize = 24;
const MAX_LOOKML_BYTES: usize = 32 * 1024;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SaveSemanticModelRequest {
    pub model_id: String,
    pub name: String,
    pub dataset_id: String,
    pub owner: Option<String>,
    pub tags: Option<Vec<String>>,
    pub lookml: Option<String>,
    pub dimensions: Option<Vec<SemanticDimension>>,
    pub measures: Option<Vec<SemanticMeasure>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SemanticDimension {
    pub name: String,
    pub field: String,
    pub data_type: Option<String>,
    pub label: Option<String>,
    pub primary_key: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SemanticMeasure {
    pub name: String,
    pub field: Option<String>,
    pub aggregation: SemanticAggregation,
    pub label: Option<String>,
    pub dax_analog: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum SemanticAggregation {
    Count,
    Sum,
    Avg,
    Min,
    Max,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SavedSemanticModel {
    pub model_id: String,
    pub name: String,
    pub dataset_id: String,
    pub owner: Option<String>,
    pub tags: Vec<String>,
    pub dimensions: Vec<SemanticDimension>,
    pub measures: Vec<SemanticMeasure>,
    pub source_kind: &'static str,
    pub created_at_ms: u128,
    pub updated_at_ms: u128,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SemanticModelSummary {
    model_id: String,
    name: String,
    dataset_id: String,
    dimension_count: usize,
    measure_count: usize,
    tag_count: usize,
    updated_at_ms: u128,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SaveSemanticModelResponse {
    ok: bool,
    model: SavedSemanticModel,
    warnings: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CompileSemanticQueryRequest {
    pub dimensions: Vec<String>,
    pub measures: Vec<String>,
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CompiledSemanticQuery {
    pub model_id: String,
    pub dataset_id: String,
    pub sql: String,
    pub query: QueryRequest,
    pub selected_dimensions: Vec<SemanticDimension>,
    pub selected_measures: Vec<SemanticMeasure>,
}

impl SaveSemanticModelRequest {
    pub(crate) fn into_model(
        self,
        now_ms: u128,
        available_fields: &BTreeSet<String>,
    ) -> Result<SavedSemanticModel, String> {
        let model_id = clean_identifier(&self.model_id).ok_or_else(|| {
            "modelId must contain letters, numbers, dash, underscore, dot, or colon".to_string()
        })?;
        let name = self.name.trim().to_string();
        if name.is_empty() || name.len() > 160 {
            return Err("semantic model name must be 1-160 characters".to_string());
        }
        let dataset_id = clean_identifier(&self.dataset_id).ok_or_else(|| {
            "datasetId must contain letters, numbers, dash, underscore, dot, or colon".to_string()
        })?;
        let tags = normalize_tags(self.tags.unwrap_or_default())?;

        let mut source_kind = "json";
        let mut dimensions = Vec::new();
        let mut measures = Vec::new();
        if let Some(lookml) = self.lookml.as_deref() {
            if lookml.len() > MAX_LOOKML_BYTES {
                return Err(format!("lookml exceeds max {MAX_LOOKML_BYTES} bytes"));
            }
            let parsed = parse_lookml_subset(lookml)?;
            dimensions.extend(parsed.dimensions);
            measures.extend(parsed.measures);
            source_kind = "lookml-subset";
        }
        dimensions.extend(self.dimensions.unwrap_or_default());
        measures.extend(self.measures.unwrap_or_default());

        let dimensions = normalize_dimensions(dimensions, available_fields)?;
        let measures = normalize_measures(measures, available_fields)?;
        if dimensions.is_empty() && measures.is_empty() {
            return Err("semantic model requires at least one dimension or measure".to_string());
        }

        Ok(SavedSemanticModel {
            model_id,
            name,
            dataset_id,
            owner: self.owner.map(|owner| owner.trim().to_string()),
            tags,
            dimensions,
            measures,
            source_kind,
            created_at_ms: now_ms,
            updated_at_ms: now_ms,
        })
    }
}

impl SavedSemanticModel {
    pub(crate) fn summary(&self) -> SemanticModelSummary {
        SemanticModelSummary {
            model_id: self.model_id.clone(),
            name: self.name.clone(),
            dataset_id: self.dataset_id.clone(),
            dimension_count: self.dimensions.len(),
            measure_count: self.measures.len(),
            tag_count: self.tags.len(),
            updated_at_ms: self.updated_at_ms,
        }
    }

    pub(crate) fn compile_query(
        &self,
        request: CompileSemanticQueryRequest,
    ) -> Result<CompiledSemanticQuery, String> {
        if request.dimensions.is_empty() && request.measures.is_empty() {
            return Err("compile request requires at least one dimension or measure".to_string());
        }

        let dimensions_by_name = self
            .dimensions
            .iter()
            .map(|dimension| (dimension.name.as_str(), dimension))
            .collect::<BTreeMap<_, _>>();
        let measures_by_name = self
            .measures
            .iter()
            .map(|measure| (measure.name.as_str(), measure))
            .collect::<BTreeMap<_, _>>();

        let mut selected_dimensions = Vec::new();
        let mut selected_measures = Vec::new();
        for name in request.dimensions {
            let name = clean_identifier(&name)
                .ok_or_else(|| "dimension name in compile request is invalid".to_string())?;
            let dimension = dimensions_by_name
                .get(name.as_str())
                .ok_or_else(|| format!("semantic dimension `{name}` not found"))?;
            selected_dimensions.push((*dimension).clone());
        }
        for name in request.measures {
            let name = clean_identifier(&name)
                .ok_or_else(|| "measure name in compile request is invalid".to_string())?;
            let measure = measures_by_name
                .get(name.as_str())
                .ok_or_else(|| format!("semantic measure `{name}` not found"))?;
            selected_measures.push((*measure).clone());
        }

        let mut select_parts = Vec::new();
        for dimension in &selected_dimensions {
            select_parts.push(dimension.field.clone());
        }
        for measure in &selected_measures {
            select_parts.push(measure.sql_projection()?);
        }
        let mut sql = format!(
            "SELECT {} FROM {}",
            select_parts.join(", "),
            self.dataset_id
        );
        if !selected_dimensions.is_empty() && !selected_measures.is_empty() {
            let fields = selected_dimensions
                .iter()
                .map(|dimension| dimension.field.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            sql.push_str(" GROUP BY ");
            sql.push_str(&fields);
        }
        let limit = request.limit.unwrap_or(1_000).clamp(1, 5_000);
        sql.push_str(" LIMIT ");
        sql.push_str(&limit.to_string());

        let query = QueryRequest {
            dialect: QueryDialect::Sql,
            query: sql.clone(),
            dataset_id: Some(self.dataset_id.clone()),
            limit: Some(limit),
        };
        Ok(CompiledSemanticQuery {
            model_id: self.model_id.clone(),
            dataset_id: self.dataset_id.clone(),
            sql,
            query,
            selected_dimensions,
            selected_measures,
        })
    }
}

impl SemanticMeasure {
    fn sql_projection(&self) -> Result<String, String> {
        let expression = match self.aggregation {
            SemanticAggregation::Count => match self.field.as_deref() {
                Some(field) => format!("COUNT({field})"),
                None => "COUNT(*)".to_string(),
            },
            SemanticAggregation::Sum => format!("SUM({})", required_field(self)?),
            SemanticAggregation::Avg => format!("AVG({})", required_field(self)?),
            SemanticAggregation::Min => format!("MIN({})", required_field(self)?),
            SemanticAggregation::Max => format!("MAX({})", required_field(self)?),
        };
        Ok(format!("{expression} AS {}", self.name))
    }
}

impl SemanticAggregation {
    fn from_token(token: &str) -> Option<Self> {
        match token.trim().to_ascii_lowercase().as_str() {
            "count" | "count_distinct" => Some(Self::Count),
            "sum" | "total" => Some(Self::Sum),
            "avg" | "average" | "mean" => Some(Self::Avg),
            "min" | "minimum" => Some(Self::Min),
            "max" | "maximum" => Some(Self::Max),
            _ => None,
        }
    }

    fn dax_function(self) -> &'static str {
        match self {
            Self::Count => "COUNTROWS",
            Self::Sum => "SUM",
            Self::Avg => "AVERAGE",
            Self::Min => "MIN",
            Self::Max => "MAX",
        }
    }
}

pub(crate) fn save_response(
    model: SavedSemanticModel,
    warnings: Vec<String>,
) -> SaveSemanticModelResponse {
    SaveSemanticModelResponse {
        ok: true,
        model,
        warnings,
    }
}

pub(crate) fn registry_payload(models: Vec<SemanticModelSummary>) -> Value {
    json!({
        "ok": true,
        "schemaVersion": "data-viz.semantic-registry.v1",
        "models": models,
        "limits": {
            "maxSemanticModels": MAX_SEMANTIC_MODELS,
            "maxDimensions": MAX_DIMENSIONS,
            "maxMeasures": MAX_MEASURES,
            "maxTags": MAX_TAGS,
            "maxLookmlBytes": MAX_LOOKML_BYTES
        }
    })
}

pub(crate) fn max_semantic_models() -> usize {
    MAX_SEMANTIC_MODELS
}

fn normalize_dimensions(
    dimensions: Vec<SemanticDimension>,
    available_fields: &BTreeSet<String>,
) -> Result<Vec<SemanticDimension>, String> {
    if dimensions.len() > MAX_DIMENSIONS {
        return Err(format!("dimensions exceeds max {MAX_DIMENSIONS}"));
    }
    let mut seen = BTreeSet::new();
    let mut normalized = Vec::with_capacity(dimensions.len());
    for dimension in dimensions {
        let name = clean_identifier(&dimension.name)
            .ok_or_else(|| "semantic dimension name is invalid".to_string())?;
        if !seen.insert(name.clone()) {
            return Err(format!("duplicate semantic dimension `{name}`"));
        }
        let field = clean_field(&dimension.field)
            .ok_or_else(|| format!("semantic dimension `{name}` field is invalid"))?;
        if !available_fields.contains(&field) {
            return Err(format!(
                "semantic dimension `{name}` references missing field `{field}`"
            ));
        }
        normalized.push(SemanticDimension {
            name,
            field,
            data_type: dimension.data_type.map(|value| value.trim().to_string()),
            label: dimension.label.map(|value| value.trim().to_string()),
            primary_key: dimension.primary_key,
        });
    }
    Ok(normalized)
}

fn normalize_measures(
    measures: Vec<SemanticMeasure>,
    available_fields: &BTreeSet<String>,
) -> Result<Vec<SemanticMeasure>, String> {
    if measures.len() > MAX_MEASURES {
        return Err(format!("measures exceeds max {MAX_MEASURES}"));
    }
    let mut seen = BTreeSet::new();
    let mut normalized = Vec::with_capacity(measures.len());
    for measure in measures {
        let name = clean_identifier(&measure.name)
            .ok_or_else(|| "semantic measure name is invalid".to_string())?;
        if !seen.insert(name.clone()) {
            return Err(format!("duplicate semantic measure `{name}`"));
        }
        let field = measure.field.as_deref().and_then(clean_field);
        if measure.aggregation != SemanticAggregation::Count && field.is_none() {
            return Err(format!(
                "semantic measure `{name}` requires a field for {:?}",
                measure.aggregation
            ));
        }
        if let Some(field) = &field {
            if !available_fields.contains(field) {
                return Err(format!(
                    "semantic measure `{name}` references missing field `{field}`"
                ));
            }
        }
        let dax_analog = measure.dax_analog.or_else(|| {
            field.as_ref().map(|field| {
                format!(
                    "{}({})",
                    measure.aggregation.dax_function(),
                    field.replace(':', "_")
                )
            })
        });
        normalized.push(SemanticMeasure {
            name,
            field,
            aggregation: measure.aggregation,
            label: measure.label.map(|value| value.trim().to_string()),
            dax_analog,
        });
    }
    Ok(normalized)
}

fn normalize_tags(tags: Vec<String>) -> Result<Vec<String>, String> {
    if tags.len() > MAX_TAGS {
        return Err(format!("semantic model tags exceeds max {MAX_TAGS}"));
    }
    let mut tags = tags
        .into_iter()
        .filter_map(|tag| clean_identifier(&tag))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    tags.sort();
    Ok(tags)
}

fn required_field(measure: &SemanticMeasure) -> Result<&str, String> {
    measure
        .field
        .as_deref()
        .ok_or_else(|| format!("semantic measure `{}` requires a field", measure.name))
}

fn parse_lookml_subset(input: &str) -> Result<ParsedLookMl, String> {
    let mut parsed = ParsedLookMl::default();
    let mut pending = PendingLookMl::None;
    for raw_line in input.lines() {
        let line = raw_line
            .split('#')
            .next()
            .unwrap_or_default()
            .trim()
            .trim_matches(',');
        if line.is_empty() || line.starts_with("view:") || line.starts_with("explore:") {
            continue;
        }
        if line.starts_with("dimension:") {
            flush_pending(&mut parsed, &mut pending)?;
            let name = item_name(line, "dimension:")
                .ok_or_else(|| "LookML dimension is missing a name".to_string())?;
            pending = PendingLookMl::Dimension(DimensionBuilder {
                name,
                field: None,
                data_type: token_after(line, "type:"),
                label: label_after(line),
                primary_key: line.contains("primary_key: yes")
                    || line.contains("primary_key: true"),
            });
            apply_dimension_tokens(line, &mut pending);
            if line.contains('}') {
                flush_pending(&mut parsed, &mut pending)?;
            }
            continue;
        }
        if line.starts_with("measure:") {
            flush_pending(&mut parsed, &mut pending)?;
            let name = item_name(line, "measure:")
                .ok_or_else(|| "LookML measure is missing a name".to_string())?;
            pending = PendingLookMl::Measure(MeasureBuilder {
                name,
                field: None,
                aggregation: token_after(line, "type:")
                    .and_then(|token| SemanticAggregation::from_token(&token)),
                label: label_after(line),
            });
            apply_measure_tokens(line, &mut pending);
            if line.contains('}') {
                flush_pending(&mut parsed, &mut pending)?;
            }
            continue;
        }
        if line.starts_with('}') {
            flush_pending(&mut parsed, &mut pending)?;
            continue;
        }

        match &mut pending {
            PendingLookMl::Dimension(_) => apply_dimension_tokens(line, &mut pending),
            PendingLookMl::Measure(_) => apply_measure_tokens(line, &mut pending),
            PendingLookMl::None => {}
        }
    }
    flush_pending(&mut parsed, &mut pending)?;
    Ok(parsed)
}

fn apply_dimension_tokens(line: &str, pending: &mut PendingLookMl) {
    let PendingLookMl::Dimension(builder) = pending else {
        return;
    };
    if builder.field.is_none() {
        builder.field = token_after(line, "field:").or_else(|| sql_field_after(line));
    }
    if builder.data_type.is_none() {
        builder.data_type = token_after(line, "type:");
    }
    if builder.label.is_none() {
        builder.label = label_after(line);
    }
    if line.contains("primary_key: yes") || line.contains("primary_key: true") {
        builder.primary_key = true;
    }
}

fn apply_measure_tokens(line: &str, pending: &mut PendingLookMl) {
    let PendingLookMl::Measure(builder) = pending else {
        return;
    };
    if builder.field.is_none() {
        builder.field = token_after(line, "field:").or_else(|| sql_field_after(line));
    }
    if builder.aggregation.is_none() {
        builder.aggregation =
            token_after(line, "type:").and_then(|token| SemanticAggregation::from_token(&token));
    }
    if builder.label.is_none() {
        builder.label = label_after(line);
    }
}

fn flush_pending(parsed: &mut ParsedLookMl, pending: &mut PendingLookMl) -> Result<(), String> {
    match std::mem::replace(pending, PendingLookMl::None) {
        PendingLookMl::None => Ok(()),
        PendingLookMl::Dimension(builder) => {
            let field = builder
                .field
                .unwrap_or_else(|| builder.name.clone())
                .trim()
                .to_string();
            parsed.dimensions.push(SemanticDimension {
                name: builder.name,
                field,
                data_type: builder.data_type,
                label: builder.label,
                primary_key: Some(builder.primary_key),
            });
            Ok(())
        }
        PendingLookMl::Measure(builder) => {
            let aggregation = builder.aggregation.unwrap_or(SemanticAggregation::Count);
            parsed.measures.push(SemanticMeasure {
                name: builder.name,
                field: builder.field,
                aggregation,
                label: builder.label,
                dax_analog: None,
            });
            Ok(())
        }
    }
}

fn item_name(line: &str, marker: &str) -> Option<String> {
    let after = line.trim_start_matches(marker).trim();
    let token = after
        .split(|ch: char| ch.is_whitespace() || matches!(ch, '{' | '}'))
        .next()
        .unwrap_or_default();
    clean_identifier(token)
}

fn token_after(line: &str, marker: &str) -> Option<String> {
    let (_, after) = line.split_once(marker)?;
    let token = after
        .trim()
        .trim_matches('"')
        .trim_matches('\'')
        .split(|ch: char| ch.is_whitespace() || matches!(ch, ';' | '{' | '}'))
        .next()
        .unwrap_or_default()
        .trim_matches('"')
        .trim_matches('\'');
    clean_identifier(token)
}

fn label_after(line: &str) -> Option<String> {
    let (_, after) = line.split_once("label:")?;
    let value = after.trim();
    if let Some(stripped) = value.strip_prefix('"') {
        let label = stripped.split('"').next().unwrap_or_default().trim();
        if !label.is_empty() {
            return Some(label.to_string());
        }
    }
    token_after(line, "label:")
}

fn sql_field_after(line: &str) -> Option<String> {
    let (_, after) = line.split_once("sql:")?;
    let token = after
        .trim()
        .split(|ch: char| ch.is_whitespace() || ch == ';')
        .next()
        .unwrap_or_default();
    let token = token
        .trim_matches('$')
        .trim_matches('{')
        .trim_matches('}')
        .trim_matches('"')
        .trim_matches('\'');
    if token.eq_ignore_ascii_case("TABLE") {
        return None;
    }
    clean_field(token)
}

#[derive(Default)]
struct ParsedLookMl {
    dimensions: Vec<SemanticDimension>,
    measures: Vec<SemanticMeasure>,
}

enum PendingLookMl {
    None,
    Dimension(DimensionBuilder),
    Measure(MeasureBuilder),
}

struct DimensionBuilder {
    name: String,
    field: Option<String>,
    data_type: Option<String>,
    label: Option<String>,
    primary_key: bool,
}

struct MeasureBuilder {
    name: String,
    field: Option<String>,
    aggregation: Option<SemanticAggregation>,
    label: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn available_fields() -> BTreeSet<String> {
        ["region", "revenue", "orders"]
            .into_iter()
            .map(str::to_string)
            .collect()
    }

    #[test]
    fn lookml_subset_parses_dimensions_and_measures() {
        let parsed = parse_lookml_subset(
            r#"
            view: sales {
              dimension: region { type: string sql: ${TABLE}.region ;; }
              measure: total_revenue { type: sum sql: ${revenue} ;; }
              measure: order_count { type: count ;; }
            }
            "#,
        )
        .expect("lookml parses");

        assert_eq!(parsed.dimensions[0].name, "region");
        assert_eq!(parsed.dimensions[0].field, "region");
        assert_eq!(parsed.measures[0].name, "total_revenue");
        assert_eq!(parsed.measures[0].field.as_deref(), Some("revenue"));
        assert_eq!(parsed.measures[1].aggregation, SemanticAggregation::Count);
    }

    #[test]
    fn semantic_model_validates_fields_and_compiles_sql() {
        let model = SaveSemanticModelRequest {
            model_id: "sales_model".to_string(),
            name: "Sales Model".to_string(),
            dataset_id: "sales".to_string(),
            owner: Some("analytics".to_string()),
            tags: Some(vec!["sales".to_string()]),
            lookml: Some(
                r#"
                dimension: region { type: string sql: ${TABLE}.region ;; }
                measure: total_revenue { type: sum sql: ${revenue} ;; }
                "#
                .to_string(),
            ),
            dimensions: None,
            measures: None,
        }
        .into_model(100, &available_fields())
        .expect("model validates");

        let compiled = model
            .compile_query(CompileSemanticQueryRequest {
                dimensions: vec!["region".to_string()],
                measures: vec!["total_revenue".to_string()],
                limit: Some(25),
            })
            .expect("query compiles");

        assert_eq!(
            compiled.sql,
            "SELECT region, SUM(revenue) AS total_revenue FROM sales GROUP BY region LIMIT 25"
        );
        assert_eq!(compiled.query.dataset_id.as_deref(), Some("sales"));
    }

    #[test]
    fn semantic_model_rejects_missing_fields() {
        let error = SaveSemanticModelRequest {
            model_id: "bad".to_string(),
            name: "Bad".to_string(),
            dataset_id: "sales".to_string(),
            owner: None,
            tags: None,
            lookml: None,
            dimensions: Some(vec![SemanticDimension {
                name: "missing".to_string(),
                field: "nope".to_string(),
                data_type: None,
                label: None,
                primary_key: None,
            }]),
            measures: None,
        }
        .into_model(100, &available_fields())
        .expect_err("missing field rejected");

        assert!(error.contains("references missing field"));
    }
}
