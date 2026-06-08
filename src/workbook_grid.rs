use std::{cmp::Ordering, collections::BTreeMap};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::{
    util::{clean_identifier, scalar_to_label},
    Dataset,
};

const DEFAULT_GRID_PAGE_ROWS: usize = 100;
const MAX_GRID_PAGE_ROWS: usize = 500;
const MAX_GRID_OFFSET: usize = 50_000;
const MAX_GRID_FIELDS: usize = 64;
const MAX_GRID_FILTERS: usize = 16;
const MAX_GRID_SORTS: usize = 8;
const MAX_GRID_FORMULAS: usize = 16;
const MAX_GRID_FORMULA_BYTES: usize = 1_024;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorkbookGridPageRequest {
    pub dataset_id: String,
    pub fields: Option<Vec<String>>,
    pub offset: Option<usize>,
    pub limit: Option<usize>,
    pub filters: Option<Vec<GridFilter>>,
    pub sorts: Option<Vec<GridSort>>,
    pub formula_columns: Option<Vec<GridFormulaColumn>>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GridFilter {
    pub field: String,
    pub op: GridFilterOp,
    pub value: Option<Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum GridFilterOp {
    Eq,
    Neq,
    Contains,
    Gt,
    Gte,
    Lt,
    Lte,
    IsNull,
    IsNotNull,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GridSort {
    pub field: String,
    pub direction: Option<GridSortDirection>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum GridSortDirection {
    Asc,
    Desc,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GridFormulaColumn {
    pub name: String,
    pub expression: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorkbookGridPageResponse {
    ok: bool,
    schema_version: &'static str,
    dataset_id: String,
    total_rows: usize,
    matched_rows: usize,
    offset: usize,
    limit: usize,
    returned_rows: usize,
    has_next_page: bool,
    fields: Vec<GridField>,
    formula_columns: Vec<PlannedFormulaColumn>,
    rows: Vec<GridRow>,
    materialization: GridMaterialization,
    warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct GridField {
    name: String,
    data_type: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct PlannedFormulaColumn {
    name: String,
    expression: String,
    dependencies: Vec<String>,
    data_type: &'static str,
    status: &'static str,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct GridRow {
    row_number: usize,
    values: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct GridMaterialization {
    mode: &'static str,
    strategy: &'static str,
    pushdown_hint: &'static str,
}

pub(crate) fn page(
    dataset: &Dataset,
    request: WorkbookGridPageRequest,
) -> Result<WorkbookGridPageResponse, String> {
    let dataset_id = clean_identifier(&request.dataset_id).ok_or_else(|| {
        "datasetId must contain letters, numbers, dash, underscore, dot, or colon".to_string()
    })?;
    if dataset_id != dataset.dataset_id {
        return Err(format!(
            "dataset `{dataset_id}` does not match loaded dataset"
        ));
    }
    let fields = normalize_fields(request.fields, dataset)?;
    let filters = normalize_filters(request.filters.unwrap_or_default(), dataset)?;
    let sorts = normalize_sorts(request.sorts.unwrap_or_default(), dataset)?;
    let formula_columns =
        normalize_formula_columns(request.formula_columns.unwrap_or_default(), dataset)?;
    let offset = request.offset.unwrap_or(0);
    if offset > MAX_GRID_OFFSET {
        return Err(format!("offset exceeds max {MAX_GRID_OFFSET}"));
    }
    let limit = request
        .limit
        .unwrap_or(DEFAULT_GRID_PAGE_ROWS)
        .clamp(1, MAX_GRID_PAGE_ROWS);

    let mut row_indexes = (0..dataset.row_count)
        .filter(|row| filters.iter().all(|filter| filter.matches(dataset, *row)))
        .collect::<Vec<_>>();
    row_indexes.sort_by(|left, right| compare_rows(dataset, *left, *right, &sorts));
    let matched_rows = row_indexes.len();
    let page_indexes = row_indexes
        .into_iter()
        .skip(offset)
        .take(limit)
        .collect::<Vec<_>>();
    let rows = page_indexes
        .iter()
        .map(|row| {
            let values = fields
                .iter()
                .map(|field| (field.clone(), dataset.value(field, *row)))
                .collect::<BTreeMap<_, _>>();
            GridRow {
                row_number: row + 1,
                values,
            }
        })
        .collect::<Vec<_>>();
    let grid_fields = fields
        .iter()
        .map(|field| GridField {
            name: field.clone(),
            data_type: dataset.field_type(field),
        })
        .collect::<Vec<_>>();
    let warnings = if formula_columns.is_empty() {
        Vec::new()
    } else {
        vec![
            "formula columns are validated and planned, but not evaluated in this bounded grid page"
                .to_string(),
        ]
    };

    Ok(WorkbookGridPageResponse {
        ok: true,
        schema_version: "data-viz.workbook-grid-page.v1",
        dataset_id,
        total_rows: dataset.row_count,
        matched_rows,
        offset,
        limit,
        returned_rows: rows.len(),
        has_next_page: offset.saturating_add(rows.len()) < matched_rows,
        fields: grid_fields,
        formula_columns,
        rows,
        materialization: GridMaterialization {
            mode: "in-memory-columnar-page",
            strategy: "filter, sort, and slice row indexes over the columnar dataset without full JSON materialization",
            pushdown_hint: "same request shape can be translated to warehouse ORDER BY / WHERE / LIMIT / OFFSET for Sigma-style live grids",
        },
        warnings,
    })
}

pub(crate) fn limits_payload() -> Value {
    json!({
        "defaultPageRows": DEFAULT_GRID_PAGE_ROWS,
        "maxPageRows": MAX_GRID_PAGE_ROWS,
        "maxOffset": MAX_GRID_OFFSET,
        "maxFields": MAX_GRID_FIELDS,
        "maxFilters": MAX_GRID_FILTERS,
        "maxSorts": MAX_GRID_SORTS,
        "maxFormulaColumns": MAX_GRID_FORMULAS,
        "maxFormulaBytes": MAX_GRID_FORMULA_BYTES
    })
}

fn normalize_fields(fields: Option<Vec<String>>, dataset: &Dataset) -> Result<Vec<String>, String> {
    let raw_fields = fields.unwrap_or_else(|| dataset.columns.keys().cloned().collect());
    if raw_fields.is_empty() {
        return Err("fields must include at least one field".to_string());
    }
    if raw_fields.len() > MAX_GRID_FIELDS {
        return Err(format!("fields exceeds max {MAX_GRID_FIELDS}"));
    }
    let mut fields = Vec::with_capacity(raw_fields.len());
    for raw_field in raw_fields {
        let field = clean_identifier(&raw_field)
            .ok_or_else(|| format!("invalid workbook grid field `{raw_field}`"))?;
        ensure_field(dataset, &field)?;
        if !fields.contains(&field) {
            fields.push(field);
        }
    }
    Ok(fields)
}

fn normalize_filters(
    filters: Vec<GridFilter>,
    dataset: &Dataset,
) -> Result<Vec<GridFilter>, String> {
    if filters.len() > MAX_GRID_FILTERS {
        return Err(format!("filters exceeds max {MAX_GRID_FILTERS}"));
    }
    filters
        .into_iter()
        .map(|filter| {
            let field = clean_identifier(&filter.field)
                .ok_or_else(|| format!("invalid workbook grid filter field `{}`", filter.field))?;
            ensure_field(dataset, &field)?;
            if !matches!(filter.op, GridFilterOp::IsNull | GridFilterOp::IsNotNull)
                && filter.value.is_none()
            {
                return Err(format!("filter `{field}` requires a value"));
            }
            if filter.value.as_ref().map(is_scalar_value).unwrap_or(true) {
                Ok(GridFilter { field, ..filter })
            } else {
                Err(format!("filter `{field}` value must be scalar JSON"))
            }
        })
        .collect()
}

fn normalize_sorts(sorts: Vec<GridSort>, dataset: &Dataset) -> Result<Vec<GridSort>, String> {
    if sorts.len() > MAX_GRID_SORTS {
        return Err(format!("sorts exceeds max {MAX_GRID_SORTS}"));
    }
    sorts
        .into_iter()
        .map(|sort| {
            let field = clean_identifier(&sort.field)
                .ok_or_else(|| format!("invalid workbook grid sort field `{}`", sort.field))?;
            ensure_field(dataset, &field)?;
            Ok(GridSort { field, ..sort })
        })
        .collect()
}

fn normalize_formula_columns(
    formulas: Vec<GridFormulaColumn>,
    dataset: &Dataset,
) -> Result<Vec<PlannedFormulaColumn>, String> {
    if formulas.len() > MAX_GRID_FORMULAS {
        return Err(format!("formulaColumns exceeds max {MAX_GRID_FORMULAS}"));
    }
    formulas
        .into_iter()
        .map(|formula| {
            let name = clean_identifier(&formula.name)
                .ok_or_else(|| format!("invalid formula column name `{}`", formula.name))?;
            let expression = formula.expression.trim().to_string();
            if expression.is_empty() {
                return Err(format!(
                    "formula column `{name}` expression cannot be empty"
                ));
            }
            if expression.len() > MAX_GRID_FORMULA_BYTES {
                return Err(format!(
                    "formula column `{name}` expression exceeds max {MAX_GRID_FORMULA_BYTES} bytes"
                ));
            }
            if expression.contains(';') || looks_secret_bearing(&expression) {
                return Err(format!(
                    "formula column `{name}` expression contains unsupported or secret-looking text"
                ));
            }
            let dependencies = formula_dependencies(&expression, dataset)?;
            Ok(PlannedFormulaColumn {
                name,
                expression,
                dependencies,
                data_type: "planned",
                status: "validated-not-evaluated",
            })
        })
        .collect()
}

fn formula_dependencies(expression: &str, dataset: &Dataset) -> Result<Vec<String>, String> {
    let mut dependencies = Vec::new();
    let mut index = 0;
    while let Some(start) = expression[index..].find('[') {
        let absolute_start = index + start;
        let Some(end) = expression[absolute_start + 1..].find(']') else {
            return Err("formula expression has unterminated field reference".to_string());
        };
        let candidate = &expression[absolute_start + 1..absolute_start + 1 + end];
        let field = clean_identifier(candidate)
            .ok_or_else(|| format!("invalid formula field reference `{candidate}`"))?;
        ensure_field(dataset, &field)?;
        if !dependencies.contains(&field) {
            dependencies.push(field);
        }
        index = absolute_start + end + 2;
    }
    Ok(dependencies)
}

fn ensure_field(dataset: &Dataset, field: &str) -> Result<(), String> {
    if dataset.columns.contains_key(field) {
        Ok(())
    } else {
        Err(format!("field `{field}` does not exist in dataset"))
    }
}

impl GridFilter {
    fn matches(&self, dataset: &Dataset, row: usize) -> bool {
        let actual = dataset.value(&self.field, row);
        match self.op {
            GridFilterOp::Eq => self
                .value
                .as_ref()
                .map(|expected| scalar_to_label(&actual) == scalar_to_label(expected))
                .unwrap_or(false),
            GridFilterOp::Neq => self
                .value
                .as_ref()
                .map(|expected| scalar_to_label(&actual) != scalar_to_label(expected))
                .unwrap_or(false),
            GridFilterOp::Contains => self
                .value
                .as_ref()
                .map(|expected| {
                    scalar_to_label(&actual)
                        .to_ascii_lowercase()
                        .contains(&scalar_to_label(expected).to_ascii_lowercase())
                })
                .unwrap_or(false),
            GridFilterOp::Gt => compare_values(&actual, self.value.as_ref()) == Ordering::Greater,
            GridFilterOp::Gte => matches!(
                compare_values(&actual, self.value.as_ref()),
                Ordering::Greater | Ordering::Equal
            ),
            GridFilterOp::Lt => compare_values(&actual, self.value.as_ref()) == Ordering::Less,
            GridFilterOp::Lte => matches!(
                compare_values(&actual, self.value.as_ref()),
                Ordering::Less | Ordering::Equal
            ),
            GridFilterOp::IsNull => actual.is_null(),
            GridFilterOp::IsNotNull => !actual.is_null(),
        }
    }
}

fn compare_rows(dataset: &Dataset, left: usize, right: usize, sorts: &[GridSort]) -> Ordering {
    for sort in sorts {
        let ordering = compare_values(
            &dataset.value(&sort.field, left),
            Some(&dataset.value(&sort.field, right)),
        );
        if ordering != Ordering::Equal {
            return match sort.direction.unwrap_or(GridSortDirection::Asc) {
                GridSortDirection::Asc => ordering,
                GridSortDirection::Desc => ordering.reverse(),
            };
        }
    }
    left.cmp(&right)
}

fn compare_values(left: &Value, right: Option<&Value>) -> Ordering {
    let Some(right) = right else {
        return Ordering::Equal;
    };
    match (left.as_f64(), right.as_f64()) {
        (Some(left), Some(right)) => left.total_cmp(&right),
        _ => scalar_to_label(left).cmp(&scalar_to_label(right)),
    }
}

fn is_scalar_value(value: &Value) -> bool {
    matches!(
        value,
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_)
    )
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
    use crate::IngestDatasetRequest;

    fn record(pairs: Vec<(&str, Value)>) -> BTreeMap<String, Value> {
        pairs
            .into_iter()
            .map(|(key, value)| (key.to_string(), value))
            .collect()
    }

    fn dataset() -> Dataset {
        Dataset::from_request(IngestDatasetRequest {
            dataset_id: "sales-grid".to_string(),
            display_name: None,
            replace: Some(true),
            records: vec![
                record(vec![
                    ("region", Value::from("north")),
                    ("revenue", Value::from(10)),
                    ("cost", Value::from(4)),
                ]),
                record(vec![
                    ("region", Value::from("south")),
                    ("revenue", Value::from(20)),
                    ("cost", Value::from(11)),
                ]),
                record(vec![
                    ("region", Value::from("west")),
                    ("revenue", Value::from(7)),
                    ("cost", Value::from(3)),
                ]),
            ],
        })
        .expect("dataset builds")
    }

    #[test]
    fn workbook_grid_pages_filtered_and_sorted_rows() {
        let response = page(
            &dataset(),
            WorkbookGridPageRequest {
                dataset_id: "sales-grid".to_string(),
                fields: Some(vec!["region".to_string(), "revenue".to_string()]),
                offset: Some(0),
                limit: Some(2),
                filters: Some(vec![GridFilter {
                    field: "revenue".to_string(),
                    op: GridFilterOp::Gte,
                    value: Some(Value::from(8)),
                }]),
                sorts: Some(vec![GridSort {
                    field: "revenue".to_string(),
                    direction: Some(GridSortDirection::Desc),
                }]),
                formula_columns: Some(vec![GridFormulaColumn {
                    name: "margin".to_string(),
                    expression: "[revenue] - [cost]".to_string(),
                }]),
            },
        )
        .expect("grid page");

        assert_eq!(response.matched_rows, 2);
        assert_eq!(response.rows[0].values["region"], Value::from("south"));
        assert_eq!(
            response.formula_columns[0].dependencies,
            vec!["revenue", "cost"]
        );
        assert!(response.has_next_page == false);
    }

    #[test]
    fn workbook_grid_rejects_missing_formula_fields() {
        let error = page(
            &dataset(),
            WorkbookGridPageRequest {
                dataset_id: "sales-grid".to_string(),
                fields: None,
                offset: None,
                limit: None,
                filters: None,
                sorts: None,
                formula_columns: Some(vec![GridFormulaColumn {
                    name: "bad".to_string(),
                    expression: "[missing] + 1".to_string(),
                }]),
            },
        )
        .expect_err("missing formula dependency rejected");

        assert!(error.contains("does not exist"));
    }
}
