//! Tableau-style analytics: level-of-detail (LOD) expressions and table
//! calculations evaluated over an already-materialized result set.
//!
//! LOD expressions (`FIXED` / `INCLUDE` / `EXCLUDE`) compute an aggregate at a
//! chosen dimensionality and broadcast it back onto every row. Table
//! calculations (running total, percent of total, moving average, rank,
//! difference) sweep the rows in their given order, optionally restarting
//! within a partition.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Number, Value};

const MAX_ROWS: usize = 50_000;
const MAX_EXPRESSIONS: usize = 64;
const MAX_DIMENSIONS: usize = 16;
const DEFAULT_WINDOW: usize = 3;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CalculationRequest {
    #[serde(default)]
    pub rows: Vec<BTreeMap<String, Value>>,
    /// Dimensions that define the current view; used to resolve INCLUDE/EXCLUDE.
    #[serde(default)]
    pub view_dimensions: Vec<String>,
    #[serde(default)]
    pub lod: Vec<LodExpression>,
    #[serde(default)]
    pub table_calculations: Vec<TableCalculation>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum LodKind {
    Fixed,
    Include,
    Exclude,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum AggregationOp {
    Count,
    Sum,
    Avg,
    Min,
    Max,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LodExpression {
    pub alias: String,
    pub kind: LodKind,
    #[serde(default)]
    pub dimensions: Vec<String>,
    pub op: AggregationOp,
    #[serde(default)]
    pub field: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum TableCalcOp {
    RunningTotal,
    PercentOfTotal,
    MovingAverage,
    Rank,
    Difference,
    PercentDifference,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TableCalculation {
    pub alias: String,
    pub op: TableCalcOp,
    pub field: String,
    /// Restart the calculation within each group of these dimensions.
    #[serde(default)]
    pub partition_by: Vec<String>,
    /// Window size for moving average (defaults to 3).
    #[serde(default)]
    pub window: Option<usize>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CalculationResponse {
    pub ok: bool,
    pub schema_version: String,
    pub row_count: usize,
    pub applied: Vec<String>,
    pub rows: Vec<BTreeMap<String, Value>>,
}

pub fn apply(request: CalculationRequest) -> Result<CalculationResponse, String> {
    if request.rows.len() > MAX_ROWS {
        return Err(format!("rows exceeds max {MAX_ROWS}"));
    }
    if request.lod.len() + request.table_calculations.len() > MAX_EXPRESSIONS {
        return Err(format!("expression count exceeds max {MAX_EXPRESSIONS}"));
    }

    let mut rows = request.rows;
    let mut applied = Vec::new();

    for expr in &request.lod {
        let alias = clean_alias(&expr.alias)?;
        let dims = resolve_lod_dimensions(expr, &request.view_dimensions)?;
        apply_lod(&mut rows, &alias, &dims, expr.op, expr.field.as_deref());
        applied.push(alias);
    }

    for calc in &request.table_calculations {
        let alias = clean_alias(&calc.alias)?;
        if calc.field.trim().is_empty() {
            return Err("table calculation field must not be empty".to_string());
        }
        if calc.partition_by.len() > MAX_DIMENSIONS {
            return Err(format!("partitionBy exceeds max {MAX_DIMENSIONS}"));
        }
        apply_table_calc(&mut rows, &alias, calc);
        applied.push(alias);
    }

    Ok(CalculationResponse {
        ok: true,
        schema_version: "data-viz.calculations.v1".to_string(),
        row_count: rows.len(),
        applied,
        rows,
    })
}

fn clean_alias(alias: &str) -> Result<String, String> {
    let trimmed = alias.trim();
    if trimmed.is_empty() || trimmed.len() > 120 {
        return Err("calculation alias must be 1-120 characters".to_string());
    }
    Ok(trimmed.to_string())
}

/// Resolve the grouping dimensions for an LOD expression. FIXED uses the
/// declared dimensions verbatim; INCLUDE adds them to the view; EXCLUDE removes
/// them from the view.
fn resolve_lod_dimensions(
    expr: &LodExpression,
    view_dimensions: &[String],
) -> Result<Vec<String>, String> {
    if expr.dimensions.len() > MAX_DIMENSIONS {
        return Err(format!("lod dimensions exceeds max {MAX_DIMENSIONS}"));
    }
    let dims = match expr.kind {
        LodKind::Fixed => expr.dimensions.clone(),
        LodKind::Include => {
            let mut dims = view_dimensions.to_vec();
            for dim in &expr.dimensions {
                if !dims.contains(dim) {
                    dims.push(dim.clone());
                }
            }
            dims
        }
        LodKind::Exclude => view_dimensions
            .iter()
            .filter(|dim| !expr.dimensions.contains(dim))
            .cloned()
            .collect(),
    };
    Ok(dims)
}

fn apply_lod(
    rows: &mut [BTreeMap<String, Value>],
    alias: &str,
    dims: &[String],
    op: AggregationOp,
    field: Option<&str>,
) {
    // Accumulate each group's values, then broadcast the aggregate back.
    let mut groups: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    for row in rows.iter() {
        let key = group_key(row, dims);
        let entry = groups.entry(key).or_default();
        if let Some(field) = field {
            if let Some(value) = row.get(field).and_then(as_f64) {
                entry.push(value);
            }
        } else {
            // No field => count rows.
            entry.push(1.0);
        }
    }

    let aggregates: BTreeMap<String, f64> = groups
        .into_iter()
        .map(|(key, values)| (key, aggregate(op, &values)))
        .collect();

    for row in rows.iter_mut() {
        let key = group_key(row, dims);
        if let Some(value) = aggregates.get(&key) {
            row.insert(alias.to_string(), number_value(*value));
        }
    }
}

fn aggregate(op: AggregationOp, values: &[f64]) -> f64 {
    match op {
        AggregationOp::Count => values.len() as f64,
        AggregationOp::Sum => values.iter().sum(),
        AggregationOp::Avg => {
            if values.is_empty() {
                0.0
            } else {
                values.iter().sum::<f64>() / values.len() as f64
            }
        }
        AggregationOp::Min => values.iter().copied().fold(f64::INFINITY, f64::min),
        AggregationOp::Max => values.iter().copied().fold(f64::NEG_INFINITY, f64::max),
    }
}

fn apply_table_calc(rows: &mut [BTreeMap<String, Value>], alias: &str, calc: &TableCalculation) {
    // Partition row indices in their original order, then compute per partition.
    let mut partitions: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (index, row) in rows.iter().enumerate() {
        partitions
            .entry(group_key(row, &calc.partition_by))
            .or_default()
            .push(index);
    }

    for indices in partitions.values() {
        let values: Vec<f64> = indices
            .iter()
            .map(|&index| rows[index].get(&calc.field).and_then(as_f64).unwrap_or(0.0))
            .collect();
        let results = compute_table_calc(calc, &values);
        for (slot, &index) in indices.iter().enumerate() {
            let value = match results[slot] {
                Some(number) => number_value(number),
                None => Value::Null,
            };
            rows[index].insert(alias.to_string(), value);
        }
    }
}

fn compute_table_calc(calc: &TableCalculation, values: &[f64]) -> Vec<Option<f64>> {
    match calc.op {
        TableCalcOp::RunningTotal => {
            let mut running = 0.0;
            values
                .iter()
                .map(|value| {
                    running += value;
                    Some(running)
                })
                .collect()
        }
        TableCalcOp::PercentOfTotal => {
            let total: f64 = values.iter().sum();
            values
                .iter()
                .map(|value| if total == 0.0 { None } else { Some(value / total) })
                .collect()
        }
        TableCalcOp::MovingAverage => {
            let window = calc.window.unwrap_or(DEFAULT_WINDOW).max(1);
            values
                .iter()
                .enumerate()
                .map(|(index, _)| {
                    let start = index.saturating_sub(window - 1);
                    let slice = &values[start..=index];
                    Some(slice.iter().sum::<f64>() / slice.len() as f64)
                })
                .collect()
        }
        TableCalcOp::Rank => values
            .iter()
            .map(|value| {
                // Standard competition rank, 1 = largest value.
                let greater = values.iter().filter(|other| **other > *value).count();
                Some((greater + 1) as f64)
            })
            .collect(),
        TableCalcOp::Difference => values
            .iter()
            .enumerate()
            .map(|(index, value)| {
                if index == 0 {
                    None
                } else {
                    Some(value - values[index - 1])
                }
            })
            .collect(),
        TableCalcOp::PercentDifference => values
            .iter()
            .enumerate()
            .map(|(index, value)| {
                if index == 0 {
                    None
                } else {
                    let prev = values[index - 1];
                    if prev == 0.0 {
                        None
                    } else {
                        Some((value - prev) / prev)
                    }
                }
            })
            .collect(),
    }
}

fn group_key(row: &BTreeMap<String, Value>, dims: &[String]) -> String {
    if dims.is_empty() {
        return String::new();
    }
    dims.iter()
        .map(|dim| match row.get(dim) {
            Some(Value::String(text)) => text.clone(),
            Some(value) => value.to_string(),
            None => String::new(),
        })
        .collect::<Vec<_>>()
        .join("\u{1f}")
}

fn as_f64(value: &Value) -> Option<f64> {
    match value {
        Value::Number(number) => number.as_f64(),
        Value::String(text) => text.trim().parse::<f64>().ok(),
        Value::Bool(flag) => Some(if *flag { 1.0 } else { 0.0 }),
        _ => None,
    }
}

fn number_value(value: f64) -> Value {
    if value.is_finite() {
        Number::from_f64(round6(value))
            .map(Value::Number)
            .unwrap_or(Value::Null)
    } else {
        Value::Null
    }
}

fn round6(value: f64) -> f64 {
    (value * 1_000_000.0).round() / 1_000_000.0
}

pub fn descriptor() -> Value {
    serde_json::json!({
        "ok": true,
        "schemaVersion": "data-viz.calculations.v1",
        "lod": {
            "kinds": ["fixed", "include", "exclude"],
            "aggregations": ["count", "sum", "avg", "min", "max"],
            "notes": "FIXED groups by the declared dimensions; INCLUDE/EXCLUDE are resolved against viewDimensions."
        },
        "tableCalculations": {
            "ops": [
                "runningTotal",
                "percentOfTotal",
                "movingAverage",
                "rank",
                "difference",
                "percentDifference"
            ],
            "notes": "Operate over rows in the supplied order; partitionBy restarts the calculation per group."
        },
        "limits": {
            "maxRows": MAX_ROWS,
            "maxExpressions": MAX_EXPRESSIONS,
            "maxDimensions": MAX_DIMENSIONS
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn row(pairs: &[(&str, Value)]) -> BTreeMap<String, Value> {
        pairs
            .iter()
            .map(|(key, value)| (key.to_string(), value.clone()))
            .collect()
    }

    #[test]
    fn fixed_lod_broadcasts_group_aggregate() {
        let rows = vec![
            row(&[("region", json!("west")), ("sales", json!(10))]),
            row(&[("region", json!("west")), ("sales", json!(30))]),
            row(&[("region", json!("east")), ("sales", json!(5))]),
        ];
        let request = CalculationRequest {
            rows,
            view_dimensions: vec![],
            lod: vec![LodExpression {
                alias: "regionTotal".to_string(),
                kind: LodKind::Fixed,
                dimensions: vec!["region".to_string()],
                op: AggregationOp::Sum,
                field: Some("sales".to_string()),
            }],
            table_calculations: vec![],
        };
        let response = apply(request).expect("calc applies");
        assert_eq!(response.rows[0]["regionTotal"], json!(40.0));
        assert_eq!(response.rows[1]["regionTotal"], json!(40.0));
        assert_eq!(response.rows[2]["regionTotal"], json!(5.0));
    }

    #[test]
    fn exclude_lod_drops_a_view_dimension() {
        let rows = vec![
            row(&[("region", json!("west")), ("city", json!("sf")), ("sales", json!(10))]),
            row(&[("region", json!("west")), ("city", json!("la")), ("sales", json!(20))]),
        ];
        let request = CalculationRequest {
            rows,
            view_dimensions: vec!["region".to_string(), "city".to_string()],
            lod: vec![LodExpression {
                alias: "regionTotal".to_string(),
                kind: LodKind::Exclude,
                dimensions: vec!["city".to_string()],
                op: AggregationOp::Sum,
                field: Some("sales".to_string()),
            }],
            table_calculations: vec![],
        };
        let response = apply(request).expect("calc applies");
        // Excluding city collapses both rows into the region group.
        assert_eq!(response.rows[0]["regionTotal"], json!(30.0));
        assert_eq!(response.rows[1]["regionTotal"], json!(30.0));
    }

    #[test]
    fn running_total_and_percent_of_total() {
        let rows = vec![
            row(&[("sales", json!(10))]),
            row(&[("sales", json!(30))]),
            row(&[("sales", json!(60))]),
        ];
        let request = CalculationRequest {
            rows,
            view_dimensions: vec![],
            lod: vec![],
            table_calculations: vec![
                TableCalculation {
                    alias: "cumulative".to_string(),
                    op: TableCalcOp::RunningTotal,
                    field: "sales".to_string(),
                    partition_by: vec![],
                    window: None,
                },
                TableCalculation {
                    alias: "share".to_string(),
                    op: TableCalcOp::PercentOfTotal,
                    field: "sales".to_string(),
                    partition_by: vec![],
                    window: None,
                },
            ],
        };
        let response = apply(request).expect("calc applies");
        assert_eq!(response.rows[2]["cumulative"], json!(100.0));
        assert_eq!(response.rows[0]["share"], json!(0.1));
        assert_eq!(response.rows[2]["share"], json!(0.6));
    }

    #[test]
    fn rank_and_difference_partition_independently() {
        let rows = vec![
            row(&[("region", json!("west")), ("sales", json!(10))]),
            row(&[("region", json!("west")), ("sales", json!(40))]),
            row(&[("region", json!("east")), ("sales", json!(99))]),
        ];
        let request = CalculationRequest {
            rows,
            view_dimensions: vec![],
            lod: vec![],
            table_calculations: vec![
                TableCalculation {
                    alias: "salesRank".to_string(),
                    op: TableCalcOp::Rank,
                    field: "sales".to_string(),
                    partition_by: vec!["region".to_string()],
                    window: None,
                },
                TableCalculation {
                    alias: "delta".to_string(),
                    op: TableCalcOp::Difference,
                    field: "sales".to_string(),
                    partition_by: vec!["region".to_string()],
                    window: None,
                },
            ],
        };
        let response = apply(request).expect("calc applies");
        // West partition: 10 ranks 2nd, 40 ranks 1st; east 99 ranks 1st alone.
        assert_eq!(response.rows[0]["salesRank"], json!(2.0));
        assert_eq!(response.rows[1]["salesRank"], json!(1.0));
        assert_eq!(response.rows[2]["salesRank"], json!(1.0));
        // First row in each partition has no predecessor.
        assert_eq!(response.rows[0]["delta"], Value::Null);
        assert_eq!(response.rows[1]["delta"], json!(30.0));
        assert_eq!(response.rows[2]["delta"], Value::Null);
    }

    #[test]
    fn moving_average_uses_window() {
        let rows = vec![
            row(&[("v", json!(1))]),
            row(&[("v", json!(2))]),
            row(&[("v", json!(3))]),
            row(&[("v", json!(10))]),
        ];
        let request = CalculationRequest {
            rows,
            view_dimensions: vec![],
            lod: vec![],
            table_calculations: vec![TableCalculation {
                alias: "ma".to_string(),
                op: TableCalcOp::MovingAverage,
                field: "v".to_string(),
                partition_by: vec![],
                window: Some(2),
            }],
        };
        let response = apply(request).expect("calc applies");
        assert_eq!(response.rows[0]["ma"], json!(1.0));
        assert_eq!(response.rows[3]["ma"], json!(6.5));
    }
}
