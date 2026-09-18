use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{util::clean_identifier, VisualizationSpec};

const MAX_DASHBOARD_SPECS: usize = 24;
const MAX_DASHBOARD_FILTERS: usize = 64;
const MAX_DASHBOARD_TAGS: usize = 24;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SaveDashboardRequest {
    pub dashboard_id: String,
    pub title: String,
    pub description: Option<String>,
    pub owner: Option<String>,
    pub tags: Option<Vec<String>>,
    pub filters: Option<BTreeMap<String, Value>>,
    pub specs: Vec<VisualizationSpec>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SaveDashboardResponse {
    pub ok: bool,
    pub dashboard: SavedDashboard,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SavedDashboard {
    pub dashboard_id: String,
    pub title: String,
    pub description: Option<String>,
    pub owner: Option<String>,
    pub tags: Vec<String>,
    pub filters: BTreeMap<String, Value>,
    pub specs: Vec<VisualizationSpec>,
    pub created_at_ms: u128,
    pub updated_at_ms: u128,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DashboardSummary {
    pub dashboard_id: String,
    pub title: String,
    pub owner: Option<String>,
    pub tag_count: usize,
    pub spec_count: usize,
    pub created_at_ms: u128,
    pub updated_at_ms: u128,
}

impl SaveDashboardRequest {
    pub(crate) fn into_saved(self, now_ms: u128) -> Result<SavedDashboard, String> {
        let dashboard_id = clean_identifier(&self.dashboard_id).ok_or_else(|| {
            "dashboardId must contain letters, numbers, dash, underscore, dot, or colon".to_string()
        })?;
        let title = self.title.trim().to_string();
        if title.is_empty() || title.len() > 160 {
            return Err("dashboard title must be 1-160 characters".to_string());
        }
        if self.specs.is_empty() {
            return Err("dashboard requires at least one visualization spec".to_string());
        }
        if self.specs.len() > MAX_DASHBOARD_SPECS {
            return Err(format!("dashboard specs exceeds max {MAX_DASHBOARD_SPECS}"));
        }
        let filters = self.filters.unwrap_or_default();
        if filters.len() > MAX_DASHBOARD_FILTERS {
            return Err(format!(
                "dashboard filters exceeds max {MAX_DASHBOARD_FILTERS}"
            ));
        }
        let mut tags = self
            .tags
            .unwrap_or_default()
            .into_iter()
            .map(|tag| tag.trim().to_ascii_lowercase())
            .filter(|tag| !tag.is_empty())
            .collect::<Vec<_>>();
        tags.sort();
        tags.dedup();
        if tags.len() > MAX_DASHBOARD_TAGS {
            return Err(format!("dashboard tags exceeds max {MAX_DASHBOARD_TAGS}"));
        }

        Ok(SavedDashboard {
            dashboard_id,
            title,
            description: self.description,
            owner: self.owner,
            tags,
            filters,
            specs: self.specs,
            created_at_ms: now_ms,
            updated_at_ms: now_ms,
        })
    }
}

impl SavedDashboard {
    pub(crate) fn summary(&self) -> DashboardSummary {
        DashboardSummary {
            dashboard_id: self.dashboard_id.clone(),
            title: self.title.clone(),
            owner: self.owner.clone(),
            tag_count: self.tags.len(),
            spec_count: self.specs.len(),
            created_at_ms: self.created_at_ms,
            updated_at_ms: self.updated_at_ms,
        }
    }
}

pub(crate) fn catalog_payload(dashboards: Vec<DashboardSummary>) -> Value {
    serde_json::json!({
        "ok": true,
        "schemaVersion": "data-viz.saved-dashboards.v1",
        "dashboards": dashboards,
        "limits": {
            "maxSpecs": MAX_DASHBOARD_SPECS,
            "maxFilters": MAX_DASHBOARD_FILTERS,
            "maxTags": MAX_DASHBOARD_TAGS
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ChannelEncoding, FitnessBreakdown};

    fn spec() -> VisualizationSpec {
        VisualizationSpec {
            id: "candidate-0".to_string(),
            title: "Revenue by region".to_string(),
            dimension_count: 2,
            mark: "bar".to_string(),
            layout: "2d-cartesian".to_string(),
            projection: "direct-axis".to_string(),
            encodings: vec![ChannelEncoding {
                channel: "x".to_string(),
                field: "region".to_string(),
                data_type: "category".to_string(),
            }],
            transforms: vec!["profile columns".to_string()],
            fitness: FitnessBreakdown::default(),
            notes: Vec::new(),
        }
    }

    #[test]
    fn dashboard_request_validates_and_normalizes_tags() {
        let saved = SaveDashboardRequest {
            dashboard_id: "exec-sales".to_string(),
            title: "Executive Sales".to_string(),
            description: None,
            owner: Some("data-team".to_string()),
            tags: Some(vec!["Sales".to_string(), " sales ".to_string()]),
            filters: None,
            specs: vec![spec()],
        }
        .into_saved(123)
        .expect("dashboard saves");

        assert_eq!(saved.tags, vec!["sales"]);
        assert_eq!(saved.summary().spec_count, 1);
    }

    #[test]
    fn dashboard_request_rejects_empty_specs() {
        let error = SaveDashboardRequest {
            dashboard_id: "bad".to_string(),
            title: "Bad".to_string(),
            description: None,
            owner: None,
            tags: None,
            filters: None,
            specs: Vec::new(),
        }
        .into_saved(123)
        .expect_err("empty specs rejected");

        assert!(error.contains("at least one"));
    }
}
