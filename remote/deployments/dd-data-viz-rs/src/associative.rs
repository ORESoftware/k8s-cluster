use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::{
    util::{clean_identifier, round4, scalar_to_label},
    Column, Dataset,
};

const MAX_SELECTION_DATASETS: usize = 16;
const MAX_SELECTIONS: usize = 64;
const DEFAULT_MAX_VALUES_PER_FIELD: usize = 32;
const MAX_VALUES_PER_FIELD: usize = 128;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AssociativeSelectionRequest {
    pub dataset_ids: Vec<String>,
    #[serde(default)]
    pub selections: Vec<AssociativeSelection>,
    pub max_values_per_field: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AssociativeSelection {
    pub dataset_id: Option<String>,
    pub field: String,
    pub value: Value,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct NormalizedSelection {
    dataset_id: Option<String>,
    field: String,
    value: String,
    source: &'static str,
}

pub(crate) fn selection_payload(
    datasets: &BTreeMap<String, Dataset>,
    request: AssociativeSelectionRequest,
) -> Result<Value, String> {
    let dataset_ids = normalize_dataset_ids(&request.dataset_ids)?;
    let max_values = request
        .max_values_per_field
        .unwrap_or(DEFAULT_MAX_VALUES_PER_FIELD)
        .clamp(1, MAX_VALUES_PER_FIELD);
    if request.selections.len() > MAX_SELECTIONS {
        return Err(format!("selections exceeds max {MAX_SELECTIONS}"));
    }

    let mut selected_datasets = Vec::with_capacity(dataset_ids.len());
    for dataset_id in &dataset_ids {
        let dataset = datasets
            .get(dataset_id)
            .ok_or_else(|| format!("dataset `{dataset_id}` not found"))?;
        selected_datasets.push(dataset);
    }

    let mut warnings = Vec::new();
    let selections = normalize_selections(&request.selections, datasets, &mut warnings)?;
    let relationship_index = relationship_index(&selected_datasets);
    let mut dataset_summaries = Vec::with_capacity(selected_datasets.len());
    let mut field_states = Vec::new();

    for dataset in selected_datasets {
        let fields = categorical_fields(dataset);
        let constraints = constraints_for_dataset(dataset, &selections);
        let row_mask = row_mask(dataset, &constraints);
        let selected_rows = row_mask.iter().filter(|included| **included).count();

        dataset_summaries.push(json!({
            "datasetId": dataset.dataset_id,
            "displayName": dataset.display_name,
            "rowCount": dataset.row_count,
            "selectedRowCount": selected_rows,
            "selectedRowRatio": if dataset.row_count == 0 {
                0.0
            } else {
                round4(selected_rows as f64 / dataset.row_count as f64)
            },
            "categoricalFieldCount": fields.len(),
            "constrainedFields": constraints.keys().cloned().collect::<Vec<_>>()
        }));

        for field in fields {
            let selected_values = selections
                .iter()
                .filter(|selection| selection.field == field)
                .map(|selection| selection.value.clone())
                .collect::<BTreeSet<_>>();
            field_states.push(field_state(
                dataset,
                &field,
                &row_mask,
                &selected_values,
                max_values,
            ));
        }
    }

    Ok(json!({
        "ok": true,
        "schemaVersion": "data-viz.associative-selection.v1",
        "datasetCount": dataset_ids.len(),
        "activeSelectionCount": selections.len(),
        "datasets": dataset_summaries,
        "fields": field_states,
        "selections": selections,
        "relationshipIndex": relationship_index,
        "selectionModel": {
            "selected": "The user-selected green value.",
            "possible": "White values still reachable after current selections.",
            "alternative": "Other values in a selected field that remain reachable.",
            "excluded": "Gray values excluded by the current selection state.",
            "propagation": "Selections propagate across datasets by exact categorical field/value equality."
        },
        "limits": {
            "maxDatasets": MAX_SELECTION_DATASETS,
            "maxSelections": MAX_SELECTIONS,
            "maxValuesPerField": MAX_VALUES_PER_FIELD
        },
        "warnings": warnings
    }))
}

fn normalize_dataset_ids(raw_ids: &[String]) -> Result<Vec<String>, String> {
    if raw_ids.is_empty() {
        return Err("datasetIds must include at least one dataset".to_string());
    }
    if raw_ids.len() > MAX_SELECTION_DATASETS {
        return Err(format!("datasetIds exceeds max {MAX_SELECTION_DATASETS}"));
    }

    let mut ids = Vec::with_capacity(raw_ids.len());
    let mut seen = BTreeSet::new();
    for raw_id in raw_ids {
        let dataset_id = clean_identifier(raw_id).ok_or_else(|| {
            "datasetIds must contain letters, numbers, dash, underscore, dot, or colon".to_string()
        })?;
        if seen.insert(dataset_id.clone()) {
            ids.push(dataset_id);
        }
    }

    Ok(ids)
}

fn normalize_selections(
    selections: &[AssociativeSelection],
    datasets: &BTreeMap<String, Dataset>,
    warnings: &mut Vec<String>,
) -> Result<Vec<NormalizedSelection>, String> {
    let mut normalized = Vec::with_capacity(selections.len());
    for selection in selections {
        let dataset_id = match selection.dataset_id.as_deref() {
            Some(raw_id) => Some(
                clean_identifier(raw_id)
                    .ok_or_else(|| "selection datasetId is invalid".to_string())?,
            ),
            None => None,
        };
        if let Some(dataset_id) = &dataset_id {
            if !datasets.contains_key(dataset_id) {
                return Err(format!("selection dataset `{dataset_id}` not found"));
            }
        }

        let field = clean_identifier(&selection.field)
            .ok_or_else(|| "selection field is invalid".to_string())?;
        let value = scalar_to_label(&selection.value);
        if value == "null" {
            return Err("selection value cannot be null".to_string());
        }
        let field_exists = datasets
            .values()
            .any(|dataset| dataset.columns.contains_key(&field));
        if !field_exists {
            warnings.push(format!(
                "selection field `{field}` does not exist in any loaded dataset"
            ));
        }

        normalized.push(NormalizedSelection {
            dataset_id,
            field,
            value,
            source: "request",
        });
    }

    Ok(normalized)
}

fn categorical_fields(dataset: &Dataset) -> Vec<String> {
    dataset
        .columns
        .iter()
        .filter_map(|(field, column)| match column {
            Column::Dictionary { .. } | Column::Boolean(_) => Some(field.clone()),
            Column::Number(_) => None,
        })
        .collect()
}

fn constraints_for_dataset(
    dataset: &Dataset,
    selections: &[NormalizedSelection],
) -> BTreeMap<String, BTreeSet<String>> {
    let fields = categorical_fields(dataset)
        .into_iter()
        .collect::<BTreeSet<_>>();
    let mut constraints = BTreeMap::<String, BTreeSet<String>>::new();
    for selection in selections {
        if fields.contains(&selection.field) {
            constraints
                .entry(selection.field.clone())
                .or_default()
                .insert(selection.value.clone());
        }
    }
    constraints
}

fn row_mask(dataset: &Dataset, constraints: &BTreeMap<String, BTreeSet<String>>) -> Vec<bool> {
    if constraints.is_empty() {
        return vec![true; dataset.row_count];
    }

    (0..dataset.row_count)
        .map(|row| {
            constraints.iter().all(|(field, values)| {
                let value = dataset.value(field, row);
                if value.is_null() {
                    return false;
                }
                values.contains(&scalar_to_label(&value))
            })
        })
        .collect()
}

fn field_state(
    dataset: &Dataset,
    field: &str,
    row_mask: &[bool],
    selected_values: &BTreeSet<String>,
    max_values: usize,
) -> Value {
    let mut counts = BTreeMap::<String, (usize, usize)>::new();
    for row in 0..dataset.row_count {
        let value = dataset.value(field, row);
        if value.is_null() {
            continue;
        }
        let label = scalar_to_label(&value);
        let entry = counts.entry(label).or_default();
        entry.0 += 1;
        if row_mask.get(row).copied().unwrap_or(false) {
            entry.1 += 1;
        }
    }

    let mut values = counts
        .into_iter()
        .map(|(value, (total_count, possible_count))| {
            let state = if selected_values.contains(&value) {
                "selected"
            } else if possible_count > 0 && !selected_values.is_empty() {
                "alternative"
            } else if possible_count > 0 {
                "possible"
            } else {
                "excluded"
            };
            json!({
                "value": value,
                "state": state,
                "totalCount": total_count,
                "possibleCount": possible_count,
                "possibleRatio": if total_count == 0 {
                    0.0
                } else {
                    round4(possible_count as f64 / total_count as f64)
                }
            })
        })
        .collect::<Vec<_>>();
    values.sort_by(|left, right| {
        state_rank(left["state"].as_str().unwrap_or("excluded"))
            .cmp(&state_rank(right["state"].as_str().unwrap_or("excluded")))
            .then_with(|| {
                right["possibleCount"]
                    .as_u64()
                    .cmp(&left["possibleCount"].as_u64())
            })
            .then_with(|| {
                right["totalCount"]
                    .as_u64()
                    .cmp(&left["totalCount"].as_u64())
            })
            .then_with(|| {
                left["value"]
                    .as_str()
                    .unwrap_or_default()
                    .cmp(right["value"].as_str().unwrap_or_default())
            })
    });
    values.truncate(max_values);

    let mut state_counts = BTreeMap::<&str, usize>::new();
    for value in &values {
        let state = value["state"].as_str().unwrap_or("excluded");
        *state_counts.entry(state).or_default() += 1;
    }

    json!({
        "datasetId": dataset.dataset_id,
        "field": field,
        "valueCount": values.len(),
        "stateCounts": state_counts,
        "values": values
    })
}

fn state_rank(state: &str) -> u8 {
    match state {
        "selected" => 0,
        "possible" => 1,
        "alternative" => 2,
        "excluded" => 3,
        _ => 4,
    }
}

fn relationship_index(datasets: &[&Dataset]) -> Value {
    let mut by_field = BTreeMap::<String, Vec<(&Dataset, BTreeSet<String>)>>::new();
    for dataset in datasets {
        for field in categorical_fields(dataset) {
            let values = field_values(dataset, &field);
            by_field.entry(field).or_default().push((*dataset, values));
        }
    }

    let mut join_keys = Vec::new();
    for (field, entries) in by_field {
        if entries.len() < 2 {
            continue;
        }
        let mut shared_values = entries[0].1.clone();
        for (_, values) in entries.iter().skip(1) {
            shared_values = shared_values
                .intersection(values)
                .cloned()
                .collect::<BTreeSet<_>>();
        }
        join_keys.push(json!({
            "field": field,
            "datasetIds": entries
                .iter()
                .map(|(dataset, _)| dataset.dataset_id.clone())
                .collect::<Vec<_>>(),
            "sharedValueCount": shared_values.len(),
            "sampleSharedValues": shared_values.into_iter().take(8).collect::<Vec<_>>()
        }));
    }

    json!({
        "strategy": "exact categorical field/value equality",
        "joinKeys": join_keys
    })
}

fn field_values(dataset: &Dataset, field: &str) -> BTreeSet<String> {
    let mut values = BTreeSet::new();
    for row in 0..dataset.row_count {
        let value = dataset.value(field, row);
        if !value.is_null() {
            values.insert(scalar_to_label(&value));
        }
    }
    values
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

    fn dataset(dataset_id: &str, records: Vec<BTreeMap<String, Value>>) -> Dataset {
        Dataset::from_request(IngestDatasetRequest {
            dataset_id: dataset_id.to_string(),
            display_name: None,
            replace: Some(true),
            records,
        })
        .expect("dataset builds")
    }

    #[test]
    fn selection_state_propagates_across_datasets_by_shared_field_values() {
        let sales = dataset(
            "sales",
            vec![
                record(vec![
                    ("region", Value::from("north")),
                    ("segment", Value::from("enterprise")),
                ]),
                record(vec![
                    ("region", Value::from("south")),
                    ("segment", Value::from("consumer")),
                ]),
            ],
        );
        let inventory = dataset(
            "inventory",
            vec![
                record(vec![
                    ("region", Value::from("north")),
                    ("category", Value::from("hardware")),
                ]),
                record(vec![
                    ("region", Value::from("south")),
                    ("category", Value::from("software")),
                ]),
            ],
        );
        let mut datasets = BTreeMap::new();
        datasets.insert(sales.dataset_id.clone(), sales);
        datasets.insert(inventory.dataset_id.clone(), inventory);

        let payload = selection_payload(
            &datasets,
            AssociativeSelectionRequest {
                dataset_ids: vec!["sales".to_string(), "inventory".to_string()],
                selections: vec![AssociativeSelection {
                    dataset_id: Some("sales".to_string()),
                    field: "region".to_string(),
                    value: Value::from("north"),
                }],
                max_values_per_field: Some(8),
            },
        )
        .expect("selection payload");

        assert_eq!(payload["ok"], true);
        assert_eq!(payload["datasetCount"], 2);
        assert_eq!(
            payload["relationshipIndex"]["joinKeys"][0]["field"],
            "region"
        );

        let fields = payload["fields"].as_array().expect("fields array");
        let inventory_category = fields
            .iter()
            .find(|field| field["datasetId"] == "inventory" && field["field"] == "category")
            .expect("inventory category state");
        let values = inventory_category["values"]
            .as_array()
            .expect("category values");

        assert!(values
            .iter()
            .any(|value| value["value"] == "hardware" && value["state"] == "possible"));
        assert!(values
            .iter()
            .any(|value| value["value"] == "software" && value["state"] == "excluded"));
    }
}
