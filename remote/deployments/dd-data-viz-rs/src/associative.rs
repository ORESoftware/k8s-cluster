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
const MAX_SELECTION_SESSIONS: usize = 256;
const MAX_SESSION_TAGS: usize = 24;
const MAX_SESSION_LABEL_BYTES: usize = 160;
const DEFAULT_MAX_RELATIONSHIPS: usize = 64;
const MAX_RELATIONSHIPS: usize = 512;

#[derive(Debug, Clone, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SaveSelectionSessionRequest {
    pub session_id: Option<String>,
    pub name: Option<String>,
    pub owner: Option<String>,
    pub tags: Option<Vec<String>>,
    pub selection: AssociativeSelectionRequest,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RelationshipDiscoveryRequest {
    pub dataset_ids: Vec<String>,
    pub max_relationships: Option<usize>,
    pub min_confidence: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SelectionSessionRecord {
    pub session_id: String,
    pub name: String,
    pub owner: Option<String>,
    pub tags: Vec<String>,
    pub dataset_ids: Vec<String>,
    pub selections: Vec<AssociativeSelection>,
    pub max_values_per_field: usize,
    pub created_at_ms: u128,
    pub updated_at_ms: u128,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SelectionSessionSummary {
    session_id: String,
    name: String,
    owner: Option<String>,
    dataset_count: usize,
    selection_count: usize,
    tag_count: usize,
    updated_at_ms: u128,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct NormalizedSelection {
    dataset_id: Option<String>,
    field: String,
    value: String,
    source: &'static str,
}

impl SaveSelectionSessionRequest {
    pub(crate) fn into_record(self, now_ms: u128) -> Result<SelectionSessionRecord, String> {
        let session_id = match self.session_id {
            Some(raw_id) => clean_identifier(&raw_id).ok_or_else(|| {
                "sessionId must contain letters, numbers, dash, underscore, dot, or colon"
                    .to_string()
            })?,
            None => format!("association-session-{now_ms}"),
        };
        let name = bounded_label(self.name.as_deref(), "selection session name")?
            .unwrap_or_else(|| "Associative selection session".to_string());
        let owner = bounded_label(self.owner.as_deref(), "selection session owner")?;
        let tags = normalize_tags(self.tags.unwrap_or_default())?;
        let dataset_ids = normalize_dataset_ids(&self.selection.dataset_ids)?;
        let max_values_per_field = self
            .selection
            .max_values_per_field
            .unwrap_or(DEFAULT_MAX_VALUES_PER_FIELD)
            .clamp(1, MAX_VALUES_PER_FIELD);
        if self.selection.selections.len() > MAX_SELECTIONS {
            return Err(format!("selections exceeds max {MAX_SELECTIONS}"));
        }
        let selections = normalize_session_selections(self.selection.selections)?;

        Ok(SelectionSessionRecord {
            session_id,
            name,
            owner,
            tags,
            dataset_ids,
            selections,
            max_values_per_field,
            created_at_ms: now_ms,
            updated_at_ms: now_ms,
        })
    }
}

impl SelectionSessionRecord {
    pub(crate) fn selection_request(&self) -> AssociativeSelectionRequest {
        AssociativeSelectionRequest {
            dataset_ids: self.dataset_ids.clone(),
            selections: self.selections.clone(),
            max_values_per_field: Some(self.max_values_per_field),
        }
    }

    pub(crate) fn summary(&self) -> SelectionSessionSummary {
        SelectionSessionSummary {
            session_id: self.session_id.clone(),
            name: self.name.clone(),
            owner: self.owner.clone(),
            dataset_count: self.dataset_ids.len(),
            selection_count: self.selections.len(),
            tag_count: self.tags.len(),
            updated_at_ms: self.updated_at_ms,
        }
    }
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

pub(crate) fn relationship_discovery_payload(
    datasets: &BTreeMap<String, Dataset>,
    request: RelationshipDiscoveryRequest,
) -> Result<Value, String> {
    let dataset_ids = normalize_dataset_ids(&request.dataset_ids)?;
    let max_relationships = request
        .max_relationships
        .unwrap_or(DEFAULT_MAX_RELATIONSHIPS)
        .clamp(1, MAX_RELATIONSHIPS);
    let min_confidence = request.min_confidence.unwrap_or(0.10).clamp(0.0, 1.0);
    let mut selected_datasets = Vec::with_capacity(dataset_ids.len());
    for dataset_id in &dataset_ids {
        let dataset = datasets
            .get(dataset_id)
            .ok_or_else(|| format!("dataset `{dataset_id}` not found"))?;
        selected_datasets.push(dataset);
    }

    let mut fields = Vec::new();
    for dataset in selected_datasets {
        for field in categorical_fields(dataset) {
            let values = field_values(dataset, &field);
            if values.is_empty() {
                continue;
            }
            fields.push(RelationshipField {
                dataset_id: dataset.dataset_id.clone(),
                field,
                values,
            });
        }
    }

    let mut candidates = Vec::new();
    for left_index in 0..fields.len() {
        for right_index in left_index + 1..fields.len() {
            let left = &fields[left_index];
            let right = &fields[right_index];
            if left.dataset_id == right.dataset_id {
                continue;
            }
            if let Some(candidate) = relationship_candidate(left, right, min_confidence) {
                candidates.push(candidate);
            }
        }
    }
    candidates.sort_by(|left, right| {
        right["confidence"]
            .as_f64()
            .unwrap_or(0.0)
            .total_cmp(&left["confidence"].as_f64().unwrap_or(0.0))
            .then_with(|| {
                right["sharedValueCount"]
                    .as_u64()
                    .cmp(&left["sharedValueCount"].as_u64())
            })
            .then_with(|| {
                left["left"]["field"]
                    .as_str()
                    .unwrap_or_default()
                    .cmp(right["left"]["field"].as_str().unwrap_or_default())
            })
    });
    candidates.truncate(max_relationships);

    Ok(json!({
        "ok": true,
        "schemaVersion": "data-viz.associative-relationships.v1",
        "datasetCount": dataset_ids.len(),
        "fieldCount": fields.len(),
        "relationshipCount": candidates.len(),
        "relationships": candidates,
        "scoring": {
            "valueJaccardWeight": 0.45,
            "coverageWeight": 0.35,
            "fieldNameWeight": 0.20,
            "aliasKinds": ["exact-name", "normalized-name", "suffix-match", "value-overlap", "weak"]
        },
        "limits": selection_limits_payload()
    }))
}

pub(crate) fn save_session_response(
    session: SelectionSessionRecord,
    selection_state: Value,
    warnings: Vec<String>,
) -> Value {
    json!({
        "ok": true,
        "schemaVersion": "data-viz.associative-session.v1",
        "session": session,
        "selectionState": selection_state,
        "warnings": warnings
    })
}

pub(crate) fn session_detail_payload(
    session: SelectionSessionRecord,
    selection_state: Value,
) -> Value {
    json!({
        "ok": true,
        "schemaVersion": "data-viz.associative-session.v1",
        "session": session,
        "selectionState": selection_state
    })
}

pub(crate) fn session_catalog_payload(sessions: Vec<SelectionSessionSummary>) -> Value {
    json!({
        "ok": true,
        "schemaVersion": "data-viz.associative-session-catalog.v1",
        "sessions": sessions,
        "count": sessions.len(),
        "limits": selection_limits_payload()
    })
}

pub(crate) fn max_selection_sessions() -> usize {
    MAX_SELECTION_SESSIONS
}

pub(crate) fn selection_limits_payload() -> Value {
    json!({
        "maxDatasets": MAX_SELECTION_DATASETS,
        "maxSelections": MAX_SELECTIONS,
        "defaultMaxValuesPerField": DEFAULT_MAX_VALUES_PER_FIELD,
        "maxValuesPerField": MAX_VALUES_PER_FIELD,
        "maxSessions": MAX_SELECTION_SESSIONS,
        "maxSessionTags": MAX_SESSION_TAGS,
        "maxSessionLabelBytes": MAX_SESSION_LABEL_BYTES,
        "defaultMaxRelationships": DEFAULT_MAX_RELATIONSHIPS,
        "maxRelationships": MAX_RELATIONSHIPS
    })
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

fn normalize_session_selections(
    selections: Vec<AssociativeSelection>,
) -> Result<Vec<AssociativeSelection>, String> {
    selections
        .into_iter()
        .map(|selection| {
            let dataset_id = selection
                .dataset_id
                .map(|raw_id| {
                    clean_identifier(&raw_id)
                        .ok_or_else(|| "selection datasetId is invalid".to_string())
                })
                .transpose()?;
            let field = clean_identifier(&selection.field)
                .ok_or_else(|| "selection field is invalid".to_string())?;
            if !is_scalar_selection_value(&selection.value) {
                return Err("selection value must be a scalar JSON value".to_string());
            }
            if selection.value.is_null() {
                return Err("selection value cannot be null".to_string());
            }
            Ok(AssociativeSelection {
                dataset_id,
                field,
                value: selection.value,
            })
        })
        .collect()
}

fn bounded_label(value: Option<&str>, label: &str) -> Result<Option<String>, String> {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    if value.len() > MAX_SESSION_LABEL_BYTES {
        return Err(format!(
            "{label} exceeds max {MAX_SESSION_LABEL_BYTES} bytes"
        ));
    }
    if looks_secret_bearing(value) {
        return Err(format!("{label} appears to contain secret-bearing text"));
    }
    Ok(Some(value.to_string()))
}

fn normalize_tags(tags: Vec<String>) -> Result<Vec<String>, String> {
    if tags.len() > MAX_SESSION_TAGS {
        return Err(format!("tags exceeds max {MAX_SESSION_TAGS}"));
    }
    let mut normalized = BTreeSet::new();
    for tag in tags {
        let tag = clean_identifier(&tag).ok_or_else(|| {
            "tags must contain letters, numbers, dash, underscore, dot, or colon".to_string()
        })?;
        normalized.insert(tag);
    }
    Ok(normalized.into_iter().collect())
}

fn is_scalar_selection_value(value: &Value) -> bool {
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

#[derive(Debug)]
struct RelationshipField {
    dataset_id: String,
    field: String,
    values: BTreeSet<String>,
}

fn relationship_candidate(
    left: &RelationshipField,
    right: &RelationshipField,
    min_confidence: f64,
) -> Option<Value> {
    let shared = left
        .values
        .intersection(&right.values)
        .cloned()
        .collect::<BTreeSet<_>>();
    if shared.is_empty() {
        return None;
    }
    let union_count = left.values.union(&right.values).count().max(1);
    let value_jaccard = shared.len() as f64 / union_count as f64;
    let left_coverage = shared.len() as f64 / left.values.len().max(1) as f64;
    let right_coverage = shared.len() as f64 / right.values.len().max(1) as f64;
    let coverage = (left_coverage + right_coverage) / 2.0;
    let field_name_score = field_name_similarity(&left.field, &right.field);
    let confidence = round4(value_jaccard * 0.45 + coverage * 0.35 + field_name_score * 0.20);
    if confidence < min_confidence {
        return None;
    }
    let alias_kind = alias_kind(&left.field, &right.field, value_jaccard);
    let shared_value_count = shared.len();
    let sample_shared_values = shared.into_iter().take(12).collect::<Vec<_>>();

    Some(json!({
        "left": {
            "datasetId": left.dataset_id,
            "field": left.field,
            "cardinality": left.values.len()
        },
        "right": {
            "datasetId": right.dataset_id,
            "field": right.field,
            "cardinality": right.values.len()
        },
        "aliasKind": alias_kind,
        "confidence": confidence,
        "strength": relationship_strength(confidence),
        "sharedValueCount": shared_value_count,
        "sampleSharedValues": sample_shared_values,
        "scoreComponents": {
            "valueJaccard": round4(value_jaccard),
            "leftCoverage": round4(left_coverage),
            "rightCoverage": round4(right_coverage),
            "fieldNameSimilarity": round4(field_name_score)
        }
    }))
}

fn relationship_strength(confidence: f64) -> &'static str {
    if confidence >= 0.75 {
        "strong"
    } else if confidence >= 0.45 {
        "medium"
    } else {
        "tentative"
    }
}

fn alias_kind(left: &str, right: &str, value_jaccard: f64) -> &'static str {
    let left_lower = left.to_ascii_lowercase();
    let right_lower = right.to_ascii_lowercase();
    if left_lower == right_lower {
        return "exact-name";
    }
    let left_canonical = canonical_field_name(left);
    let right_canonical = canonical_field_name(right);
    if left_canonical == right_canonical {
        return "normalized-name";
    }
    if token_subset_match(left, right)
        || left_canonical.ends_with(&right_canonical)
        || right_canonical.ends_with(&left_canonical)
    {
        return "suffix-match";
    }
    if value_jaccard >= 0.50 {
        "value-overlap"
    } else {
        "weak"
    }
}

fn field_name_similarity(left: &str, right: &str) -> f64 {
    let left_lower = left.to_ascii_lowercase();
    let right_lower = right.to_ascii_lowercase();
    if left_lower == right_lower {
        return 1.0;
    }
    let left_canonical = canonical_field_name(left);
    let right_canonical = canonical_field_name(right);
    if left_canonical == right_canonical {
        return 0.95;
    }
    if token_subset_match(left, right)
        || left_canonical.ends_with(&right_canonical)
        || right_canonical.ends_with(&left_canonical)
    {
        return 0.80;
    }
    let left_tokens = field_tokens(left);
    let right_tokens = field_tokens(right);
    if left_tokens.is_empty() || right_tokens.is_empty() {
        return 0.0;
    }
    let shared = left_tokens.intersection(&right_tokens).count();
    let union = left_tokens.union(&right_tokens).count().max(1);
    shared as f64 / union as f64
}

fn canonical_field_name(field: &str) -> String {
    field_tokens(field)
        .into_iter()
        .collect::<Vec<_>>()
        .join("_")
}

fn token_subset_match(left: &str, right: &str) -> bool {
    let left_tokens = field_tokens(left);
    let right_tokens = field_tokens(right);
    !left_tokens.is_empty()
        && !right_tokens.is_empty()
        && (left_tokens.is_subset(&right_tokens) || right_tokens.is_subset(&left_tokens))
}

fn field_tokens(field: &str) -> BTreeSet<String> {
    field
        .to_ascii_lowercase()
        .split(|ch: char| !(ch.is_ascii_alphanumeric()))
        .filter(|token| !token.is_empty())
        .filter(|token| {
            !matches!(
                *token,
                "id" | "key" | "code" | "name" | "dim" | "fact" | "src" | "source"
            )
        })
        .map(str::to_string)
        .collect()
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

    #[test]
    fn selection_session_normalizes_and_summarizes_request() {
        let record = SaveSelectionSessionRequest {
            session_id: Some("exec-session".to_string()),
            name: Some("Executive Region".to_string()),
            owner: Some("analyst".to_string()),
            tags: Some(vec!["sales".to_string(), "sales".to_string()]),
            selection: AssociativeSelectionRequest {
                dataset_ids: vec!["sales".to_string(), "inventory".to_string()],
                selections: vec![AssociativeSelection {
                    dataset_id: Some("sales".to_string()),
                    field: "region".to_string(),
                    value: Value::from("north"),
                }],
                max_values_per_field: Some(12),
            },
        }
        .into_record(42)
        .expect("session record");

        assert_eq!(record.session_id, "exec-session");
        assert_eq!(record.dataset_ids.len(), 2);
        assert_eq!(record.tags, vec!["sales"]);
        assert_eq!(record.selection_request().max_values_per_field, Some(12));
        assert_eq!(record.summary().selection_count, 1);
    }

    #[test]
    fn selection_session_rejects_secret_like_owner() {
        let error = SaveSelectionSessionRequest {
            session_id: None,
            name: Some("Daily selection".to_string()),
            owner: Some("token owner".to_string()),
            tags: None,
            selection: AssociativeSelectionRequest {
                dataset_ids: vec!["sales".to_string()],
                selections: Vec::new(),
                max_values_per_field: None,
            },
        }
        .into_record(42)
        .expect_err("secret-like owner rejected");

        assert!(error.contains("secret-bearing"));
    }

    #[test]
    fn relationship_discovery_scores_field_aliases_by_values_and_names() {
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
                    ("sales_region", Value::from("north")),
                    ("category", Value::from("hardware")),
                ]),
                record(vec![
                    ("sales_region", Value::from("south")),
                    ("category", Value::from("software")),
                ]),
            ],
        );
        let mut datasets = BTreeMap::new();
        datasets.insert(sales.dataset_id.clone(), sales);
        datasets.insert(inventory.dataset_id.clone(), inventory);

        let payload = relationship_discovery_payload(
            &datasets,
            RelationshipDiscoveryRequest {
                dataset_ids: vec!["sales".to_string(), "inventory".to_string()],
                max_relationships: Some(8),
                min_confidence: Some(0.20),
            },
        )
        .expect("relationship discovery");

        assert_eq!(payload["ok"], true);
        let relationships = payload["relationships"]
            .as_array()
            .expect("relationships array");
        let region_alias = relationships
            .iter()
            .find(|relationship| {
                relationship["left"]["field"] == "region"
                    && relationship["right"]["field"] == "sales_region"
            })
            .expect("region alias relationship");
        assert_eq!(region_alias["aliasKind"], "suffix-match");
        assert_eq!(region_alias["sharedValueCount"], 2);
        assert!(region_alias["confidence"].as_f64().unwrap() >= 0.80);
    }
}
