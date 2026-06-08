use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::util::{clean_identifier, now_ms};

const MAX_ETL_STEPS: usize = 64;
const MAX_ETL_FIELDS: usize = 256;
const MAX_ETL_JOIN_KEYS: usize = 8;
const MAX_ETL_AGGREGATIONS: usize = 64;
const MAX_ETL_FORMULA_BYTES: usize = 512;
const MAX_ETL_LIMIT: usize = 1_000_000;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DatasetShape {
    pub dataset_id: String,
    pub fields: Vec<FieldShape>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FieldShape {
    pub name: String,
    pub data_type: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EtlPlanRequest {
    pub plan_id: Option<String>,
    pub source_dataset: String,
    pub steps: Vec<EtlStep>,
    pub materialize: Option<MaterializeTarget>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", rename_all_fields = "camelCase")]
pub(crate) enum EtlStep {
    SelectColumns {
        fields: Vec<String>,
    },
    RenameColumns {
        mappings: BTreeMap<String, String>,
    },
    FilterRows {
        field: String,
        op: String,
        value: Value,
    },
    DeriveColumn {
        field: String,
        expression: String,
    },
    Join {
        dataset_id: String,
        left_key: String,
        right_key: String,
        join_type: Option<JoinType>,
    },
    Union {
        dataset_id: String,
    },
    GroupAggregate {
        group_by: Vec<String>,
        aggregations: Vec<EtlAggregation>,
    },
    SortRows {
        fields: Vec<SortField>,
    },
    LimitRows {
        limit: usize,
    },
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum JoinType {
    Inner,
    Left,
    Right,
    Full,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EtlAggregation {
    pub alias: String,
    pub op: EtlAggregationOp,
    pub field: Option<String>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum EtlAggregationOp {
    Count,
    Sum,
    Avg,
    Min,
    Max,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SortField {
    pub field: String,
    pub direction: Option<SortDirection>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum SortDirection {
    Asc,
    Desc,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MaterializeTarget {
    pub mode: MaterializeMode,
    pub target_dataset_id: Option<String>,
    pub refresh: Option<RefreshMode>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum MaterializeMode {
    Virtual,
    Snapshot,
    Incremental,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum RefreshMode {
    Manual,
    Scheduled,
    StreamingCheckpoint,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EtlPlanResponse {
    ok: bool,
    schema_version: &'static str,
    plan_id: String,
    source_dataset: String,
    generated_at_ms: u128,
    step_count: usize,
    output_fields: Vec<FieldShape>,
    logical_steps: Vec<PlannedEtlStep>,
    lineage: Vec<FieldLineage>,
    pushdown: Value,
    materialize: MaterializeTarget,
    contracts: Value,
    warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PlannedEtlStep {
    index: usize,
    kind: &'static str,
    planner_node: &'static str,
    reads: Vec<String>,
    writes: Vec<String>,
    output_field_count: usize,
    pushdown: &'static str,
    description: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FieldLineage {
    field: String,
    data_type: String,
    sources: Vec<String>,
    transformation: String,
}

#[derive(Debug, Clone)]
struct FieldState {
    data_type: String,
    sources: BTreeSet<String>,
    transformation: String,
}

pub(crate) fn max_etl_steps() -> usize {
    MAX_ETL_STEPS
}

pub(crate) fn max_etl_fields() -> usize {
    MAX_ETL_FIELDS
}

pub(crate) fn plan(
    request: EtlPlanRequest,
    datasets: &[DatasetShape],
) -> Result<EtlPlanResponse, String> {
    if request.steps.is_empty() {
        return Err("ETL plan requires at least one step".to_string());
    }
    if request.steps.len() > MAX_ETL_STEPS {
        return Err(format!("ETL steps exceeds max {MAX_ETL_STEPS}"));
    }
    let source_dataset = clean_identifier(&request.source_dataset)
        .ok_or_else(|| "sourceDataset must be a safe identifier".to_string())?;
    let dataset_index = datasets
        .iter()
        .map(|dataset| (dataset.dataset_id.as_str(), dataset))
        .collect::<BTreeMap<_, _>>();
    let source_shape = dataset_index
        .get(source_dataset.as_str())
        .copied()
        .ok_or_else(|| format!("source dataset `{source_dataset}` not found"))?;
    let plan_id = request
        .plan_id
        .as_deref()
        .and_then(clean_identifier)
        .unwrap_or_else(|| format!("{source_dataset}:etl-plan"));
    let materialize = request.materialize.unwrap_or(MaterializeTarget {
        mode: MaterializeMode::Virtual,
        target_dataset_id: None,
        refresh: Some(RefreshMode::Manual),
    });
    validate_materialize(&materialize)?;

    let mut warnings = Vec::new();
    let mut fields = initial_fields(source_shape)?;
    let mut logical_steps = Vec::new();
    let mut source_pushdown_steps = 0usize;
    let mut engine_steps = 0usize;

    for (index, step) in request.steps.iter().enumerate() {
        let planned = apply_step(
            index + 1,
            step,
            &dataset_index,
            &mut fields,
            &mut warnings,
        )?;
        if planned.pushdown == "source-pushdown" {
            source_pushdown_steps += 1;
        } else {
            engine_steps += 1;
        }
        logical_steps.push(planned);
    }

    let output_fields = fields
        .iter()
        .map(|(name, state)| FieldShape {
            name: name.clone(),
            data_type: state.data_type.clone(),
        })
        .collect::<Vec<_>>();
    let lineage = fields
        .into_iter()
        .map(|(field, state)| FieldLineage {
            field,
            data_type: state.data_type,
            sources: state.sources.into_iter().collect(),
            transformation: state.transformation,
        })
        .collect::<Vec<_>>();

    Ok(EtlPlanResponse {
        ok: true,
        schema_version: "data-viz.etl-plan.v1",
        plan_id,
        source_dataset,
        generated_at_ms: now_ms(),
        step_count: logical_steps.len(),
        output_fields,
        logical_steps,
        lineage,
        pushdown: json!({
            "sourcePushdownSteps": source_pushdown_steps,
            "engineSteps": engine_steps,
            "strategy": if source_pushdown_steps > 0 {
                "push simple projection/filter/sort/limit nodes to connectors, execute joins/formulas/aggregates in the engine until connector compilers land"
            } else {
                "engine-only planning"
            }
        }),
        materialize,
        contracts: json!({
            "powerQueryM": "validated AST skeleton; no user formula execution in planner",
            "domoMagicEtl": "node graph with bounded transforms, lineage, and refresh hints",
            "sigma": "live-grid compatible output field model",
            "superset": "SQL Lab-compatible relational plan outline"
        }),
        warnings,
    })
}

fn apply_step(
    index: usize,
    step: &EtlStep,
    dataset_index: &BTreeMap<&str, &DatasetShape>,
    fields: &mut BTreeMap<String, FieldState>,
    warnings: &mut Vec<String>,
) -> Result<PlannedEtlStep, String> {
    match step {
        EtlStep::SelectColumns { fields: requested } => {
            let selected = clean_field_list(requested, "select fields")?;
            if selected.len() > MAX_ETL_FIELDS {
                return Err(format!("select fields exceeds max {MAX_ETL_FIELDS}"));
            }
            for field in &selected {
                ensure_field(fields, field)?;
            }
            let next = selected
                .iter()
                .filter_map(|field| fields.get(field).cloned().map(|state| (field.clone(), state)))
                .collect::<BTreeMap<_, _>>();
            *fields = next;
            Ok(planned_step(
                index,
                "select-columns",
                "Projection",
                selected.clone(),
                selected,
                fields.len(),
                "source-pushdown",
                "Project a governed subset of columns.",
            ))
        }
        EtlStep::RenameColumns { mappings } => {
            if mappings.is_empty() {
                return Err("rename-columns requires at least one mapping".to_string());
            }
            let mut clean_mappings = BTreeMap::new();
            for (from, to) in mappings {
                let from = clean_identifier(from)
                    .ok_or_else(|| format!("invalid rename source field `{from}`"))?;
                let to =
                    clean_identifier(to).ok_or_else(|| format!("invalid rename target `{to}`"))?;
                ensure_field(fields, &from)?;
                clean_mappings.insert(from, to);
            }
            let mut next = BTreeMap::new();
            for (field, mut state) in fields.clone() {
                let output = clean_mappings
                    .get(&field)
                    .cloned()
                    .unwrap_or_else(|| field.clone());
                if next.contains_key(&output) {
                    return Err(format!("rename creates duplicate output field `{output}`"));
                }
                if output != field {
                    state.transformation = format!("rename({field})");
                }
                next.insert(output, state);
            }
            let writes = clean_mappings.values().cloned().collect::<Vec<_>>();
            let reads = clean_mappings.keys().cloned().collect::<Vec<_>>();
            *fields = next;
            Ok(planned_step(
                index,
                "rename-columns",
                "Rename",
                reads,
                writes,
                fields.len(),
                "engine",
                "Rename fields while preserving lineage.",
            ))
        }
        EtlStep::FilterRows { field, op, value: _ } => {
            let field =
                clean_identifier(field).ok_or_else(|| format!("invalid filter field `{field}`"))?;
            ensure_field(fields, &field)?;
            let op = op.trim();
            if !matches!(op, "=" | "==" | "!=" | ">" | ">=" | "<" | "<=" | "contains") {
                return Err(format!("unsupported filter operator `{op}`"));
            }
            Ok(planned_step(
                index,
                "filter-rows",
                "Filter",
                vec![field],
                Vec::new(),
                fields.len(),
                if op == "contains" {
                    "engine"
                } else {
                    "source-pushdown"
                },
                "Apply a bounded predicate without changing output columns.",
            ))
        }
        EtlStep::DeriveColumn { field, expression } => {
            let field =
                clean_identifier(field).ok_or_else(|| format!("invalid derived field `{field}`"))?;
            if expression.trim().is_empty() {
                return Err("derive-column expression cannot be empty".to_string());
            }
            if expression.len() > MAX_ETL_FORMULA_BYTES {
                return Err(format!(
                    "derive-column expression exceeds max {MAX_ETL_FORMULA_BYTES} bytes"
                ));
            }
            if expression.contains(';') {
                return Err("derive-column expression cannot contain statement separators".to_string());
            }
            let reads = expression_references(expression, fields);
            if reads.is_empty() {
                warnings.push(format!(
                    "derived field `{field}` has no references to current output fields"
                ));
            }
            let mut sources = BTreeSet::new();
            for read in &reads {
                if let Some(state) = fields.get(read) {
                    sources.extend(state.sources.iter().cloned());
                }
            }
            if sources.is_empty() {
                sources.insert(format!("formula:{field}"));
            }
            if fields.contains_key(&field) {
                warnings.push(format!("derived field `{field}` replaces an existing field"));
            }
            fields.insert(
                field.clone(),
                FieldState {
                    data_type: inferred_expression_type(expression),
                    sources,
                    transformation: format!("derive({})", expression.trim()),
                },
            );
            ensure_field_bound(fields)?;
            Ok(planned_step(
                index,
                "derive-column",
                "Formula",
                reads,
                vec![field],
                fields.len(),
                "engine",
                "Add a calculated field from a bounded formula string.",
            ))
        }
        EtlStep::Join {
            dataset_id,
            left_key,
            right_key,
            join_type,
        } => {
            let dataset_id = clean_identifier(dataset_id)
                .ok_or_else(|| format!("invalid join dataset `{dataset_id}`"))?;
            let left_key = clean_identifier(left_key)
                .ok_or_else(|| format!("invalid join left key `{left_key}`"))?;
            let right_key = clean_identifier(right_key)
                .ok_or_else(|| format!("invalid join right key `{right_key}`"))?;
            ensure_field(fields, &left_key)?;
            let right_shape = dataset_index
                .get(dataset_id.as_str())
                .copied()
                .ok_or_else(|| format!("join dataset `{dataset_id}` not found"))?;
            ensure_shape_field(right_shape, &right_key)?;
            let prefix = dataset_id.replace(['.', ':', '-'], "_");
            for right_field in &right_shape.fields {
                if right_field.name == right_key {
                    continue;
                }
                let output = if fields.contains_key(&right_field.name) {
                    format!("{prefix}__{}", right_field.name)
                } else {
                    right_field.name.clone()
                };
                fields.insert(
                    output.clone(),
                    FieldState {
                        data_type: right_field.data_type.clone(),
                        sources: BTreeSet::from([format!("{dataset_id}.{}", right_field.name)]),
                        transformation: format!("join({dataset_id})"),
                    },
                );
            }
            ensure_field_bound(fields)?;
            let join_label = match join_type {
                Some(JoinType::Inner) => "inner",
                Some(JoinType::Left) | None => "left",
                Some(JoinType::Right) => "right",
                Some(JoinType::Full) => "full",
            };
            Ok(planned_step(
                index,
                "join",
                "Join",
                vec![left_key, format!("{dataset_id}.{right_key}")],
                fields.keys().cloned().collect(),
                fields.len(),
                "engine",
                format!("Plan a {join_label} join and prefix conflicting right-side fields."),
            ))
        }
        EtlStep::Union { dataset_id } => {
            let dataset_id = clean_identifier(dataset_id)
                .ok_or_else(|| format!("invalid union dataset `{dataset_id}`"))?;
            let other = dataset_index
                .get(dataset_id.as_str())
                .copied()
                .ok_or_else(|| format!("union dataset `{dataset_id}` not found"))?;
            for (field, state) in fields.iter_mut() {
                let other_field = other
                    .fields
                    .iter()
                    .find(|candidate| candidate.name == *field)
                    .ok_or_else(|| {
                        format!("union dataset `{dataset_id}` is missing field `{field}`")
                    })?;
                if other_field.data_type != state.data_type {
                    warnings.push(format!(
                        "union field `{field}` type differs: {} vs {}",
                        state.data_type, other_field.data_type
                    ));
                }
                state.sources.insert(format!("{dataset_id}.{field}"));
                state.transformation = format!("union({dataset_id})");
            }
            Ok(planned_step(
                index,
                "union",
                "Union",
                fields.keys().cloned().collect(),
                Vec::new(),
                fields.len(),
                "engine",
                "Append another dataset with a compatible schema.",
            ))
        }
        EtlStep::GroupAggregate {
            group_by,
            aggregations,
        } => {
            let group_by = clean_field_list(group_by, "groupBy")?;
            if aggregations.is_empty() {
                return Err("group-aggregate requires at least one aggregation".to_string());
            }
            if aggregations.len() > MAX_ETL_AGGREGATIONS {
                return Err(format!(
                    "aggregations exceeds max {MAX_ETL_AGGREGATIONS}"
                ));
            }
            for field in &group_by {
                ensure_field(fields, field)?;
            }
            let mut next = BTreeMap::new();
            let mut reads = group_by.clone();
            for field in &group_by {
                if let Some(state) = fields.get(field).cloned() {
                    next.insert(field.clone(), state);
                }
            }
            for aggregation in aggregations {
                let alias = clean_identifier(&aggregation.alias)
                    .ok_or_else(|| format!("invalid aggregation alias `{}`", aggregation.alias))?;
                if next.contains_key(&alias) {
                    return Err(format!("aggregation alias `{alias}` duplicates an output field"));
                }
                let source_field = match aggregation.op {
                    EtlAggregationOp::Count => aggregation.field.as_deref().and_then(clean_identifier),
                    _ => {
                        let field = aggregation
                            .field
                            .as_deref()
                            .and_then(clean_identifier)
                            .ok_or_else(|| {
                                format!("aggregation `{alias}` requires a source field")
                            })?;
                        ensure_field(fields, &field)?;
                        Some(field)
                    }
                };
                if let Some(field) = &source_field {
                    reads.push(field.clone());
                    if !matches!(aggregation.op, EtlAggregationOp::Count)
                        && fields
                            .get(field)
                            .map(|state| state.data_type.as_str())
                            != Some("number")
                    {
                        warnings.push(format!(
                            "aggregation `{alias}` reads non-numeric field `{field}`"
                        ));
                    }
                }
                let sources = source_field
                    .as_ref()
                    .and_then(|field| fields.get(field))
                    .map(|state| state.sources.clone())
                    .unwrap_or_else(|| BTreeSet::from(["row-count".to_string()]));
                next.insert(
                    alias.clone(),
                    FieldState {
                        data_type: "number".to_string(),
                        sources,
                        transformation: format!("aggregate({})", aggregation.op.label()),
                    },
                );
            }
            *fields = next;
            ensure_field_bound(fields)?;
            Ok(planned_step(
                index,
                "group-aggregate",
                "Aggregate",
                reads,
                fields.keys().cloned().collect(),
                fields.len(),
                "engine",
                "Group rows and compute bounded reducer expressions.",
            ))
        }
        EtlStep::SortRows {
            fields: sort_fields,
        } => {
            if sort_fields.is_empty() {
                return Err("sort-rows requires at least one sort field".to_string());
            }
            if sort_fields.len() > MAX_ETL_JOIN_KEYS {
                return Err(format!("sort fields exceeds max {MAX_ETL_JOIN_KEYS}"));
            }
            let mut reads = Vec::new();
            for sort in sort_fields {
                let field = clean_identifier(&sort.field)
                    .ok_or_else(|| format!("invalid sort field `{}`", sort.field))?;
                ensure_field(fields, &field)?;
                reads.push(field);
                let _direction = sort.direction.as_ref().unwrap_or(&SortDirection::Asc);
            }
            Ok(planned_step(
                index,
                "sort-rows",
                "Sort",
                reads,
                Vec::new(),
                fields.len(),
                "source-pushdown",
                "Order rows by a bounded list of fields.",
            ))
        }
        EtlStep::LimitRows { limit } => {
            if *limit == 0 || *limit > MAX_ETL_LIMIT {
                return Err(format!("limit must be between 1 and {MAX_ETL_LIMIT}"));
            }
            Ok(planned_step(
                index,
                "limit-rows",
                "Limit",
                Vec::new(),
                Vec::new(),
                fields.len(),
                "source-pushdown",
                format!("Cap output rows at {limit}."),
            ))
        }
    }
}

fn initial_fields(shape: &DatasetShape) -> Result<BTreeMap<String, FieldState>, String> {
    if shape.fields.is_empty() {
        return Err(format!("dataset `{}` has no fields", shape.dataset_id));
    }
    if shape.fields.len() > MAX_ETL_FIELDS {
        return Err(format!("dataset fields exceeds max {MAX_ETL_FIELDS}"));
    }
    let mut fields = BTreeMap::new();
    for field in &shape.fields {
        let name = clean_identifier(&field.name)
            .ok_or_else(|| format!("invalid field `{}` in dataset shape", field.name))?;
        if fields.contains_key(&name) {
            return Err(format!("duplicate field `{name}` in dataset shape"));
        }
        fields.insert(
            name.clone(),
            FieldState {
                data_type: field.data_type.clone(),
                sources: BTreeSet::from([format!("{}.{}", shape.dataset_id, name)]),
                transformation: "source".to_string(),
            },
        );
    }
    Ok(fields)
}

fn validate_materialize(target: &MaterializeTarget) -> Result<(), String> {
    if let Some(target_dataset_id) = &target.target_dataset_id {
        clean_identifier(target_dataset_id)
            .ok_or_else(|| "materialize.targetDatasetId must be a safe identifier".to_string())?;
    }
    if matches!(
        target.mode,
        MaterializeMode::Snapshot | MaterializeMode::Incremental
    ) && target.target_dataset_id.is_none()
    {
        return Err("snapshot or incremental materialization requires targetDatasetId".to_string());
    }
    Ok(())
}

fn clean_field_list(fields: &[String], label: &str) -> Result<Vec<String>, String> {
    if fields.is_empty() {
        return Err(format!("{label} requires at least one field"));
    }
    let mut clean = Vec::new();
    let mut seen = BTreeSet::new();
    for field in fields {
        let field = clean_identifier(field)
            .ok_or_else(|| format!("{label} contains invalid field `{field}`"))?;
        if !seen.insert(field.clone()) {
            return Err(format!("{label} contains duplicate field `{field}`"));
        }
        clean.push(field);
    }
    Ok(clean)
}

fn ensure_field(fields: &BTreeMap<String, FieldState>, field: &str) -> Result<(), String> {
    if fields.contains_key(field) {
        Ok(())
    } else {
        Err(format!("field `{field}` is not available at this ETL step"))
    }
}

fn ensure_shape_field(shape: &DatasetShape, field: &str) -> Result<(), String> {
    if shape.fields.iter().any(|candidate| candidate.name == field) {
        Ok(())
    } else {
        Err(format!("dataset `{}` has no field `{field}`", shape.dataset_id))
    }
}

fn ensure_field_bound(fields: &BTreeMap<String, FieldState>) -> Result<(), String> {
    if fields.len() > MAX_ETL_FIELDS {
        Err(format!("ETL output fields exceeds max {MAX_ETL_FIELDS}"))
    } else {
        Ok(())
    }
}

fn planned_step(
    index: usize,
    kind: &'static str,
    planner_node: &'static str,
    mut reads: Vec<String>,
    mut writes: Vec<String>,
    output_field_count: usize,
    pushdown: &'static str,
    description: impl Into<String>,
) -> PlannedEtlStep {
    reads.sort();
    reads.dedup();
    writes.sort();
    writes.dedup();
    PlannedEtlStep {
        index,
        kind,
        planner_node,
        reads,
        writes,
        output_field_count,
        pushdown,
        description: description.into(),
    }
}

fn expression_references(
    expression: &str,
    fields: &BTreeMap<String, FieldState>,
) -> Vec<String> {
    let mut references = BTreeSet::new();
    let bytes = expression.as_bytes();
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes[index] == b'[' {
            if let Some(end) = expression[index + 1..].find(']') {
                let candidate = &expression[index + 1..index + 1 + end];
                if let Some(candidate) = clean_identifier(candidate) {
                    if fields.contains_key(&candidate) {
                        references.insert(candidate);
                    }
                }
                index += end + 2;
                continue;
            }
        }
        index += 1;
    }
    for token in expression.split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_')) {
        if token.is_empty() {
            continue;
        }
        if let Some(token) = clean_identifier(token) {
            if fields.contains_key(&token) {
                references.insert(token);
            }
        }
    }
    references.into_iter().collect()
}

fn inferred_expression_type(expression: &str) -> String {
    if expression.contains('+')
        || expression.contains('-')
        || expression.contains('*')
        || expression.contains('/')
    {
        "number".to_string()
    } else {
        "calculated".to_string()
    }
}

impl EtlAggregationOp {
    fn label(self) -> &'static str {
        match self {
            Self::Count => "count",
            Self::Sum => "sum",
            Self::Avg => "avg",
            Self::Min => "min",
            Self::Max => "max",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shapes() -> Vec<DatasetShape> {
        vec![
            DatasetShape {
                dataset_id: "sales".to_string(),
                fields: vec![
                    FieldShape {
                        name: "region".to_string(),
                        data_type: "category".to_string(),
                    },
                    FieldShape {
                        name: "revenue".to_string(),
                        data_type: "number".to_string(),
                    },
                    FieldShape {
                        name: "cost".to_string(),
                        data_type: "number".to_string(),
                    },
                    FieldShape {
                        name: "customer_id".to_string(),
                        data_type: "category".to_string(),
                    },
                ],
            },
            DatasetShape {
                dataset_id: "customers".to_string(),
                fields: vec![
                    FieldShape {
                        name: "customer_id".to_string(),
                        data_type: "category".to_string(),
                    },
                    FieldShape {
                        name: "segment".to_string(),
                        data_type: "category".to_string(),
                    },
                ],
            },
        ]
    }

    #[test]
    fn etl_plan_validates_magic_etl_flow_and_lineage() {
        let response = plan(
            EtlPlanRequest {
                plan_id: Some("profit-by-segment".to_string()),
                source_dataset: "sales".to_string(),
                steps: vec![
                    EtlStep::SelectColumns {
                        fields: vec![
                            "region".to_string(),
                            "customer_id".to_string(),
                            "revenue".to_string(),
                            "cost".to_string(),
                        ],
                    },
                    EtlStep::DeriveColumn {
                        field: "profit".to_string(),
                        expression: "[revenue] - [cost]".to_string(),
                    },
                    EtlStep::Join {
                        dataset_id: "customers".to_string(),
                        left_key: "customer_id".to_string(),
                        right_key: "customer_id".to_string(),
                        join_type: Some(JoinType::Left),
                    },
                    EtlStep::GroupAggregate {
                        group_by: vec!["segment".to_string()],
                        aggregations: vec![EtlAggregation {
                            alias: "total_profit".to_string(),
                            op: EtlAggregationOp::Sum,
                            field: Some("profit".to_string()),
                        }],
                    },
                    EtlStep::LimitRows { limit: 100 },
                ],
                materialize: Some(MaterializeTarget {
                    mode: MaterializeMode::Snapshot,
                    target_dataset_id: Some("profit_snapshot".to_string()),
                    refresh: Some(RefreshMode::Scheduled),
                }),
            },
            &shapes(),
        )
        .expect("plan compiles");

        assert_eq!(response.step_count, 5);
        assert_eq!(response.output_fields.len(), 2);
        assert!(response
            .lineage
            .iter()
            .any(|lineage| lineage.field == "total_profit"
                && lineage.sources.contains(&"sales.revenue".to_string())));
        assert_eq!(response.pushdown["sourcePushdownSteps"], 2);
    }

    #[test]
    fn etl_plan_rejects_missing_fields() {
        let error = plan(
            EtlPlanRequest {
                plan_id: None,
                source_dataset: "sales".to_string(),
                steps: vec![EtlStep::FilterRows {
                    field: "missing".to_string(),
                    op: "=".to_string(),
                    value: Value::from("north"),
                }],
                materialize: None,
            },
            &shapes(),
        )
        .expect_err("missing field rejected");

        assert!(error.contains("missing"));
    }
}
