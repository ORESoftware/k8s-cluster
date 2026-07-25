use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct IngestRequest {
    pub(crate) request_id: Option<String>,
    pub(crate) schema_version: Option<String>,
    pub(crate) dataset_id: Option<String>,
    pub(crate) source: String,
    pub(crate) source_url: Option<String>,
    pub(crate) tags: Option<Vec<String>>,
    pub(crate) records: Vec<IncomingRecord>,
    pub(crate) pipeline: Option<PipelineOptions>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct IncomingRecord {
    pub(crate) record_id: Option<String>,
    pub(crate) dataset_id: Option<String>,
    pub(crate) source: Option<String>,
    pub(crate) source_url: Option<String>,
    pub(crate) title: Option<String>,
    pub(crate) summary: Option<String>,
    pub(crate) published_at: Option<String>,
    pub(crate) authors: Option<Vec<String>>,
    pub(crate) tags: Option<Vec<String>>,
    pub(crate) metrics: Option<BTreeMap<String, f64>>,
    pub(crate) grant: Option<GrantOpportunity>,
    pub(crate) raw: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DataRecord {
    pub(crate) record_id: String,
    pub(crate) dataset_id: String,
    pub(crate) source: String,
    pub(crate) source_url: Option<String>,
    pub(crate) title: Option<String>,
    pub(crate) summary: Option<String>,
    pub(crate) published_at: Option<String>,
    pub(crate) collected_at_ms: u128,
    pub(crate) authors: Vec<String>,
    pub(crate) tags: Vec<String>,
    pub(crate) metrics: BTreeMap<String, f64>,
    pub(crate) grant: Option<GrantOpportunity>,
    pub(crate) raw: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GrantOpportunity {
    pub(crate) grant_id: Option<String>,
    pub(crate) title: String,
    pub(crate) agency: Option<String>,
    pub(crate) program: Option<String>,
    pub(crate) amount: Option<f64>,
    pub(crate) due_date: Option<String>,
    pub(crate) eligibility: Option<String>,
    pub(crate) topics: Vec<String>,
    pub(crate) url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ScrapeRequest {
    pub(crate) request_id: Option<String>,
    pub(crate) source: String,
    pub(crate) url: String,
    pub(crate) dataset_id: Option<String>,
    pub(crate) strategy: Option<String>,
    pub(crate) render_javascript: Option<bool>,
    pub(crate) selector: Option<String>,
    pub(crate) selectors: Option<BTreeMap<String, String>>,
    pub(crate) include_links: Option<bool>,
    pub(crate) tags: Option<Vec<String>>,
    pub(crate) pipeline: Option<PipelineOptions>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct UiScrapeForm {
    pub(crate) source: Option<String>,
    pub(crate) url: String,
    pub(crate) dataset_id: Option<String>,
    pub(crate) strategy: Option<String>,
    pub(crate) selector: Option<String>,
    pub(crate) tags: Option<String>,
    pub(crate) render_javascript: Option<String>,
    pub(crate) include_links: Option<String>,
    pub(crate) pipeline_enabled: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ScraperExtraction {
    pub(crate) title: Option<String>,
    pub(crate) text: Option<String>,
    pub(crate) fields: Option<BTreeMap<String, String>>,
    pub(crate) links: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ScraperResponse {
    pub(crate) ok: bool,
    pub(crate) request_id: Option<String>,
    pub(crate) url: Option<String>,
    pub(crate) final_url: Option<String>,
    pub(crate) status: Option<u16>,
    pub(crate) content_type: Option<String>,
    pub(crate) duration_ms: Option<u64>,
    pub(crate) extraction: Option<ScraperExtraction>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WebhookIngestRequest {
    pub(crate) request_id: Option<String>,
    pub(crate) provider: String,
    pub(crate) event_type: Option<String>,
    pub(crate) dataset_id: Option<String>,
    pub(crate) source_url: Option<String>,
    pub(crate) payload: Value,
    pub(crate) records: Option<Vec<IncomingRecord>>,
    pub(crate) pipeline: Option<PipelineOptions>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WebhookReceipt {
    pub(crate) receipt_id: String,
    pub(crate) provider: String,
    pub(crate) event_type: String,
    pub(crate) dataset_id: Option<String>,
    pub(crate) source_url: Option<String>,
    pub(crate) received_at_ms: u128,
    pub(crate) record_count: usize,
    pub(crate) payload_shape: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PipelineOptions {
    pub(crate) enabled: Option<bool>,
    pub(crate) job_type: Option<String>,
    pub(crate) sink: Option<String>,
    pub(crate) airflow_dag: Option<String>,
    pub(crate) spark_app: Option<String>,
    pub(crate) parameters: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PipelineRequest {
    pub(crate) request_id: Option<String>,
    pub(crate) job_type: Option<String>,
    pub(crate) dataset_ids: Option<Vec<String>>,
    pub(crate) analysis_ids: Option<Vec<String>>,
    pub(crate) sink: Option<String>,
    pub(crate) airflow_dag: Option<String>,
    pub(crate) spark_app: Option<String>,
    pub(crate) parameters: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PipelineJob {
    pub(crate) job_id: String,
    pub(crate) request_id: String,
    pub(crate) job_type: String,
    pub(crate) status: String,
    pub(crate) dataset_ids: Vec<String>,
    pub(crate) analysis_ids: Vec<String>,
    pub(crate) sink: String,
    pub(crate) airflow_dag: Option<String>,
    pub(crate) spark_app: Option<String>,
    pub(crate) parameters: Value,
    pub(crate) submitted_at_ms: u128,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GrantMatchRequest {
    pub(crate) request_id: Option<String>,
    pub(crate) applicant_profile: String,
    pub(crate) focus_areas: Vec<String>,
    pub(crate) dataset_ids: Option<Vec<String>>,
    pub(crate) min_amount: Option<f64>,
    pub(crate) limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GrantMatch {
    pub(crate) record_id: String,
    pub(crate) dataset_id: String,
    pub(crate) source: String,
    pub(crate) title: String,
    pub(crate) url: Option<String>,
    pub(crate) agency: Option<String>,
    pub(crate) program: Option<String>,
    pub(crate) amount: Option<f64>,
    pub(crate) due_date: Option<String>,
    pub(crate) score: f64,
    pub(crate) reasons: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AnalysisRequest {
    pub(crate) request_id: Option<String>,
    pub(crate) dataset_ids: Option<Vec<String>>,
    pub(crate) metrics: Option<Vec<String>>,
    pub(crate) tags: Option<Vec<String>>,
    pub(crate) limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AnalysisResult {
    pub(crate) analysis_id: String,
    pub(crate) request_id: String,
    pub(crate) kind: String,
    pub(crate) generated_at_ms: u128,
    pub(crate) dataset_ids: Vec<String>,
    pub(crate) summary: String,
    pub(crate) graph: GraphData,
    pub(crate) trends: Vec<TrendSummary>,
    pub(crate) correlations: Vec<CorrelationSummary>,
    pub(crate) grants: Vec<GrantMatch>,
    pub(crate) model_notes: Vec<ModelNote>,
    pub(crate) markdown: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GraphData {
    pub(crate) graph_type: String,
    pub(crate) title: String,
    pub(crate) x_label: String,
    pub(crate) y_label: String,
    pub(crate) series: Vec<GraphSeries>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GraphSeries {
    pub(crate) name: String,
    pub(crate) points: Vec<GraphPoint>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GraphPoint {
    pub(crate) x: f64,
    pub(crate) y: f64,
    pub(crate) label: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TrendSummary {
    pub(crate) metric: String,
    pub(crate) count: usize,
    pub(crate) mean: f64,
    pub(crate) min: f64,
    pub(crate) max: f64,
    pub(crate) slope_per_record: f64,
    pub(crate) direction: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CorrelationSummary {
    pub(crate) left_metric: String,
    pub(crate) right_metric: String,
    pub(crate) count: usize,
    pub(crate) pearson: f64,
    pub(crate) strength: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ModelNote {
    pub(crate) name: String,
    pub(crate) equation: String,
    pub(crate) use_case: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WhitePaperRequest {
    pub(crate) request_id: Option<String>,
    pub(crate) title: Option<String>,
    pub(crate) research_question: String,
    pub(crate) dataset_ids: Option<Vec<String>>,
    pub(crate) focus_areas: Option<Vec<String>>,
    pub(crate) include_grants: Option<bool>,
    pub(crate) limit: Option<usize>,
}

pub(crate) enum AuthFailure {
    MissingSecret,
    Unauthorized,
}
