use std::{
    collections::{BTreeMap, BTreeSet},
    env,
    net::SocketAddr,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, RwLock,
    },
};

use axum::{
    extract::{DefaultBodyLimit, Path, State},
    http::{HeaderMap, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

mod alerts;
mod associative;
mod dashboard;
mod hardening;
mod platform;
mod rbac;
mod semantic;
mod sql_frontend;
mod util;

use util::{
    clean_field, clean_identifier, env_flag, find_ascii_case, header_value, html_escape, now_ms,
    round4, scalar_to_label, xml_escape,
};

const SERVICE_NAME: &str = "dd-data-viz-rs";
const SCHEMA_VERSION: &str = "data-viz.analytics.v1";
const DEFAULT_HOST: &str = "0.0.0.0";
const DEFAULT_PORT: u16 = 8126;
const MAX_HTTP_BODY_BYTES: usize = 4 * 1024 * 1024;
const MAX_DATASETS: usize = 64;
const MAX_RECORDS: usize = 50_000;
const MAX_COLUMNS: usize = 192;
const MAX_QUERY_ROWS: usize = 5_000;
const MAX_EVOLUTION_POPULATION: usize = 96;
const MAX_EVOLUTION_GENERATIONS: usize = 32;

#[derive(Clone)]
struct AppState {
    config: Arc<Config>,
    metrics: Arc<Metrics>,
    datasets: Arc<RwLock<BTreeMap<String, Dataset>>>,
    evolution_runs: Arc<RwLock<BTreeMap<String, EvolutionRunRecord>>>,
    dashboards: Arc<RwLock<BTreeMap<String, dashboard::SavedDashboard>>>,
    alert_rules: Arc<RwLock<BTreeMap<String, alerts::AlertRule>>>,
    semantic_models: Arc<RwLock<BTreeMap<String, semantic::SavedSemanticModel>>>,
}

#[derive(Clone)]
struct Config {
    host: String,
    port: u16,
    server_auth_secret: Option<String>,
    allow_unauthenticated: bool,
}

#[derive(Default)]
struct Metrics {
    http_requests_total: AtomicU64,
    datasets_ingested_total: AtomicU64,
    queries_total: AtomicU64,
    visualizations_total: AtomicU64,
    evolution_runs_total: AtomicU64,
    presentation_exports_total: AtomicU64,
    platform_requests_total: AtomicU64,
    hardening_requests_total: AtomicU64,
    association_requests_total: AtomicU64,
    dashboard_requests_total: AtomicU64,
    dashboards_saved_total: AtomicU64,
    alert_requests_total: AtomicU64,
    alert_rules_saved_total: AtomicU64,
    alert_evaluations_total: AtomicU64,
    semantic_requests_total: AtomicU64,
    semantic_models_saved_total: AtomicU64,
    rbac_denials_total: AtomicU64,
    auth_failures_total: AtomicU64,
    errors_total: AtomicU64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IngestDatasetRequest {
    dataset_id: String,
    display_name: Option<String>,
    replace: Option<bool>,
    records: Vec<BTreeMap<String, Value>>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct IngestDatasetResponse {
    ok: bool,
    dataset: DatasetMetadata,
    warnings: Vec<String>,
}

#[derive(Debug, Clone)]
struct Dataset {
    dataset_id: String,
    display_name: String,
    row_count: usize,
    columns: BTreeMap<String, Column>,
    created_at_ms: u128,
    updated_at_ms: u128,
}

#[derive(Debug, Clone)]
enum Column {
    Number(Vec<Option<f64>>),
    Dictionary {
        dictionary: Vec<String>,
        codes: Vec<Option<u32>>,
    },
    Boolean(Vec<Option<bool>>),
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct DatasetMetadata {
    dataset_id: String,
    display_name: String,
    row_count: usize,
    column_count: usize,
    created_at_ms: u128,
    updated_at_ms: u128,
    columns: Vec<ColumnProfile>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ColumnProfile {
    name: String,
    data_type: String,
    missing_count: usize,
    numeric: Option<NumericProfile>,
    categorical: Option<CategoricalProfile>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct NumericProfile {
    min: f64,
    max: f64,
    mean: f64,
    variance: f64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct CategoricalProfile {
    cardinality: usize,
    top_values: Vec<CategoryCount>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct CategoryCount {
    value: String,
    count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct QueryRequest {
    dialect: QueryDialect,
    query: String,
    dataset_id: Option<String>,
    limit: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
enum QueryDialect {
    #[serde(rename = "sql")]
    Sql,
    #[serde(rename = "graphql")]
    GraphQl,
    #[serde(rename = "promql")]
    PromQl,
    #[serde(rename = "flux")]
    Flux,
    #[serde(rename = "influxql")]
    InfluxQl,
    #[serde(rename = "logql")]
    LogQl,
    #[serde(rename = "cypher")]
    Cypher,
    #[serde(rename = "gremlin")]
    Gremlin,
    #[serde(rename = "mongo")]
    Mongo,
    #[serde(rename = "jmespath")]
    JmesPath,
    #[serde(rename = "lucene")]
    Lucene,
    #[serde(rename = "spl")]
    Spl,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
enum AggregationOp {
    Count,
    Sum,
    Avg,
    Min,
    Max,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AggregationExpr {
    alias: String,
    op: AggregationOp,
    field: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FilterExpr {
    field: String,
    op: String,
    value: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LogicalPlan {
    schema_version: String,
    dialect: QueryDialect,
    source: String,
    projections: Vec<String>,
    filter: Option<FilterExpr>,
    group_by: Vec<String>,
    aggregations: Vec<AggregationExpr>,
    limit: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct QueryResponse {
    ok: bool,
    logical_plan: LogicalPlan,
    rows: Vec<BTreeMap<String, Value>>,
    row_count: usize,
    warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct VisualizationRequest {
    dataset_id: String,
    query: Option<QueryRequest>,
    dimensions: Option<Vec<String>>,
    target_dimensions: Option<usize>,
    intent: Option<String>,
    max_candidates: Option<usize>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct VisualizationResponse {
    ok: bool,
    dataset: DatasetMetadata,
    logical_plan: Option<LogicalPlan>,
    candidates: Vec<VisualizationSpec>,
    warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct VisualizationSpec {
    id: String,
    title: String,
    dimension_count: usize,
    mark: String,
    layout: String,
    projection: String,
    encodings: Vec<ChannelEncoding>,
    transforms: Vec<String>,
    fitness: FitnessBreakdown,
    notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ChannelEncoding {
    channel: String,
    field: String,
    data_type: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FitnessBreakdown {
    total: f64,
    information_density: f64,
    legibility: f64,
    novelty: f64,
    task_fit: f64,
    ai_evaluator: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct EvolutionRequest {
    dataset_id: String,
    dimensions: Option<Vec<String>>,
    objective: Option<String>,
    population_size: Option<usize>,
    generations: Option<usize>,
    seed: Option<u64>,
    ai_evaluations: Option<Vec<AiEvaluation>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AiEvaluation {
    candidate_id: String,
    score: f64,
    rationale: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct EvolutionResponse {
    ok: bool,
    run_id: String,
    objective: String,
    best: VisualizationSpec,
    population: Vec<VisualizationSpec>,
    generations: Vec<EvolutionGeneration>,
    evaluator_prompt: String,
    warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct EvolutionGeneration {
    generation: usize,
    best_candidate_id: String,
    best_score: f64,
    average_score: f64,
    mutation_summary: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct EvolutionRunRecord {
    run_id: String,
    dataset_id: String,
    objective: String,
    created_at_ms: u128,
    best_candidate_id: String,
    best_score: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PresentationExportRequest {
    title: String,
    subtitle: Option<String>,
    narrative: Option<Vec<String>>,
    specs: Vec<VisualizationSpec>,
    format: Option<PresentationFormat>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
enum PresentationFormat {
    #[serde(rename = "all")]
    All,
    #[serde(rename = "powerpoint-openxml")]
    PowerPointOpenXml,
    #[serde(rename = "google-slides")]
    GoogleSlides,
    #[serde(rename = "reveal-markdown")]
    RevealMarkdown,
    #[serde(rename = "final-layer-json")]
    FinalLayerJson,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct PresentationExportResponse {
    ok: bool,
    format: PresentationFormat,
    slides: Vec<PresentationSlide>,
    powerpoint_open_xml: Option<BTreeMap<String, String>>,
    google_slides_batch_update: Option<Value>,
    reveal_markdown: Option<String>,
    final_layers: Value,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct PresentationSlide {
    slide_id: String,
    title: String,
    body: Vec<String>,
    visual_spec_id: Option<String>,
    speaker_notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct RouteDoc {
    method: &'static str,
    path: &'static str,
    auth: &'static str,
    description: &'static str,
}

#[derive(Debug)]
struct ApiError {
    status: StatusCode,
    message: String,
}

impl ApiError {
    fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: message.into(),
        }
    }

    fn unauthorized(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
            message: message.into(),
        }
    }

    fn not_found(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            message: message.into(),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(json!({
                "ok": false,
                "error": self.message,
            })),
        )
            .into_response()
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = Config::from_env();
    let bind_addr: SocketAddr = format!("{}:{}", config.host, config.port).parse()?;
    let state = AppState {
        config: Arc::new(config),
        metrics: Arc::new(Metrics::default()),
        datasets: Arc::new(RwLock::new(BTreeMap::new())),
        evolution_runs: Arc::new(RwLock::new(BTreeMap::new())),
        dashboards: Arc::new(RwLock::new(BTreeMap::new())),
        alert_rules: Arc::new(RwLock::new(BTreeMap::new())),
        semantic_models: Arc::new(RwLock::new(BTreeMap::new())),
    };
    let app = app_router(state);
    let listener = tokio::net::TcpListener::bind(bind_addr).await?;

    log_event(
        "INFO",
        "data_viz.startup",
        "dd-data-viz-rs listening",
        json!({ "addr": bind_addr.to_string() }),
    );

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

fn app_router(state: AppState) -> Router {
    Router::new()
        .route("/", get(home))
        .route("/descriptor", get(descriptor))
        .route("/dialects", get(dialects))
        .route("/schema", get(schema))
        .route("/example", get(example))
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .route("/metrics", get(metrics))
        .route("/capabilities/parity", get(platform_capabilities))
        .route("/connectors/catalog", get(connector_catalog))
        .route("/semantic/models", get(semantic_models))
        .route(
            "/semantic/registry",
            get(list_semantic_registry).post(save_semantic_model),
        )
        .route("/semantic/registry/:model_id", get(get_semantic_model))
        .route(
            "/semantic/registry/:model_id/compile",
            post(compile_semantic_model),
        )
        .route("/workbooks/blueprints", get(workbook_blueprints))
        .route("/dashboards/panels", get(dashboard_panels))
        .route("/renderers/contracts", get(renderer_contracts))
        .route("/reports/evidence", get(evidence_report_blueprint))
        .route("/security/policy", get(security_policy))
        .route("/security/rbac", get(rbac_policy))
        .route("/associations/:dataset_id", get(association_graph))
        .route("/associations/select", post(association_selection))
        .route("/dashboards", get(list_dashboards).post(save_dashboard))
        .route("/dashboards/:dashboard_id", get(get_dashboard))
        .route("/alerts/rules", get(list_alert_rules).post(save_alert_rule))
        .route("/alerts/rules/:rule_id", get(get_alert_rule))
        .route("/alerts/rules/:rule_id/evaluate", post(evaluate_alert_rule))
        .route("/datasets", get(list_datasets).post(ingest_dataset))
        .route("/datasets/:dataset_id", get(get_dataset))
        .route("/query", post(query))
        .route("/visualizations/suggest", post(suggest_visualizations))
        .route("/evolution/run", post(run_evolution))
        .route("/evolution/runs", get(list_evolution_runs))
        .route("/presentations/export", post(export_presentation))
        .route("/docs/api", get(docs_html))
        .route("/api/docs", get(docs_html))
        .route("/api/docs.json", get(docs_json))
        .layer(DefaultBodyLimit::max(MAX_HTTP_BODY_BYTES))
        .with_state(state)
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}

impl Config {
    fn from_env() -> Self {
        let host = env::var("HOST").unwrap_or_else(|_| DEFAULT_HOST.to_string());
        let port = env::var("PORT")
            .ok()
            .and_then(|value| value.parse::<u16>().ok())
            .unwrap_or(DEFAULT_PORT);
        let server_auth_secret = env::var("SERVER_AUTH_SECRET")
            .ok()
            .filter(|value| !value.trim().is_empty());
        let allow_unauthenticated = env_flag("DATA_VIZ_ALLOW_UNAUTHENTICATED", false);

        Self {
            host,
            port,
            server_auth_secret,
            allow_unauthenticated,
        }
    }
}

async fn home(State(state): State<AppState>) -> Html<String> {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    Html(format!(
        r#"<!doctype html>
<html>
<head>
  <meta charset="utf-8">
  <title>{service}</title>
  <style>
    body {{ font-family: system-ui, sans-serif; margin: 2rem; max-width: 960px; }}
    code {{ background: #f4f4f5; padding: 0.1rem 0.25rem; border-radius: 4px; }}
    li {{ margin: 0.25rem 0; }}
  </style>
</head>
<body>
  <h1>{service}</h1>
  <p>Columnar analytics, multi-dialect query translation, evolutionary visualization search, and presentation-layer export.</p>
  <ul>
    <li><code>POST /datasets</code> ingests records into a columnar in-memory store.</li>
    <li><code>POST /query</code> translates SQL, GraphQL, PromQL, Flux, InfluxQL, LogQL, Cypher, Gremlin, Mongo, JMESPath, Lucene, and SPL into one logical plan.</li>
    <li><code>POST /visualizations/suggest</code> creates 2D, 3D, 4D, 5D, and XD visual specs.</li>
    <li><code>POST /evolution/run</code> mutates and scores visualization genomes with optional AI evaluator feedback.</li>
    <li><code>POST /presentations/export</code> emits PowerPoint OpenXML package layers, Google Slides batch operations, Reveal markdown, and final JSON layers.</li>
  </ul>
  <p>See <a href="/docs/api">/docs/api</a> and <a href="/example">/example</a>.</p>
</body>
</html>"#,
        service = SERVICE_NAME
    ))
}

async fn descriptor(State(state): State<AppState>) -> Json<Value> {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    Json(json!({
        "ok": true,
        "service": SERVICE_NAME,
        "schemaVersion": SCHEMA_VERSION,
        "storage": {
            "mode": "in-memory-columnar",
            "numericColumns": "Vec<Option<f64>>",
            "categoricalColumns": "dictionary-encoded strings with u32 codes",
            "booleanColumns": "Vec<Option<bool>>",
            "maxDatasets": MAX_DATASETS,
            "maxRecordsPerDataset": MAX_RECORDS,
            "maxColumnsPerDataset": MAX_COLUMNS
        },
        "queryCore": {
            "intermediateRepresentation": "LogicalPlan",
            "dialects": dialect_catalog(),
            "execution": "single-process vector-friendly column scan with grouped aggregations",
            "futureAccelerators": ["Apache Arrow", "DataFusion", "SIMD kernels", "Rayon chunk reducers"]
        },
        "visualizationCore": {
            "dimensions": ["2d", "3d", "4d", "5d", "xd"],
            "encodings": ["x", "y", "z", "color", "size", "shape", "time", "facet", "hyperSlice"],
            "evolution": ["mutation", "crossover-style channel rotation", "fitness scoring", "optional AI evaluator feedback"]
        },
        "platformParity": {
            "products": platform::parity_matrix(),
            "semanticModels": platform::semantic_models(),
            "connectors": platform::connector_catalog(),
            "workbooks": platform::workbook_blueprints(),
            "etl": platform::etl_primitives(),
            "dashboardPanels": platform::dashboard_panel_catalog(),
            "rendererContracts": platform::renderer_contracts(),
            "selfService": platform::self_service_surfaces()
        },
        "presentationLayers": ["powerpoint-openxml", "google-slides", "reveal-markdown", "final-layer-json"],
        "hardening": hardening::hardening_payload(
            MAX_DATASETS,
            MAX_RECORDS,
            MAX_COLUMNS,
            MAX_QUERY_ROWS,
            MAX_HTTP_BODY_BYTES,
            state.config.server_auth_secret.is_some(),
            state.config.allow_unauthenticated,
        ),
        "auth": {
            "operatorHeaders": ["X-Server-Auth", "Auth", "Authorization: Bearer ..."],
            "roleHeaders": ["X-Data-Viz-Role", "X-DD-Role"],
            "rbacPolicy": rbac::policy_catalog(),
            "allowUnauthenticated": state.config.allow_unauthenticated,
            "secretConfigured": state.config.server_auth_secret.is_some()
        },
        "routes": route_docs()
    }))
}

async fn dialects(State(state): State<AppState>) -> Json<Value> {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    Json(json!({
        "ok": true,
        "dialects": dialect_catalog()
    }))
}

async fn schema(State(state): State<AppState>) -> Json<Value> {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    Json(json!({
        "ok": true,
        "schemaVersion": SCHEMA_VERSION,
        "datasetRecord": {
            "datasetId": "string",
            "displayName": "optional string",
            "replace": "optional bool",
            "records": ["object rows with scalar JSON values"]
        },
        "logicalPlan": {
            "source": "dataset id",
            "projections": ["field"],
            "filter": "optional simple comparison",
            "groupBy": ["field"],
            "aggregations": ["count", "sum", "avg", "min", "max"],
            "limit": "bounded result rows"
        },
        "visualizationSpec": {
            "mark": "bar, line, scatter, surface, parallel-coordinates, radial-density, hyper-slice-matrix, volume-cloud",
            "layout": "2d-cartesian, 3d-scene, 4d-encoded-scene, 5d-faceted-hypercube, xd-projection-atlas",
            "encodings": ["channel to field bindings"],
            "fitness": "informationDensity + legibility + novelty + taskFit + optional aiEvaluator"
        },
        "presentationExport": {
            "formats": ["all", "powerpoint-openxml", "google-slides", "reveal-markdown", "final-layer-json"]
        },
        "semanticModel": {
            "modelId": "string",
            "datasetId": "existing dataset id",
            "lookml": "optional LookML-like view text",
            "dimensions": ["name + field + type metadata"],
            "measures": ["name + aggregation + optional field"]
        },
        "semanticCompile": {
            "dimensions": ["governed dimension names"],
            "measures": ["governed measure names"],
            "limit": "bounded SQL target rows"
        },
        "paritySurfaces": {
            "semanticLayer": "LookML-like governed registry with dataset validation and SQL compile targets",
            "associativeEngine": "Qlik-style categorical co-occurrence graph plus multi-dataset selection state over ingested datasets",
            "workbooks": "Sigma-style live-grid and executive-card blueprints",
            "connectorsAndEtl": "Domo/Power Query-style connector and transformation planners",
            "selfService": "Superset/Metabase SQL lab and visual query-builder contracts",
            "observabilityPanels": "Grafana-style time-series panel catalog and alert rule evaluator",
            "programmaticRenderers": "D3, Plotly/Dash, Evidence, and Office export contracts"
        }
    }))
}

async fn example(State(state): State<AppState>) -> Json<Value> {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    Json(json!({
        "dataset": {
            "datasetId": "sales-lab",
            "displayName": "Sales Lab",
            "replace": true,
            "records": sample_records()
        },
        "query": {
            "dialect": "sql",
            "datasetId": "sales-lab",
            "query": "SELECT region, SUM(revenue) AS totalRevenue, AVG(margin) AS avgMargin FROM sales-lab GROUP BY region LIMIT 20"
        },
        "visualization": {
            "datasetId": "sales-lab",
            "targetDimensions": 5,
            "intent": "compare revenue, margin, churn, and region performance"
        },
        "evolution": {
            "datasetId": "sales-lab",
            "objective": "maximize executive readability without hiding high-dimensional structure",
            "populationSize": 24,
            "generations": 8,
            "aiEvaluations": [
                {
                    "candidateId": "candidate-0",
                    "score": 0.82,
                    "rationale": "Readable and compares segments clearly."
                }
            ]
        }
    }))
}

async fn healthz() -> Json<Value> {
    Json(json!({ "ok": true, "service": SERVICE_NAME }))
}

async fn readyz(State(state): State<AppState>) -> Json<Value> {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    let dataset_count = state
        .datasets
        .read()
        .map(|datasets| datasets.len())
        .unwrap_or(0);
    Json(json!({
        "ok": true,
        "service": SERVICE_NAME,
        "datasetCount": dataset_count
    }))
}

async fn metrics(State(state): State<AppState>) -> Response {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    let metrics = &state.metrics;
    let body = format!(
        "\
# HELP dd_data_viz_http_requests_total HTTP requests handled.
# TYPE dd_data_viz_http_requests_total counter
dd_data_viz_http_requests_total {}
# HELP dd_data_viz_datasets_ingested_total Dataset ingest requests accepted.
# TYPE dd_data_viz_datasets_ingested_total counter
dd_data_viz_datasets_ingested_total {}
# HELP dd_data_viz_queries_total Query requests executed.
# TYPE dd_data_viz_queries_total counter
dd_data_viz_queries_total {}
# HELP dd_data_viz_visualizations_total Visualization suggestion requests handled.
# TYPE dd_data_viz_visualizations_total counter
dd_data_viz_visualizations_total {}
# HELP dd_data_viz_evolution_runs_total Evolutionary visualization runs handled.
# TYPE dd_data_viz_evolution_runs_total counter
dd_data_viz_evolution_runs_total {}
# HELP dd_data_viz_presentation_exports_total Presentation export requests handled.
# TYPE dd_data_viz_presentation_exports_total counter
dd_data_viz_presentation_exports_total {}
# HELP dd_data_viz_platform_requests_total Platform parity requests handled.
# TYPE dd_data_viz_platform_requests_total counter
dd_data_viz_platform_requests_total {}
# HELP dd_data_viz_hardening_requests_total Hardening policy requests handled.
# TYPE dd_data_viz_hardening_requests_total counter
dd_data_viz_hardening_requests_total {}
# HELP dd_data_viz_association_requests_total Associative graph requests handled.
# TYPE dd_data_viz_association_requests_total counter
dd_data_viz_association_requests_total {}
# HELP dd_data_viz_dashboard_requests_total Dashboard requests handled.
# TYPE dd_data_viz_dashboard_requests_total counter
dd_data_viz_dashboard_requests_total {}
# HELP dd_data_viz_dashboards_saved_total Dashboards saved.
# TYPE dd_data_viz_dashboards_saved_total counter
dd_data_viz_dashboards_saved_total {}
# HELP dd_data_viz_alert_requests_total Alert rule requests handled.
# TYPE dd_data_viz_alert_requests_total counter
dd_data_viz_alert_requests_total {}
# HELP dd_data_viz_alert_rules_saved_total Alert rules saved.
# TYPE dd_data_viz_alert_rules_saved_total counter
dd_data_viz_alert_rules_saved_total {}
# HELP dd_data_viz_alert_evaluations_total Alert rule evaluations executed.
# TYPE dd_data_viz_alert_evaluations_total counter
dd_data_viz_alert_evaluations_total {}
# HELP dd_data_viz_semantic_requests_total Semantic registry requests handled.
# TYPE dd_data_viz_semantic_requests_total counter
dd_data_viz_semantic_requests_total {}
# HELP dd_data_viz_semantic_models_saved_total Semantic models saved.
# TYPE dd_data_viz_semantic_models_saved_total counter
dd_data_viz_semantic_models_saved_total {}
# HELP dd_data_viz_rbac_denials_total Role-based authorization denials.
# TYPE dd_data_viz_rbac_denials_total counter
dd_data_viz_rbac_denials_total {}
# HELP dd_data_viz_auth_failures_total Failed operator auth checks.
# TYPE dd_data_viz_auth_failures_total counter
dd_data_viz_auth_failures_total {}
# HELP dd_data_viz_errors_total Request errors.
# TYPE dd_data_viz_errors_total counter
dd_data_viz_errors_total {}
",
        metrics.http_requests_total.load(Ordering::Relaxed),
        metrics.datasets_ingested_total.load(Ordering::Relaxed),
        metrics.queries_total.load(Ordering::Relaxed),
        metrics.visualizations_total.load(Ordering::Relaxed),
        metrics.evolution_runs_total.load(Ordering::Relaxed),
        metrics.presentation_exports_total.load(Ordering::Relaxed),
        metrics.platform_requests_total.load(Ordering::Relaxed),
        metrics.hardening_requests_total.load(Ordering::Relaxed),
        metrics.association_requests_total.load(Ordering::Relaxed),
        metrics.dashboard_requests_total.load(Ordering::Relaxed),
        metrics.dashboards_saved_total.load(Ordering::Relaxed),
        metrics.alert_requests_total.load(Ordering::Relaxed),
        metrics.alert_rules_saved_total.load(Ordering::Relaxed),
        metrics.alert_evaluations_total.load(Ordering::Relaxed),
        metrics.semantic_requests_total.load(Ordering::Relaxed),
        metrics.semantic_models_saved_total.load(Ordering::Relaxed),
        metrics.rbac_denials_total.load(Ordering::Relaxed),
        metrics.auth_failures_total.load(Ordering::Relaxed),
        metrics.errors_total.load(Ordering::Relaxed),
    );
    (
        StatusCode::OK,
        [("content-type", "text/plain; version=0.0.4")],
        body,
    )
        .into_response()
}

async fn ingest_dataset(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<IngestDatasetRequest>,
) -> Result<Json<IngestDatasetResponse>, ApiError> {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    authorize(&state, &headers, rbac::Permission::DatasetWrite)?;

    let dataset = Dataset::from_request(request)?;
    let metadata = dataset.metadata();
    let mut warnings = Vec::new();
    let mut datasets = state
        .datasets
        .write()
        .map_err(|_| ApiError::bad_request("dataset store lock poisoned"))?;

    if datasets.len() >= MAX_DATASETS && !datasets.contains_key(&dataset.dataset_id) {
        return Err(ApiError::bad_request(format!(
            "dataset limit exceeded; max {MAX_DATASETS}"
        )));
    }

    if datasets.contains_key(&dataset.dataset_id) {
        warnings.push(format!(
            "dataset {} replaced with a new in-memory snapshot",
            dataset.dataset_id
        ));
    }
    datasets.insert(dataset.dataset_id.clone(), dataset);
    state
        .metrics
        .datasets_ingested_total
        .fetch_add(1, Ordering::Relaxed);

    Ok(Json(IngestDatasetResponse {
        ok: true,
        dataset: metadata,
        warnings,
    }))
}

async fn list_datasets(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    authorize(&state, &headers, rbac::Permission::DatasetRead)?;
    let datasets = state
        .datasets
        .read()
        .map_err(|_| ApiError::bad_request("dataset store lock poisoned"))?;
    let items: Vec<DatasetMetadata> = datasets.values().map(Dataset::metadata).collect();
    Ok(Json(json!({
        "ok": true,
        "datasets": items
    })))
}

async fn platform_capabilities(State(state): State<AppState>) -> Json<Value> {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    state
        .metrics
        .platform_requests_total
        .fetch_add(1, Ordering::Relaxed);
    Json(platform::platform_capabilities_payload())
}

async fn connector_catalog(State(state): State<AppState>) -> Json<Value> {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    state
        .metrics
        .platform_requests_total
        .fetch_add(1, Ordering::Relaxed);
    Json(json!({
        "ok": true,
        "connectors": platform::connector_catalog(),
        "etl": platform::etl_primitives()
    }))
}

async fn semantic_models(State(state): State<AppState>) -> Json<Value> {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    state
        .metrics
        .platform_requests_total
        .fetch_add(1, Ordering::Relaxed);
    Json(json!({
        "ok": true,
        "semanticModels": platform::semantic_models()
    }))
}

async fn save_semantic_model(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<semantic::SaveSemanticModelRequest>,
) -> Result<Json<semantic::SaveSemanticModelResponse>, ApiError> {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    state
        .metrics
        .semantic_requests_total
        .fetch_add(1, Ordering::Relaxed);
    authorize(&state, &headers, rbac::Permission::SemanticWrite)?;

    let dataset = get_dataset_snapshot(&state, &request.dataset_id)?;
    let available_fields = dataset.columns.keys().cloned().collect::<BTreeSet<_>>();
    let mut model = request
        .into_model(now_ms(), &available_fields)
        .map_err(ApiError::bad_request)?;
    let mut warnings = Vec::new();
    let mut models = state
        .semantic_models
        .write()
        .map_err(|_| ApiError::bad_request("semantic model store lock poisoned"))?;
    if let Some(existing) = models.get(&model.model_id) {
        model.created_at_ms = existing.created_at_ms;
        warnings.push(format!(
            "semantic model {} replaced with a new in-memory definition",
            model.model_id
        ));
    } else if models.len() >= semantic::max_semantic_models() {
        return Err(ApiError::bad_request(format!(
            "semantic model count exceeds max {}",
            semantic::max_semantic_models()
        )));
    }
    model.updated_at_ms = now_ms();
    models.insert(model.model_id.clone(), model.clone());
    state
        .metrics
        .semantic_models_saved_total
        .fetch_add(1, Ordering::Relaxed);

    Ok(Json(semantic::save_response(model, warnings)))
}

async fn list_semantic_registry(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    state
        .metrics
        .semantic_requests_total
        .fetch_add(1, Ordering::Relaxed);
    authorize(&state, &headers, rbac::Permission::SemanticRead)?;
    let models = state
        .semantic_models
        .read()
        .map_err(|_| ApiError::bad_request("semantic model store lock poisoned"))?;
    let summaries = models
        .values()
        .map(semantic::SavedSemanticModel::summary)
        .collect::<Vec<_>>();
    Ok(Json(semantic::registry_payload(summaries)))
}

async fn get_semantic_model(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(model_id): Path<String>,
) -> Result<Json<semantic::SavedSemanticModel>, ApiError> {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    state
        .metrics
        .semantic_requests_total
        .fetch_add(1, Ordering::Relaxed);
    authorize(&state, &headers, rbac::Permission::SemanticRead)?;
    let models = state
        .semantic_models
        .read()
        .map_err(|_| ApiError::bad_request("semantic model store lock poisoned"))?;
    models
        .get(&model_id)
        .cloned()
        .map(Json)
        .ok_or_else(|| ApiError::not_found(format!("semantic model `{model_id}` not found")))
}

async fn compile_semantic_model(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(model_id): Path<String>,
    Json(request): Json<semantic::CompileSemanticQueryRequest>,
) -> Result<Json<Value>, ApiError> {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    state
        .metrics
        .semantic_requests_total
        .fetch_add(1, Ordering::Relaxed);
    authorize(&state, &headers, rbac::Permission::SemanticCompile)?;
    let model = {
        let models = state
            .semantic_models
            .read()
            .map_err(|_| ApiError::bad_request("semantic model store lock poisoned"))?;
        models
            .get(&model_id)
            .cloned()
            .ok_or_else(|| ApiError::not_found(format!("semantic model `{model_id}` not found")))?
    };
    let compiled = model
        .compile_query(request)
        .map_err(ApiError::bad_request)?;
    let logical_plan = logical_plan_from_query(&compiled.query)?;
    Ok(Json(json!({
        "ok": true,
        "schemaVersion": "data-viz.semantic-compile.v1",
        "compiled": compiled,
        "logicalPlan": logical_plan
    })))
}

async fn workbook_blueprints(State(state): State<AppState>) -> Json<Value> {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    state
        .metrics
        .platform_requests_total
        .fetch_add(1, Ordering::Relaxed);
    Json(json!({
        "ok": true,
        "workbooks": platform::workbook_blueprints(),
        "selfService": platform::self_service_surfaces()
    }))
}

async fn dashboard_panels(State(state): State<AppState>) -> Json<Value> {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    state
        .metrics
        .platform_requests_total
        .fetch_add(1, Ordering::Relaxed);
    Json(json!({
        "ok": true,
        "dashboardPanels": platform::dashboard_panel_catalog()
    }))
}

async fn renderer_contracts(State(state): State<AppState>) -> Json<Value> {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    state
        .metrics
        .platform_requests_total
        .fetch_add(1, Ordering::Relaxed);
    Json(json!({
        "ok": true,
        "rendererContracts": platform::renderer_contracts(),
        "presentationTargets": platform::presentation_targets()
    }))
}

async fn evidence_report_blueprint(State(state): State<AppState>) -> Json<Value> {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    state
        .metrics
        .platform_requests_total
        .fetch_add(1, Ordering::Relaxed);
    Json(json!({
        "ok": true,
        "schemaVersion": "data-viz.evidence-report.v1",
        "analog": "Evidence.dev",
        "blueprint": {
            "frontmatter": {
                "title": "Data Viz Report",
                "source": "dd-data-viz-rs"
            },
            "sections": [
                {
                    "type": "sql",
                    "name": "regional_revenue",
                    "query": "SELECT region, SUM(revenue) AS totalRevenue FROM sales-lab GROUP BY region"
                },
                {
                    "type": "chart",
                    "renderer": "final-layer-json",
                    "visualizationSpecRef": "candidate-0"
                },
                {
                    "type": "narrative",
                    "body": "Explain the observed trend, confidence, and decision implication."
                }
            ]
        }
    }))
}

async fn security_policy(State(state): State<AppState>) -> Json<Value> {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    state
        .metrics
        .hardening_requests_total
        .fetch_add(1, Ordering::Relaxed);
    Json(hardening::hardening_payload(
        MAX_DATASETS,
        MAX_RECORDS,
        MAX_COLUMNS,
        MAX_QUERY_ROWS,
        MAX_HTTP_BODY_BYTES,
        state.config.server_auth_secret.is_some(),
        state.config.allow_unauthenticated,
    ))
}

async fn rbac_policy(State(state): State<AppState>) -> Json<Value> {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    state
        .metrics
        .hardening_requests_total
        .fetch_add(1, Ordering::Relaxed);
    Json(rbac::policy_payload())
}

async fn association_graph(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(dataset_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    authorize(&state, &headers, rbac::Permission::AssociationRead)?;
    let dataset = get_dataset_snapshot(&state, &dataset_id)?;
    state
        .metrics
        .association_requests_total
        .fetch_add(1, Ordering::Relaxed);
    Ok(Json(dataset_association_graph(&dataset)))
}

async fn association_selection(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<associative::AssociativeSelectionRequest>,
) -> Result<Json<Value>, ApiError> {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    authorize(&state, &headers, rbac::Permission::AssociationRead)?;
    state
        .metrics
        .association_requests_total
        .fetch_add(1, Ordering::Relaxed);

    let datasets = state
        .datasets
        .read()
        .map_err(|_| ApiError::bad_request("dataset store lock poisoned"))?;
    associative::selection_payload(&datasets, request)
        .map(Json)
        .map_err(ApiError::bad_request)
}

async fn save_dashboard(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<dashboard::SaveDashboardRequest>,
) -> Result<Json<dashboard::SaveDashboardResponse>, ApiError> {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    state
        .metrics
        .dashboard_requests_total
        .fetch_add(1, Ordering::Relaxed);
    authorize(&state, &headers, rbac::Permission::DashboardWrite)?;

    let mut dashboard = request
        .into_saved(now_ms())
        .map_err(ApiError::bad_request)?;
    let mut warnings = Vec::new();
    let mut dashboards = state
        .dashboards
        .write()
        .map_err(|_| ApiError::bad_request("dashboard store lock poisoned"))?;
    if let Some(existing) = dashboards.get(&dashboard.dashboard_id) {
        dashboard.created_at_ms = existing.created_at_ms;
        warnings.push(format!(
            "dashboard {} replaced with a new in-memory snapshot",
            dashboard.dashboard_id
        ));
    }
    dashboard.updated_at_ms = now_ms();
    dashboards.insert(dashboard.dashboard_id.clone(), dashboard.clone());
    state
        .metrics
        .dashboards_saved_total
        .fetch_add(1, Ordering::Relaxed);

    Ok(Json(dashboard::SaveDashboardResponse {
        ok: true,
        dashboard,
        warnings,
    }))
}

async fn list_dashboards(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    state
        .metrics
        .dashboard_requests_total
        .fetch_add(1, Ordering::Relaxed);
    authorize(&state, &headers, rbac::Permission::DashboardRead)?;
    let dashboards = state
        .dashboards
        .read()
        .map_err(|_| ApiError::bad_request("dashboard store lock poisoned"))?;
    let summaries = dashboards
        .values()
        .map(dashboard::SavedDashboard::summary)
        .collect::<Vec<_>>();
    Ok(Json(dashboard::catalog_payload(summaries)))
}

async fn get_dashboard(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(dashboard_id): Path<String>,
) -> Result<Json<dashboard::SavedDashboard>, ApiError> {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    state
        .metrics
        .dashboard_requests_total
        .fetch_add(1, Ordering::Relaxed);
    authorize(&state, &headers, rbac::Permission::DashboardRead)?;
    let dashboards = state
        .dashboards
        .read()
        .map_err(|_| ApiError::bad_request("dashboard store lock poisoned"))?;
    dashboards
        .get(&dashboard_id)
        .cloned()
        .map(Json)
        .ok_or_else(|| ApiError::not_found(format!("dashboard `{dashboard_id}` not found")))
}

async fn save_alert_rule(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<alerts::SaveAlertRuleRequest>,
) -> Result<Json<alerts::SaveAlertRuleResponse>, ApiError> {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    state
        .metrics
        .alert_requests_total
        .fetch_add(1, Ordering::Relaxed);
    authorize(&state, &headers, rbac::Permission::AlertWrite)?;

    let mut rule = request.into_rule(now_ms()).map_err(ApiError::bad_request)?;
    let plan = logical_plan_from_query(&rule.query)?;
    let _dataset = get_dataset_snapshot(&state, &plan.source)?;
    let mut warnings = Vec::new();
    let mut alert_rules = state
        .alert_rules
        .write()
        .map_err(|_| ApiError::bad_request("alert rule store lock poisoned"))?;
    if let Some(existing) = alert_rules.get(&rule.rule_id) {
        rule.created_at_ms = existing.created_at_ms;
        warnings.push(format!(
            "alert rule {} replaced with a new in-memory definition",
            rule.rule_id
        ));
    } else if alert_rules.len() >= alerts::max_alert_rules() {
        return Err(ApiError::bad_request(format!(
            "alert rule count exceeds max {}",
            alerts::max_alert_rules()
        )));
    }
    rule.updated_at_ms = now_ms();
    alert_rules.insert(rule.rule_id.clone(), rule.clone());
    state
        .metrics
        .alert_rules_saved_total
        .fetch_add(1, Ordering::Relaxed);

    Ok(Json(alerts::save_response(rule, warnings)))
}

async fn list_alert_rules(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    state
        .metrics
        .alert_requests_total
        .fetch_add(1, Ordering::Relaxed);
    authorize(&state, &headers, rbac::Permission::AlertRead)?;
    let alert_rules = state
        .alert_rules
        .read()
        .map_err(|_| ApiError::bad_request("alert rule store lock poisoned"))?;
    let summaries = alert_rules
        .values()
        .map(alerts::AlertRule::summary)
        .collect::<Vec<_>>();
    Ok(Json(alerts::catalog_payload(summaries)))
}

async fn get_alert_rule(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(rule_id): Path<String>,
) -> Result<Json<alerts::AlertRule>, ApiError> {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    state
        .metrics
        .alert_requests_total
        .fetch_add(1, Ordering::Relaxed);
    authorize(&state, &headers, rbac::Permission::AlertRead)?;
    let alert_rules = state
        .alert_rules
        .read()
        .map_err(|_| ApiError::bad_request("alert rule store lock poisoned"))?;
    alert_rules
        .get(&rule_id)
        .cloned()
        .map(Json)
        .ok_or_else(|| ApiError::not_found(format!("alert rule `{rule_id}` not found")))
}

async fn evaluate_alert_rule(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(rule_id): Path<String>,
) -> Result<Json<alerts::AlertEvaluationResponse>, ApiError> {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    state
        .metrics
        .alert_requests_total
        .fetch_add(1, Ordering::Relaxed);
    authorize(&state, &headers, rbac::Permission::AlertEvaluate)?;
    let rule = {
        let alert_rules = state
            .alert_rules
            .read()
            .map_err(|_| ApiError::bad_request("alert rule store lock poisoned"))?;
        alert_rules
            .get(&rule_id)
            .cloned()
            .ok_or_else(|| ApiError::not_found(format!("alert rule `{rule_id}` not found")))?
    };
    let plan = logical_plan_from_query(&rule.query)?;
    let dataset = get_dataset_snapshot(&state, &plan.source)?;
    let response = execute_plan(&dataset, plan)?;
    state
        .metrics
        .alert_evaluations_total
        .fetch_add(1, Ordering::Relaxed);
    Ok(Json(alerts::evaluate_rule(&rule, &response.rows)))
}

async fn get_dataset(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(dataset_id): Path<String>,
) -> Result<Json<DatasetMetadata>, ApiError> {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    authorize(&state, &headers, rbac::Permission::DatasetRead)?;
    let dataset = get_dataset_snapshot(&state, &dataset_id)?;
    Ok(Json(dataset.metadata()))
}

async fn query(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<QueryRequest>,
) -> Result<Json<QueryResponse>, ApiError> {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    authorize(&state, &headers, rbac::Permission::QueryExecute)?;

    let plan = logical_plan_from_query(&request)?;
    let dataset = get_dataset_snapshot(&state, &plan.source)?;
    let mut response = execute_plan(&dataset, plan)?;
    response.ok = true;
    state.metrics.queries_total.fetch_add(1, Ordering::Relaxed);
    Ok(Json(response))
}

async fn suggest_visualizations(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<VisualizationRequest>,
) -> Result<Json<VisualizationResponse>, ApiError> {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    authorize(&state, &headers, rbac::Permission::VisualizationSuggest)?;

    let dataset = get_dataset_snapshot(&state, &request.dataset_id)?;
    let plan = match &request.query {
        Some(query_request) => Some(logical_plan_from_query(query_request)?),
        None => None,
    };
    let dimensions = requested_dimensions(&dataset, request.dimensions.as_deref());
    let target_dimensions = request
        .target_dimensions
        .unwrap_or_else(|| dimensions.len().clamp(2, 8))
        .max(2);
    let max_candidates = request.max_candidates.unwrap_or(8).clamp(1, 24);
    let candidates = visualization_candidates(
        &dataset,
        &dimensions,
        target_dimensions,
        request.intent.as_deref().unwrap_or("explore"),
        max_candidates,
        None,
    );

    state
        .metrics
        .visualizations_total
        .fetch_add(1, Ordering::Relaxed);
    Ok(Json(VisualizationResponse {
        ok: true,
        dataset: dataset.metadata(),
        logical_plan: plan,
        candidates,
        warnings: Vec::new(),
    }))
}

async fn run_evolution(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<EvolutionRequest>,
) -> Result<Json<EvolutionResponse>, ApiError> {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    authorize(&state, &headers, rbac::Permission::EvolutionRun)?;

    let dataset = get_dataset_snapshot(&state, &request.dataset_id)?;
    let dimensions = requested_dimensions(&dataset, request.dimensions.as_deref());
    let objective = request
        .objective
        .clone()
        .unwrap_or_else(|| "discover a useful, legible high-dimensional view".to_string());
    let population_size = request
        .population_size
        .unwrap_or(32)
        .clamp(4, MAX_EVOLUTION_POPULATION);
    let generations = request
        .generations
        .unwrap_or(8)
        .clamp(1, MAX_EVOLUTION_GENERATIONS);
    let ai_scores = ai_score_map(request.ai_evaluations.as_deref());
    let seed = request.seed.unwrap_or_else(|| now_ms() as u64);
    let mut rng = Lcg::new(seed);
    let mut population = visualization_candidates(
        &dataset,
        &dimensions,
        dimensions.len().clamp(2, 12),
        &objective,
        population_size,
        Some(&ai_scores),
    );
    let mut generation_summaries = Vec::new();

    for generation in 0..generations {
        population.sort_by(|left, right| right.fitness.total.total_cmp(&left.fitness.total));
        let best_score = population[0].fitness.total;
        let average_score = population
            .iter()
            .map(|candidate| candidate.fitness.total)
            .sum::<f64>()
            / population.len() as f64;
        generation_summaries.push(EvolutionGeneration {
            generation,
            best_candidate_id: population[0].id.clone(),
            best_score,
            average_score,
            mutation_summary: vec![
                "rotated channel assignments".to_string(),
                "varied mark family".to_string(),
                "rebalanced dimensional projection".to_string(),
            ],
        });

        let elites = population.iter().take(4).cloned().collect::<Vec<_>>();
        let mut next_population = elites.clone();
        while next_population.len() < population_size {
            let parent = &elites[rng.choose(elites.len())];
            let mut child = mutate_visualization(parent, &dataset, &dimensions, &mut rng);
            score_visualization(&mut child, &dataset, &objective, Some(&ai_scores));
            next_population.push(child);
        }
        population = next_population;
    }

    population.sort_by(|left, right| right.fitness.total.total_cmp(&left.fitness.total));
    let best = population[0].clone();
    let run_id = format!("viz-run-{}", now_ms());
    let record = EvolutionRunRecord {
        run_id: run_id.clone(),
        dataset_id: dataset.dataset_id.clone(),
        objective: objective.clone(),
        created_at_ms: now_ms(),
        best_candidate_id: best.id.clone(),
        best_score: best.fitness.total,
    };
    state
        .evolution_runs
        .write()
        .map_err(|_| ApiError::bad_request("evolution store lock poisoned"))?
        .insert(run_id.clone(), record);
    state
        .metrics
        .evolution_runs_total
        .fetch_add(1, Ordering::Relaxed);

    Ok(Json(EvolutionResponse {
        ok: true,
        run_id,
        objective: objective.clone(),
        best,
        population,
        generations: generation_summaries,
        evaluator_prompt: evaluator_prompt(&dataset, &objective),
        warnings: Vec::new(),
    }))
}

async fn list_evolution_runs(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    authorize(&state, &headers, rbac::Permission::EvolutionRead)?;
    let runs = state
        .evolution_runs
        .read()
        .map_err(|_| ApiError::bad_request("evolution store lock poisoned"))?;
    let items: Vec<EvolutionRunRecord> = runs.values().cloned().collect();
    Ok(Json(json!({ "ok": true, "runs": items })))
}

async fn export_presentation(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<PresentationExportRequest>,
) -> Result<Json<PresentationExportResponse>, ApiError> {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    authorize(&state, &headers, rbac::Permission::PresentationExport)?;

    if request.specs.is_empty() {
        return Err(ApiError::bad_request(
            "presentation export requires at least one visualization spec",
        ));
    }
    let format = request.format.unwrap_or(PresentationFormat::All);
    let slides = presentation_slides(&request);
    let powerpoint_open_xml = match format {
        PresentationFormat::All | PresentationFormat::PowerPointOpenXml => {
            Some(powerpoint_open_xml_package(&request, &slides))
        }
        _ => None,
    };
    let google_slides_batch_update = match format {
        PresentationFormat::All | PresentationFormat::GoogleSlides => {
            Some(google_slides_batch_update(&request, &slides))
        }
        _ => None,
    };
    let reveal_markdown = match format {
        PresentationFormat::All | PresentationFormat::RevealMarkdown => {
            Some(reveal_markdown(&request, &slides))
        }
        _ => None,
    };
    let final_layers = final_layer_json(&request, &slides);
    state
        .metrics
        .presentation_exports_total
        .fetch_add(1, Ordering::Relaxed);

    Ok(Json(PresentationExportResponse {
        ok: true,
        format,
        slides,
        powerpoint_open_xml,
        google_slides_batch_update,
        reveal_markdown,
        final_layers,
    }))
}

async fn docs_html(State(state): State<AppState>) -> Html<String> {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    let routes = route_docs()
        .iter()
        .map(|route| {
            format!(
                "<tr><td><code>{}</code></td><td><code>{}</code></td><td>{}</td><td>{}</td></tr>",
                route.method,
                route.path,
                html_escape(route.auth),
                html_escape(route.description)
            )
        })
        .collect::<Vec<_>>()
        .join("\n");

    Html(format!(
        r#"<!doctype html>
<html>
<head>
  <meta charset="utf-8">
  <title>{service} API</title>
  <style>
    body {{ font-family: system-ui, sans-serif; margin: 2rem; max-width: 1100px; }}
    table {{ border-collapse: collapse; width: 100%; }}
    th, td {{ border-bottom: 1px solid #ddd; padding: 0.45rem; text-align: left; vertical-align: top; }}
    code {{ background: #f4f4f5; padding: 0.1rem 0.25rem; border-radius: 4px; }}
  </style>
</head>
<body>
  <h1>{service} API</h1>
  <p>Machine-readable docs are available at <code>/api/docs.json</code>.</p>
  <table>
    <thead><tr><th>Method</th><th>Path</th><th>Auth</th><th>Description</th></tr></thead>
    <tbody>{routes}</tbody>
  </table>
</body>
</html>"#,
        service = SERVICE_NAME,
        routes = routes
    ))
}

async fn docs_json(State(state): State<AppState>) -> Json<Value> {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    Json(json!({
        "ok": true,
        "service": SERVICE_NAME,
        "schemaVersion": SCHEMA_VERSION,
        "routes": route_docs(),
        "dialects": dialect_catalog(),
        "platformParity": platform::parity_matrix(),
        "hardening": hardening::control_catalog()
    }))
}

impl Dataset {
    fn from_request(request: IngestDatasetRequest) -> Result<Self, ApiError> {
        let dataset_id = clean_identifier(&request.dataset_id).ok_or_else(|| {
            ApiError::bad_request(
                "datasetId must contain letters, numbers, dash, underscore, dot, or colon",
            )
        })?;
        if request.records.is_empty() {
            return Err(ApiError::bad_request("records cannot be empty"));
        }
        if request.records.len() > MAX_RECORDS {
            return Err(ApiError::bad_request(format!(
                "records exceeds max {MAX_RECORDS}"
            )));
        }

        let mut field_names = BTreeSet::new();
        for record in &request.records {
            for key in record.keys() {
                let field = clean_identifier(key).ok_or_else(|| {
                    ApiError::bad_request(format!("invalid field name `{key}` in records"))
                })?;
                field_names.insert(field);
            }
        }
        if field_names.is_empty() {
            return Err(ApiError::bad_request(
                "records must contain at least one field",
            ));
        }
        if field_names.len() > MAX_COLUMNS {
            return Err(ApiError::bad_request(format!(
                "column count exceeds max {MAX_COLUMNS}"
            )));
        }

        let mut columns = BTreeMap::new();
        for field in field_names {
            let mut values = Vec::with_capacity(request.records.len());
            for record in &request.records {
                values.push(record.get(&field).cloned().unwrap_or(Value::Null));
            }
            columns.insert(field, Column::from_values(values));
        }

        let now = now_ms();
        Ok(Self {
            dataset_id: dataset_id.clone(),
            display_name: request.display_name.unwrap_or(dataset_id),
            row_count: request.records.len(),
            columns,
            created_at_ms: now,
            updated_at_ms: now,
        })
    }

    fn metadata(&self) -> DatasetMetadata {
        DatasetMetadata {
            dataset_id: self.dataset_id.clone(),
            display_name: self.display_name.clone(),
            row_count: self.row_count,
            column_count: self.columns.len(),
            created_at_ms: self.created_at_ms,
            updated_at_ms: self.updated_at_ms,
            columns: self
                .columns
                .iter()
                .map(|(name, column)| column.profile(name))
                .collect(),
        }
    }

    fn value(&self, field: &str, row: usize) -> Value {
        self.columns
            .get(field)
            .map(|column| column.value(row))
            .unwrap_or(Value::Null)
    }

    fn numeric_value(&self, field: &str, row: usize) -> Option<f64> {
        self.columns
            .get(field)
            .and_then(|column| column.numeric(row))
    }

    fn field_type(&self, field: &str) -> String {
        self.columns
            .get(field)
            .map(Column::data_type)
            .unwrap_or_else(|| "unknown".to_string())
    }
}

impl Column {
    fn from_values(values: Vec<Value>) -> Self {
        let all_numbers = values
            .iter()
            .all(|value| value.is_null() || value.as_f64().is_some());
        if all_numbers {
            return Self::Number(values.iter().map(Value::as_f64).collect());
        }

        let all_bools = values
            .iter()
            .all(|value| value.is_null() || value.as_bool().is_some());
        if all_bools {
            return Self::Boolean(values.iter().map(Value::as_bool).collect());
        }

        let mut dictionary = Vec::<String>::new();
        let mut dictionary_index = BTreeMap::<String, u32>::new();
        let mut codes = Vec::with_capacity(values.len());

        for value in values {
            if value.is_null() {
                codes.push(None);
                continue;
            }
            let label = scalar_to_label(&value);
            let code = match dictionary_index.get(&label) {
                Some(code) => *code,
                None => {
                    let code = dictionary.len() as u32;
                    dictionary.push(label.clone());
                    dictionary_index.insert(label, code);
                    code
                }
            };
            codes.push(Some(code));
        }

        Self::Dictionary { dictionary, codes }
    }

    fn data_type(&self) -> String {
        match self {
            Self::Number(_) => "number".to_string(),
            Self::Dictionary { .. } => "category".to_string(),
            Self::Boolean(_) => "boolean".to_string(),
        }
    }

    fn profile(&self, name: &str) -> ColumnProfile {
        match self {
            Self::Number(values) => {
                let numbers: Vec<f64> = values.iter().flatten().copied().collect();
                let missing_count = values.len() - numbers.len();
                let numeric = if numbers.is_empty() {
                    None
                } else {
                    let min = numbers.iter().copied().fold(f64::INFINITY, f64::min);
                    let max = numbers.iter().copied().fold(f64::NEG_INFINITY, f64::max);
                    let mean = numbers.iter().sum::<f64>() / numbers.len() as f64;
                    let variance = numbers
                        .iter()
                        .map(|value| {
                            let diff = value - mean;
                            diff * diff
                        })
                        .sum::<f64>()
                        / numbers.len().max(1) as f64;
                    Some(NumericProfile {
                        min,
                        max,
                        mean,
                        variance,
                    })
                };
                ColumnProfile {
                    name: name.to_string(),
                    data_type: self.data_type(),
                    missing_count,
                    numeric,
                    categorical: None,
                }
            }
            Self::Dictionary { dictionary, codes } => {
                let mut counts = BTreeMap::<String, usize>::new();
                let mut present = 0usize;
                for code in codes.iter().flatten() {
                    present += 1;
                    if let Some(value) = dictionary.get(*code as usize) {
                        *counts.entry(value.clone()).or_default() += 1;
                    }
                }
                let mut top_values = counts
                    .into_iter()
                    .map(|(value, count)| CategoryCount { value, count })
                    .collect::<Vec<_>>();
                top_values.sort_by(|left, right| right.count.cmp(&left.count));
                top_values.truncate(8);
                ColumnProfile {
                    name: name.to_string(),
                    data_type: self.data_type(),
                    missing_count: codes.len() - present,
                    numeric: None,
                    categorical: Some(CategoricalProfile {
                        cardinality: dictionary.len(),
                        top_values,
                    }),
                }
            }
            Self::Boolean(values) => {
                let mut counts = BTreeMap::<String, usize>::new();
                let mut present = 0usize;
                for value in values.iter().flatten() {
                    present += 1;
                    *counts.entry(value.to_string()).or_default() += 1;
                }
                let top_values = counts
                    .into_iter()
                    .map(|(value, count)| CategoryCount { value, count })
                    .collect();
                ColumnProfile {
                    name: name.to_string(),
                    data_type: self.data_type(),
                    missing_count: values.len() - present,
                    numeric: None,
                    categorical: Some(CategoricalProfile {
                        cardinality: 2,
                        top_values,
                    }),
                }
            }
        }
    }

    fn value(&self, row: usize) -> Value {
        match self {
            Self::Number(values) => values
                .get(row)
                .copied()
                .flatten()
                .map(Value::from)
                .unwrap_or(Value::Null),
            Self::Dictionary { dictionary, codes } => codes
                .get(row)
                .copied()
                .flatten()
                .and_then(|code| dictionary.get(code as usize).cloned())
                .map(Value::from)
                .unwrap_or(Value::Null),
            Self::Boolean(values) => values
                .get(row)
                .copied()
                .flatten()
                .map(Value::from)
                .unwrap_or(Value::Null),
        }
    }

    fn numeric(&self, row: usize) -> Option<f64> {
        match self {
            Self::Number(values) => values.get(row).copied().flatten(),
            Self::Boolean(values) => {
                values
                    .get(row)
                    .copied()
                    .flatten()
                    .map(|value| if value { 1.0 } else { 0.0 })
            }
            Self::Dictionary { .. } => None,
        }
    }
}

fn logical_plan_from_query(request: &QueryRequest) -> Result<LogicalPlan, ApiError> {
    let limit = request.limit.unwrap_or(1_000).clamp(1, MAX_QUERY_ROWS);
    let mut plan = match request.dialect {
        QueryDialect::Sql | QueryDialect::InfluxQl => parse_sql_like(request, limit)?,
        QueryDialect::GraphQl => parse_graphql(request, limit)?,
        QueryDialect::PromQl | QueryDialect::LogQl => parse_promql_like(request, limit)?,
        QueryDialect::Flux => parse_flux(request, limit)?,
        QueryDialect::Cypher => parse_cypher(request, limit)?,
        QueryDialect::Gremlin => parse_gremlin(request, limit)?,
        QueryDialect::Mongo => parse_mongo(request, limit)?,
        QueryDialect::JmesPath => parse_jmespath(request, limit)?,
        QueryDialect::Lucene | QueryDialect::Spl => parse_search_pipeline(request, limit)?,
    };
    if let Some(dataset_id) = request
        .dataset_id
        .as_ref()
        .and_then(|value| clean_identifier(value))
    {
        plan.source = dataset_id;
    }
    if plan.source.trim().is_empty() {
        return Err(ApiError::bad_request(
            "query must name a dataset or include datasetId",
        ));
    }
    Ok(plan)
}

fn parse_sql_like(request: &QueryRequest, default_limit: usize) -> Result<LogicalPlan, ApiError> {
    if request.dialect == QueryDialect::Sql {
        return sql_frontend::parse_select(request, default_limit);
    }

    let query = request.query.trim();
    let select_idx = find_ascii_case(query, "SELECT ")
        .ok_or_else(|| ApiError::bad_request("SQL query must include SELECT"))?;
    let from_idx = find_ascii_case(query, " FROM ")
        .ok_or_else(|| ApiError::bad_request("SQL query must include FROM"))?;
    let select_part = &query[select_idx + "SELECT ".len()..from_idx];
    let after_from = &query[from_idx + " FROM ".len()..];
    let (after_from, limit) = split_optional_limit(after_from, default_limit);
    let (source_part, where_part, group_part) = split_sql_clauses(after_from);
    let source = clean_identifier(source_part.trim())
        .or_else(|| request.dataset_id.clone())
        .ok_or_else(|| ApiError::bad_request("SQL FROM must name a dataset"))?;
    let mut group_by = group_part
        .map(parse_field_list)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|field| clean_field(&field))
        .collect::<Vec<_>>();
    let mut projections = Vec::new();
    let mut aggregations = Vec::new();

    for item in select_part.split(',') {
        if let Some(agg) = parse_aggregation_item(item) {
            aggregations.push(agg);
        } else if let Some(field) = clean_field(item) {
            if !group_by.contains(&field) {
                projections.push(field.clone());
            }
            if !aggregations.is_empty() && !group_by.contains(&field) {
                group_by.push(field);
            }
        }
    }

    Ok(LogicalPlan {
        schema_version: SCHEMA_VERSION.to_string(),
        dialect: request.dialect,
        source,
        projections,
        filter: where_part.and_then(parse_filter_expr),
        group_by,
        aggregations,
        limit,
    })
}

fn parse_graphql(request: &QueryRequest, limit: usize) -> Result<LogicalPlan, ApiError> {
    let query = request.query.as_str();
    let source = request
        .dataset_id
        .clone()
        .or_else(|| extract_arg_after(query, "dataset", "name"))
        .or_else(|| extract_arg_after(query, "dataset", "id"))
        .and_then(|value| clean_identifier(&value))
        .ok_or_else(|| {
            ApiError::bad_request("GraphQL query needs datasetId or dataset(name: ...)")
        })?;
    let group_by = extract_arg_after(query, "groupBy", "field")
        .and_then(|field| clean_field(&field))
        .into_iter()
        .collect::<Vec<_>>();
    let mut aggregations = Vec::new();
    for (marker, op) in [
        ("sum", AggregationOp::Sum),
        ("avg", AggregationOp::Avg),
        ("mean", AggregationOp::Avg),
        ("min", AggregationOp::Min),
        ("max", AggregationOp::Max),
        ("count", AggregationOp::Count),
    ] {
        if let Some(field) = extract_arg_after(query, marker, "field") {
            let field = clean_field(&field);
            aggregations.push(AggregationExpr {
                alias: marker.to_string(),
                op,
                field,
            });
        } else if marker == "count" && find_ascii_case(query, "count").is_some() {
            aggregations.push(AggregationExpr {
                alias: "count".to_string(),
                op,
                field: None,
            });
        }
    }
    Ok(LogicalPlan {
        schema_version: SCHEMA_VERSION.to_string(),
        dialect: QueryDialect::GraphQl,
        source,
        projections: Vec::new(),
        filter: None,
        group_by,
        aggregations,
        limit,
    })
}

fn parse_promql_like(request: &QueryRequest, limit: usize) -> Result<LogicalPlan, ApiError> {
    let query = request.query.trim();
    let lower = query.to_ascii_lowercase();
    let op = if lower.starts_with("avg") || lower.starts_with("mean") {
        AggregationOp::Avg
    } else if lower.starts_with("min") {
        AggregationOp::Min
    } else if lower.starts_with("max") {
        AggregationOp::Max
    } else if lower.contains("count_over_time") || lower.starts_with("count") {
        AggregationOp::Count
    } else {
        AggregationOp::Sum
    };
    let group_by = extract_between(query, "by (", ")")
        .map(parse_field_list)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|field| clean_field(&field))
        .collect::<Vec<_>>();
    let metric = extract_last_parenthesized(query)
        .and_then(|value| clean_field(&value))
        .or_else(|| request.dataset_id.clone())
        .unwrap_or_else(|| "value".to_string());
    let source = request
        .dataset_id
        .clone()
        .and_then(|value| clean_identifier(&value))
        .unwrap_or_else(|| metric.clone());
    Ok(LogicalPlan {
        schema_version: SCHEMA_VERSION.to_string(),
        dialect: request.dialect,
        source,
        projections: Vec::new(),
        filter: None,
        group_by,
        aggregations: vec![AggregationExpr {
            alias: format!("{}_{metric}", op.as_str()),
            op,
            field: if op == AggregationOp::Count {
                None
            } else {
                Some(metric)
            },
        }],
        limit,
    })
}

fn parse_flux(request: &QueryRequest, limit: usize) -> Result<LogicalPlan, ApiError> {
    let query = request.query.as_str();
    let source = request
        .dataset_id
        .clone()
        .or_else(|| extract_arg(query, "bucket"))
        .and_then(|value| clean_identifier(&value))
        .ok_or_else(|| ApiError::bad_request("Flux query needs datasetId or from(bucket: ...)"))?;
    let group_by = extract_between(query, "columns: [", "]")
        .map(|items| {
            items
                .split(',')
                .filter_map(clean_field)
                .collect::<Vec<String>>()
        })
        .unwrap_or_default();
    let (op, marker) = if find_ascii_case(query, "mean(").is_some() {
        (AggregationOp::Avg, "mean")
    } else if find_ascii_case(query, "min(").is_some() {
        (AggregationOp::Min, "min")
    } else if find_ascii_case(query, "max(").is_some() {
        (AggregationOp::Max, "max")
    } else if find_ascii_case(query, "count(").is_some() {
        (AggregationOp::Count, "count")
    } else {
        (AggregationOp::Sum, "sum")
    };
    let field = extract_arg_after(query, marker, "column")
        .and_then(|value| clean_field(&value))
        .or_else(|| Some("value".to_string()));
    Ok(LogicalPlan {
        schema_version: SCHEMA_VERSION.to_string(),
        dialect: QueryDialect::Flux,
        source,
        projections: Vec::new(),
        filter: None,
        group_by,
        aggregations: vec![AggregationExpr {
            alias: marker.to_string(),
            op,
            field: if op == AggregationOp::Count {
                None
            } else {
                field
            },
        }],
        limit,
    })
}

fn parse_cypher(request: &QueryRequest, limit: usize) -> Result<LogicalPlan, ApiError> {
    let query = request.query.as_str();
    let source = request
        .dataset_id
        .clone()
        .or_else(|| extract_between(query, ":", ")").map(|value| value.to_string()))
        .and_then(|value| clean_identifier(&value))
        .ok_or_else(|| ApiError::bad_request("Cypher query needs datasetId or node label"))?;
    let return_idx = find_ascii_case(query, " RETURN ")
        .ok_or_else(|| ApiError::bad_request("Cypher query must include RETURN"))?;
    let return_part = &query[return_idx + " RETURN ".len()..];
    let mut group_by = Vec::new();
    let mut aggregations = Vec::new();
    for item in return_part.split(',') {
        if let Some(agg) = parse_aggregation_item(item) {
            aggregations.push(agg);
        } else if let Some(field) = clean_field(item) {
            group_by.push(field);
        }
    }
    Ok(LogicalPlan {
        schema_version: SCHEMA_VERSION.to_string(),
        dialect: QueryDialect::Cypher,
        source,
        projections: Vec::new(),
        filter: None,
        group_by,
        aggregations,
        limit,
    })
}

fn parse_gremlin(request: &QueryRequest, limit: usize) -> Result<LogicalPlan, ApiError> {
    let query = request.query.as_str();
    let source = request
        .dataset_id
        .clone()
        .or_else(|| extract_between(query, "hasLabel('", "')").map(|value| value.to_string()))
        .and_then(|value| clean_identifier(&value))
        .ok_or_else(|| {
            ApiError::bad_request("Gremlin query needs datasetId or hasLabel('dataset')")
        })?;
    let group_by = extract_between(query, "by('", "')")
        .and_then(clean_field)
        .into_iter()
        .collect::<Vec<_>>();
    let field = extract_between(query, "values('", "')").and_then(clean_field);
    let op = if query.contains(".mean()") {
        AggregationOp::Avg
    } else if query.contains(".min()") {
        AggregationOp::Min
    } else if query.contains(".max()") {
        AggregationOp::Max
    } else if query.contains(".count()") {
        AggregationOp::Count
    } else {
        AggregationOp::Sum
    };
    Ok(LogicalPlan {
        schema_version: SCHEMA_VERSION.to_string(),
        dialect: QueryDialect::Gremlin,
        source,
        projections: Vec::new(),
        filter: None,
        group_by,
        aggregations: vec![AggregationExpr {
            alias: op.as_str().to_string(),
            op,
            field: if op == AggregationOp::Count {
                None
            } else {
                field
            },
        }],
        limit,
    })
}

fn parse_mongo(request: &QueryRequest, limit: usize) -> Result<LogicalPlan, ApiError> {
    let source = request
        .dataset_id
        .clone()
        .and_then(|value| clean_identifier(&value))
        .ok_or_else(|| ApiError::bad_request("Mongo pipeline requires datasetId"))?;
    let pipeline: Value = serde_json::from_str(&request.query)
        .map_err(|error| ApiError::bad_request(format!("invalid Mongo pipeline JSON: {error}")))?;
    let stages = pipeline
        .as_array()
        .ok_or_else(|| ApiError::bad_request("Mongo query must be a JSON pipeline array"))?;
    let mut filter = None;
    let mut group_by = Vec::new();
    let mut aggregations = Vec::new();

    for stage in stages {
        if let Some(match_stage) = stage.get("$match").and_then(Value::as_object) {
            if let Some((field, value)) = match_stage.iter().next() {
                filter = Some(FilterExpr {
                    field: field.clone(),
                    op: "=".to_string(),
                    value: value.clone(),
                });
            }
        }
        if let Some(group_stage) = stage.get("$group").and_then(Value::as_object) {
            if let Some(id_field) = group_stage.get("_id").and_then(Value::as_str) {
                if let Some(field) = clean_field(id_field) {
                    group_by.push(field);
                }
            }
            for (alias, expression) in group_stage {
                if alias == "_id" {
                    continue;
                }
                if let Some(object) = expression.as_object() {
                    if let Some(value) = object.get("$sum") {
                        if value.as_i64() == Some(1) {
                            aggregations.push(AggregationExpr {
                                alias: alias.clone(),
                                op: AggregationOp::Count,
                                field: None,
                            });
                        } else {
                            aggregations.push(AggregationExpr {
                                alias: alias.clone(),
                                op: AggregationOp::Sum,
                                field: value.as_str().and_then(clean_field),
                            });
                        }
                    }
                    if let Some(value) = object.get("$avg") {
                        aggregations.push(AggregationExpr {
                            alias: alias.clone(),
                            op: AggregationOp::Avg,
                            field: value.as_str().and_then(clean_field),
                        });
                    }
                    if let Some(value) = object.get("$min") {
                        aggregations.push(AggregationExpr {
                            alias: alias.clone(),
                            op: AggregationOp::Min,
                            field: value.as_str().and_then(clean_field),
                        });
                    }
                    if let Some(value) = object.get("$max") {
                        aggregations.push(AggregationExpr {
                            alias: alias.clone(),
                            op: AggregationOp::Max,
                            field: value.as_str().and_then(clean_field),
                        });
                    }
                }
            }
        }
    }

    Ok(LogicalPlan {
        schema_version: SCHEMA_VERSION.to_string(),
        dialect: QueryDialect::Mongo,
        source,
        projections: Vec::new(),
        filter,
        group_by,
        aggregations,
        limit,
    })
}

fn parse_jmespath(request: &QueryRequest, limit: usize) -> Result<LogicalPlan, ApiError> {
    let source = request
        .dataset_id
        .clone()
        .and_then(|value| clean_identifier(&value))
        .ok_or_else(|| ApiError::bad_request("JMESPath query requires datasetId"))?;
    let mut group_by = Vec::new();
    if let Some(group) = extract_between(&request.query, "group_by(&", ")") {
        if let Some(field) = clean_field(group) {
            group_by.push(field);
        }
    }
    let projections = extract_between(&request.query, "{", "}")
        .map(|fields| {
            fields
                .split(',')
                .filter_map(|item| item.split(':').next_back())
                .filter_map(clean_field)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    Ok(LogicalPlan {
        schema_version: SCHEMA_VERSION.to_string(),
        dialect: QueryDialect::JmesPath,
        source,
        projections,
        filter: None,
        group_by,
        aggregations: vec![AggregationExpr {
            alias: "count".to_string(),
            op: AggregationOp::Count,
            field: None,
        }],
        limit,
    })
}

fn parse_search_pipeline(request: &QueryRequest, limit: usize) -> Result<LogicalPlan, ApiError> {
    let query = request.query.as_str();
    let source = request
        .dataset_id
        .clone()
        .or_else(|| extract_token_value(query, "index"))
        .or_else(|| extract_token_value(query, "dataset"))
        .and_then(|value| clean_identifier(&value))
        .ok_or_else(|| {
            ApiError::bad_request("search dialect query needs datasetId, index:, or dataset:")
        })?;
    let stats = query
        .split('|')
        .find(|part| find_ascii_case(part, "stats").is_some())
        .unwrap_or(query);
    let mut group_by = Vec::new();
    if let Some(by_idx) = find_ascii_case(stats, " by ") {
        group_by = parse_field_list(&stats[by_idx + " by ".len()..])
            .into_iter()
            .filter_map(|field| clean_field(&field))
            .collect();
    }
    let aggregations = parse_field_list(stats)
        .into_iter()
        .filter_map(|item| parse_aggregation_item(&item))
        .collect::<Vec<_>>();
    Ok(LogicalPlan {
        schema_version: SCHEMA_VERSION.to_string(),
        dialect: request.dialect,
        source,
        projections: Vec::new(),
        filter: None,
        group_by,
        aggregations,
        limit,
    })
}

fn execute_plan(dataset: &Dataset, plan: LogicalPlan) -> Result<QueryResponse, ApiError> {
    if plan.aggregations.is_empty() {
        let mut rows = Vec::new();
        let projection_fields = if plan.projections.is_empty() {
            dataset.columns.keys().cloned().collect::<Vec<_>>()
        } else {
            plan.projections.clone()
        };
        for row_idx in 0..dataset.row_count {
            if !filter_matches(dataset, row_idx, plan.filter.as_ref()) {
                continue;
            }
            let mut row = BTreeMap::new();
            for field in &projection_fields {
                row.insert(field.clone(), dataset.value(field, row_idx));
            }
            rows.push(row);
            if rows.len() >= plan.limit {
                break;
            }
        }
        return Ok(QueryResponse {
            ok: true,
            row_count: rows.len(),
            rows,
            logical_plan: plan,
            warnings: Vec::new(),
        });
    }

    let mut groups = BTreeMap::<String, GroupAccumulator>::new();
    for row_idx in 0..dataset.row_count {
        if !filter_matches(dataset, row_idx, plan.filter.as_ref()) {
            continue;
        }
        let key_values = plan
            .group_by
            .iter()
            .map(|field| scalar_to_label(&dataset.value(field, row_idx)))
            .collect::<Vec<_>>();
        let key = if key_values.is_empty() {
            "__all__".to_string()
        } else {
            key_values.join("\u{1f}")
        };
        let accumulator = groups.entry(key).or_insert_with(|| GroupAccumulator {
            key_values,
            aggregations: plan
                .aggregations
                .iter()
                .map(|aggregation| AggState::new(aggregation.op))
                .collect(),
        });
        for (index, aggregation) in plan.aggregations.iter().enumerate() {
            let numeric = aggregation
                .field
                .as_deref()
                .and_then(|field| dataset.numeric_value(field, row_idx));
            accumulator.aggregations[index].update(aggregation.op, numeric);
        }
    }

    let mut rows = Vec::new();
    for accumulator in groups.values().take(plan.limit) {
        let mut row = BTreeMap::new();
        for (index, field) in plan.group_by.iter().enumerate() {
            row.insert(
                field.clone(),
                Value::from(
                    accumulator
                        .key_values
                        .get(index)
                        .cloned()
                        .unwrap_or_default(),
                ),
            );
        }
        for (index, aggregation) in plan.aggregations.iter().enumerate() {
            row.insert(
                aggregation.alias.clone(),
                accumulator.aggregations[index].finish(aggregation.op),
            );
        }
        rows.push(row);
    }

    Ok(QueryResponse {
        ok: true,
        row_count: rows.len(),
        rows,
        logical_plan: plan,
        warnings: Vec::new(),
    })
}

#[derive(Debug, Clone)]
struct GroupAccumulator {
    key_values: Vec<String>,
    aggregations: Vec<AggState>,
}

#[derive(Debug, Clone)]
struct AggState {
    count: u64,
    sum: f64,
    min: f64,
    max: f64,
}

impl AggState {
    fn new(op: AggregationOp) -> Self {
        Self {
            count: 0,
            sum: 0.0,
            min: if op == AggregationOp::Min {
                f64::INFINITY
            } else {
                0.0
            },
            max: if op == AggregationOp::Max {
                f64::NEG_INFINITY
            } else {
                0.0
            },
        }
    }

    fn update(&mut self, op: AggregationOp, numeric: Option<f64>) {
        match op {
            AggregationOp::Count => {
                self.count += 1;
            }
            AggregationOp::Sum | AggregationOp::Avg => {
                if let Some(value) = numeric {
                    self.count += 1;
                    self.sum += value;
                }
            }
            AggregationOp::Min => {
                if let Some(value) = numeric {
                    self.count += 1;
                    self.min = self.min.min(value);
                }
            }
            AggregationOp::Max => {
                if let Some(value) = numeric {
                    self.count += 1;
                    self.max = self.max.max(value);
                }
            }
        }
    }

    fn finish(&self, op: AggregationOp) -> Value {
        match op {
            AggregationOp::Count => Value::from(self.count),
            AggregationOp::Sum => Value::from(round4(self.sum)),
            AggregationOp::Avg => {
                if self.count == 0 {
                    Value::Null
                } else {
                    Value::from(round4(self.sum / self.count as f64))
                }
            }
            AggregationOp::Min => {
                if self.count == 0 {
                    Value::Null
                } else {
                    Value::from(round4(self.min))
                }
            }
            AggregationOp::Max => {
                if self.count == 0 {
                    Value::Null
                } else {
                    Value::from(round4(self.max))
                }
            }
        }
    }
}

impl AggregationOp {
    fn as_str(self) -> &'static str {
        match self {
            Self::Count => "count",
            Self::Sum => "sum",
            Self::Avg => "avg",
            Self::Min => "min",
            Self::Max => "max",
        }
    }
}

fn visualization_candidates(
    dataset: &Dataset,
    dimensions: &[String],
    target_dimensions: usize,
    intent: &str,
    max_candidates: usize,
    ai_scores: Option<&BTreeMap<String, f64>>,
) -> Vec<VisualizationSpec> {
    let marks = [
        "scatter",
        "bar",
        "line",
        "surface",
        "parallel-coordinates",
        "radial-density",
        "hyper-slice-matrix",
        "volume-cloud",
    ];
    let mut candidates = Vec::new();
    for index in 0..max_candidates {
        let dimension_count = target_dimensions.min(dimensions.len().max(2));
        let mark = marks[index % marks.len()].to_string();
        let layout = layout_for_dimensions(dimension_count).to_string();
        let projection = projection_for_dimensions(dimension_count, index).to_string();
        let encodings = encode_dimensions(dataset, dimensions, dimension_count, index);
        let transforms = transforms_for_dimensions(dimension_count, intent);
        let mut spec = VisualizationSpec {
            id: format!("candidate-{index}"),
            title: format!(
                "{} {} view",
                dimension_label(dimension_count),
                mark.replace('-', " ")
            ),
            dimension_count,
            mark,
            layout,
            projection,
            encodings,
            transforms,
            fitness: FitnessBreakdown::default(),
            notes: vec![
                "Server-side spec only; renderers can map it to canvas, WebGL, Vega, or native deck layers."
                    .to_string(),
            ],
        };
        score_visualization(&mut spec, dataset, intent, ai_scores);
        candidates.push(spec);
    }
    candidates.sort_by(|left, right| right.fitness.total.total_cmp(&left.fitness.total));
    candidates
}

fn encode_dimensions(
    dataset: &Dataset,
    dimensions: &[String],
    dimension_count: usize,
    offset: usize,
) -> Vec<ChannelEncoding> {
    let channels = [
        "x",
        "y",
        "z",
        "color",
        "size",
        "shape",
        "time",
        "facet",
        "hyperSlice",
        "opacity",
        "texture",
        "smallMultiple",
    ];
    if dimensions.is_empty() {
        return Vec::new();
    }
    let mut encodings = Vec::new();
    for index in 0..dimension_count.min(channels.len()) {
        let field = dimensions[(index + offset) % dimensions.len()].clone();
        encodings.push(ChannelEncoding {
            channel: channels[index].to_string(),
            data_type: dataset.field_type(&field),
            field,
        });
    }
    encodings
}

fn transforms_for_dimensions(dimension_count: usize, intent: &str) -> Vec<String> {
    let mut transforms = vec![
        "profile columns".to_string(),
        "normalize numeric channels".to_string(),
    ];
    if dimension_count >= 4 {
        transforms.push("dictionary-code categorical channels for color/shape/facet".to_string());
    }
    if dimension_count >= 6 {
        transforms.push("project surplus dimensions into hyperSlice controls".to_string());
        transforms.push("generate paired small multiples for lost variance checks".to_string());
    }
    if intent.to_ascii_lowercase().contains("compare") {
        transforms.push("rank groups by primary quantitative channel".to_string());
    }
    transforms
}

fn score_visualization(
    spec: &mut VisualizationSpec,
    dataset: &Dataset,
    intent: &str,
    ai_scores: Option<&BTreeMap<String, f64>>,
) {
    let encoded_fields = spec
        .encodings
        .iter()
        .map(|encoding| encoding.field.clone())
        .collect::<BTreeSet<_>>();
    let numeric_channels = spec
        .encodings
        .iter()
        .filter(|encoding| encoding.data_type == "number")
        .count();
    let categorical_channels = spec
        .encodings
        .iter()
        .filter(|encoding| encoding.data_type == "category" || encoding.data_type == "boolean")
        .count();
    let coverage = encoded_fields.len() as f64 / dataset.columns.len().max(1) as f64;
    let information_density =
        (coverage * 0.65 + (spec.dimension_count as f64 / 12.0) * 0.35).clamp(0.0, 1.0);
    let legibility_penalty = if spec.dimension_count <= 3 {
        0.0
    } else {
        (spec.dimension_count as f64 - 3.0) * 0.045
    };
    let legibility =
        (0.92 - legibility_penalty + (numeric_channels as f64 * 0.015)).clamp(0.0, 1.0);
    let novelty = match spec.mark.as_str() {
        "hyper-slice-matrix" | "volume-cloud" | "radial-density" => 0.9,
        "surface" | "parallel-coordinates" => 0.78,
        _ => 0.48,
    };
    let intent_lower = intent.to_ascii_lowercase();
    let task_fit = if intent_lower.contains("compare") {
        if categorical_channels > 0 && numeric_channels > 0 {
            0.88
        } else {
            0.55
        }
    } else if intent_lower.contains("correlation") || intent_lower.contains("relationship") {
        if numeric_channels >= 2 {
            0.9
        } else {
            0.5
        }
    } else {
        0.72
    };
    let ai_evaluator = ai_scores
        .and_then(|scores| scores.get(&spec.id).copied())
        .unwrap_or(0.0)
        .clamp(0.0, 1.0);
    let total = information_density * 0.28
        + legibility * 0.28
        + novelty * 0.18
        + task_fit * 0.18
        + ai_evaluator * 0.08;

    spec.fitness = FitnessBreakdown {
        total: round4(total),
        information_density: round4(information_density),
        legibility: round4(legibility),
        novelty: round4(novelty),
        task_fit: round4(task_fit),
        ai_evaluator: round4(ai_evaluator),
    };
}

fn mutate_visualization(
    parent: &VisualizationSpec,
    dataset: &Dataset,
    dimensions: &[String],
    rng: &mut Lcg,
) -> VisualizationSpec {
    let marks = [
        "scatter",
        "bar",
        "line",
        "surface",
        "parallel-coordinates",
        "radial-density",
        "hyper-slice-matrix",
        "volume-cloud",
    ];
    let mut child = parent.clone();
    child.id = format!("{}-m{}", parent.id, rng.next_u32());
    child.mark = marks[rng.choose(marks.len())].to_string();
    let delta = match rng.choose(3) {
        0 => -1isize,
        1 => 0,
        _ => 1,
    };
    child.dimension_count = ((parent.dimension_count as isize + delta).clamp(2, 12)) as usize;
    child.layout = layout_for_dimensions(child.dimension_count).to_string();
    child.projection = projection_for_dimensions(child.dimension_count, rng.choose(7)).to_string();
    child.encodings = encode_dimensions(dataset, dimensions, child.dimension_count, rng.choose(11));
    child.transforms = transforms_for_dimensions(child.dimension_count, "evolved");
    child.notes =
        vec!["Mutated by channel rotation, mark swap, and projection rebalance.".to_string()];
    child
}

fn presentation_slides(request: &PresentationExportRequest) -> Vec<PresentationSlide> {
    let mut slides = vec![PresentationSlide {
        slide_id: "slide-1".to_string(),
        title: request.title.clone(),
        body: vec![request
            .subtitle
            .clone()
            .unwrap_or_else(|| "Analytics visualization brief".to_string())],
        visual_spec_id: None,
        speaker_notes: request.narrative.clone().unwrap_or_default(),
    }];

    for (index, spec) in request.specs.iter().enumerate() {
        slides.push(PresentationSlide {
            slide_id: format!("slide-{}", index + 2),
            title: spec.title.clone(),
            body: vec![
                format!("Mark: {}", spec.mark),
                format!("Layout: {}", spec.layout),
                format!("Projection: {}", spec.projection),
                format!("Fitness: {}", spec.fitness.total),
                format!(
                    "Channels: {}",
                    spec.encodings
                        .iter()
                        .map(|encoding| format!("{}={}", encoding.channel, encoding.field))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            ],
            visual_spec_id: Some(spec.id.clone()),
            speaker_notes: spec.notes.clone(),
        });
    }
    slides
}

fn powerpoint_open_xml_package(
    request: &PresentationExportRequest,
    slides: &[PresentationSlide],
) -> BTreeMap<String, String> {
    let mut files = BTreeMap::new();
    files.insert(
        "[Content_Types].xml".to_string(),
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
  <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
  <Default Extension="xml" ContentType="application/xml"/>
  <Override PartName="/ppt/presentation.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml"/>
</Types>"#
            .to_string(),
    );
    files.insert(
        "_rels/.rels".to_string(),
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="ppt/presentation.xml"/>
</Relationships>"#
            .to_string(),
    );

    let slide_ids = slides
        .iter()
        .enumerate()
        .map(|(index, _)| format!(r#"<p:sldId id="{}" r:id="rId{}"/>"#, 256 + index, index + 1))
        .collect::<Vec<_>>()
        .join("");
    files.insert(
        "ppt/presentation.xml".to_string(),
        format!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:presentation xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
  <p:sldIdLst>{slide_ids}</p:sldIdLst>
  <p:notesSz cx="6858000" cy="9144000"/>
</p:presentation>"#
        ),
    );
    files.insert(
        "ppt/_rels/presentation.xml.rels".to_string(),
        slides
            .iter()
            .enumerate()
            .map(|(index, _)| {
                format!(
                    r#"<Relationship Id="rId{}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide" Target="slides/slide{}.xml"/>"#,
                    index + 1,
                    index + 1
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
            .pipe(|rels| {
                format!(
                    r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
{rels}
</Relationships>"#
                )
            }),
    );

    for (index, slide) in slides.iter().enumerate() {
        let text = std::iter::once(slide.title.clone())
            .chain(slide.body.clone())
            .map(|line| format!(r#"<a:p><a:r><a:t>{}</a:t></a:r></a:p>"#, xml_escape(&line)))
            .collect::<Vec<_>>()
            .join("");
        files.insert(
            format!("ppt/slides/slide{}.xml", index + 1),
            format!(
                r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:sld xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main">
  <p:cSld>
    <p:spTree>
      <p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr>
      <p:grpSpPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="0" cy="0"/><a:chOff x="0" y="0"/><a:chExt cx="0" cy="0"/></a:xfrm></p:grpSpPr>
      <p:sp>
        <p:nvSpPr><p:cNvPr id="2" name="Content"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr>
        <p:spPr/>
        <p:txBody><a:bodyPr/><a:lstStyle/>{text}</p:txBody>
      </p:sp>
    </p:spTree>
  </p:cSld>
</p:sld>"#
            ),
        );
    }

    files.insert(
        "ppt/customXml/dd-data-viz-final-layer.json".to_string(),
        serde_json::to_string_pretty(&final_layer_json(request, slides)).unwrap_or_default(),
    );
    files
}

fn google_slides_batch_update(
    request: &PresentationExportRequest,
    slides: &[PresentationSlide],
) -> Value {
    let mut requests = Vec::new();
    for slide in slides {
        requests.push(json!({
            "createSlide": {
                "objectId": slide.slide_id,
                "slideLayoutReference": { "predefinedLayout": "TITLE_AND_BODY" }
            }
        }));
        requests.push(json!({
            "insertText": {
                "objectId": slide.slide_id,
                "text": format!("{}\n\n{}", slide.title, slide.body.join("\n"))
            }
        }));
    }
    json!({
        "presentationTitle": request.title,
        "requests": requests,
        "notes": "BatchUpdate blueprint; caller can bind generated visualization layers to placeholders."
    })
}

fn reveal_markdown(request: &PresentationExportRequest, slides: &[PresentationSlide]) -> String {
    let mut markdown = format!("# {}\n\n", request.title);
    if let Some(subtitle) = &request.subtitle {
        markdown.push_str(subtitle);
        markdown.push_str("\n\n");
    }
    for slide in slides.iter().skip(1) {
        markdown.push_str("---\n\n");
        markdown.push_str("## ");
        markdown.push_str(&slide.title);
        markdown.push_str("\n\n");
        for line in &slide.body {
            markdown.push_str("- ");
            markdown.push_str(line);
            markdown.push('\n');
        }
        if let Some(spec_id) = &slide.visual_spec_id {
            markdown.push_str("\n<!-- data-viz-spec: ");
            markdown.push_str(spec_id);
            markdown.push_str(" -->\n");
        }
        markdown.push('\n');
    }
    markdown
}

fn final_layer_json(request: &PresentationExportRequest, slides: &[PresentationSlide]) -> Value {
    json!({
        "schemaVersion": "data-viz.presentation-layer.v1",
        "title": request.title,
        "subtitle": request.subtitle,
        "slideCount": slides.len(),
        "slides": slides,
        "visualSpecs": request.specs,
        "rendererHints": {
            "preferred2d": "svg/canvas",
            "preferred3d": "webgl",
            "preferredXd": "small-multiple hyper-slice atlas with interactive dimension controls"
        }
    })
}

fn route_docs() -> Vec<RouteDoc> {
    vec![
        RouteDoc {
            method: "GET",
            path: "/",
            auth: "public",
            description: "HTML operator home.",
        },
        RouteDoc {
            method: "GET",
            path: "/descriptor",
            auth: "public",
            description: "Service descriptor, storage model, dialect catalog, and route map.",
        },
        RouteDoc {
            method: "GET",
            path: "/dialects",
            auth: "public",
            description: "Supported query dialect frontends and current subset notes.",
        },
        RouteDoc {
            method: "POST",
            path: "/datasets",
            auth: "dataset-write",
            description: "Ingest JSON records into a columnar in-memory dataset.",
        },
        RouteDoc {
            method: "GET",
            path: "/datasets",
            auth: "dataset-read",
            description: "List in-memory datasets and column profiles.",
        },
        RouteDoc {
            method: "GET",
            path: "/datasets/:dataset_id",
            auth: "dataset-read",
            description: "Return one dataset profile.",
        },
        RouteDoc {
            method: "POST",
            path: "/query",
            auth: "query-execute",
            description: "Translate a supported dialect into a LogicalPlan and execute it.",
        },
        RouteDoc {
            method: "POST",
            path: "/visualizations/suggest",
            auth: "visualization-suggest",
            description: "Generate 2D, 3D, 4D, 5D, or XD visualization specs.",
        },
        RouteDoc {
            method: "POST",
            path: "/evolution/run",
            auth: "evolution-run",
            description: "Run evolutionary visualization mutation and scoring.",
        },
        RouteDoc {
            method: "GET",
            path: "/evolution/runs",
            auth: "evolution-read",
            description: "List prior in-memory evolution run summaries.",
        },
        RouteDoc {
            method: "POST",
            path: "/presentations/export",
            auth: "presentation-export",
            description:
                "Emit PowerPoint OpenXML, Google Slides, Reveal markdown, and final JSON layers.",
        },
        RouteDoc {
            method: "GET",
            path: "/healthz",
            auth: "public",
            description: "Liveness probe.",
        },
        RouteDoc {
            method: "GET",
            path: "/readyz",
            auth: "public",
            description: "Readiness probe.",
        },
        RouteDoc {
            method: "GET",
            path: "/metrics",
            auth: "public",
            description: "Prometheus text metrics.",
        },
        RouteDoc {
            method: "GET",
            path: "/capabilities/parity",
            auth: "public",
            description: "BI and visualization-tool parity matrix with implemented surfaces and next engine work.",
        },
        RouteDoc {
            method: "GET",
            path: "/connectors/catalog",
            auth: "public",
            description: "Domo/Power Query-style connector and ETL planner catalog.",
        },
        RouteDoc {
            method: "GET",
            path: "/semantic/models",
            auth: "public",
            description: "Looker/Power BI-inspired semantic model, dimensions, measures, and calculations.",
        },
        RouteDoc {
            method: "POST",
            path: "/semantic/registry",
            auth: "semantic-write",
            description: "Create or replace a LookML-like governed semantic model validated against an ingested dataset.",
        },
        RouteDoc {
            method: "GET",
            path: "/semantic/registry",
            auth: "semantic-read",
            description: "List saved governed semantic model summaries.",
        },
        RouteDoc {
            method: "GET",
            path: "/semantic/registry/:model_id",
            auth: "semantic-read",
            description: "Read a saved governed semantic model definition.",
        },
        RouteDoc {
            method: "POST",
            path: "/semantic/registry/:model_id/compile",
            auth: "semantic-compile",
            description: "Compile governed dimensions and measures into a SQL query target and LogicalPlan.",
        },
        RouteDoc {
            method: "GET",
            path: "/workbooks/blueprints",
            auth: "public",
            description: "Sigma/Metabase-style workbook and self-service query-builder blueprints.",
        },
        RouteDoc {
            method: "GET",
            path: "/dashboards/panels",
            auth: "public",
            description: "Tableau/Superset/Grafana/D3/Plotly dashboard panel catalog.",
        },
        RouteDoc {
            method: "GET",
            path: "/renderers/contracts",
            auth: "public",
            description: "D3, Plotly/Dash, Evidence, and Office renderer/export contracts.",
        },
        RouteDoc {
            method: "GET",
            path: "/reports/evidence",
            auth: "public",
            description: "Evidence.dev-style Markdown plus SQL report blueprint.",
        },
        RouteDoc {
            method: "GET",
            path: "/security/policy",
            auth: "public",
            description: "Hardening, limit, control, and residual-risk report.",
        },
        RouteDoc {
            method: "GET",
            path: "/security/rbac",
            auth: "public",
            description: "Role and permission policy for protected analytics routes.",
        },
        RouteDoc {
            method: "GET",
            path: "/associations/:dataset_id",
            auth: "association-read",
            description: "Qlik-style associative graph over categorical fields in an ingested dataset.",
        },
        RouteDoc {
            method: "POST",
            path: "/associations/select",
            auth: "association-read",
            description: "Qlik-style multi-dataset associative selection state with possible, selected, alternative, and excluded values.",
        },
        RouteDoc {
            method: "POST",
            path: "/dashboards",
            auth: "dashboard-write",
            description: "Create or replace a saved dashboard definition backed by visualization specs.",
        },
        RouteDoc {
            method: "GET",
            path: "/dashboards",
            auth: "dashboard-read",
            description: "List saved dashboard summaries.",
        },
        RouteDoc {
            method: "GET",
            path: "/dashboards/:dashboard_id",
            auth: "dashboard-read",
            description: "Read a saved dashboard definition.",
        },
        RouteDoc {
            method: "POST",
            path: "/alerts/rules",
            auth: "alert-write",
            description: "Create or replace a Grafana-style alert rule over an analytical query.",
        },
        RouteDoc {
            method: "GET",
            path: "/alerts/rules",
            auth: "alert-read",
            description: "List saved alert rule summaries.",
        },
        RouteDoc {
            method: "GET",
            path: "/alerts/rules/:rule_id",
            auth: "alert-read",
            description: "Read a saved alert rule definition.",
        },
        RouteDoc {
            method: "POST",
            path: "/alerts/rules/:rule_id/evaluate",
            auth: "alert-evaluate",
            description: "Evaluate a saved alert rule and return normal, alerting, no-data, error, or disabled state.",
        },
        RouteDoc {
            method: "GET",
            path: "/docs/api, /api/docs, /api/docs.json",
            auth: "public",
            description: "Generated-from-route API documentation.",
        },
    ]
}

fn dialect_catalog() -> Vec<Value> {
    vec![
        dialect(
            "sql",
            "Parser-backed one-table SELECT/FROM/WHERE/GROUP BY/LIMIT analytics subset.",
        ),
        dialect(
            "graphql",
            "dataset(name:) with groupBy(field:) and aggregate field calls.",
        ),
        dialect(
            "promql",
            "sum/avg/min/max/count by (...) (...) metric-style subset.",
        ),
        dialect(
            "flux",
            "from(bucket:) |> group(columns:) |> sum/mean/min/max/count(column:) subset.",
        ),
        dialect(
            "influxql",
            "SQL-like SELECT mean/sum/min/max FROM measurement GROUP BY tags.",
        ),
        dialect("logql", "PromQL-style log metric aggregations."),
        dialect(
            "cypher",
            "MATCH label plus RETURN fields and aggregates subset.",
        ),
        dialect("gremlin", "hasLabel/group/by/values aggregate subset."),
        dialect("mongo", "JSON pipeline with $match and $group."),
        dialect("jmespath", "Projection and group_by(&field) subset."),
        dialect("lucene", "dataset:index token plus stats pipeline subset."),
        dialect(
            "spl",
            "search index=... | stats aggregate(field) by group subset.",
        ),
    ]
}

fn dialect(name: &str, notes: &str) -> Value {
    json!({
        "name": name,
        "frontend": "parser-subset",
        "internalPlan": "LogicalPlan",
        "notes": notes
    })
}

fn get_dataset_snapshot(state: &AppState, dataset_id: &str) -> Result<Dataset, ApiError> {
    let datasets = state
        .datasets
        .read()
        .map_err(|_| ApiError::bad_request("dataset store lock poisoned"))?;
    datasets
        .get(dataset_id)
        .cloned()
        .ok_or_else(|| ApiError::not_found(format!("dataset `{dataset_id}` not found")))
}

fn dataset_association_graph(dataset: &Dataset) -> Value {
    let categorical_fields = dataset
        .columns
        .iter()
        .filter_map(|(field, column)| match column {
            Column::Dictionary { .. } | Column::Boolean(_) => Some(field.clone()),
            Column::Number(_) => None,
        })
        .collect::<Vec<_>>();
    let nodes = categorical_fields
        .iter()
        .filter_map(|field| {
            dataset.columns.get(field).map(|column| {
                let profile = column.profile(field);
                json!({
                    "field": field,
                    "dataType": profile.data_type,
                    "missingCount": profile.missing_count,
                    "cardinality": profile.categorical.map(|item| item.cardinality).unwrap_or(0)
                })
            })
        })
        .collect::<Vec<_>>();
    let mut edge_counts = BTreeMap::<(String, String, String, String), usize>::new();
    let mut left_counts = BTreeMap::<(String, String), usize>::new();

    for row in 0..dataset.row_count {
        for left_index in 0..categorical_fields.len() {
            for right_index in left_index + 1..categorical_fields.len() {
                let left_field = &categorical_fields[left_index];
                let right_field = &categorical_fields[right_index];
                let left_value = dataset.value(left_field, row);
                let right_value = dataset.value(right_field, row);
                if left_value.is_null() || right_value.is_null() {
                    continue;
                }
                let left_label = scalar_to_label(&left_value);
                let right_label = scalar_to_label(&right_value);
                *left_counts
                    .entry((left_field.clone(), left_label.clone()))
                    .or_default() += 1;
                *edge_counts
                    .entry((
                        left_field.clone(),
                        left_label,
                        right_field.clone(),
                        right_label,
                    ))
                    .or_default() += 1;
            }
        }
    }

    let mut edges = edge_counts
        .into_iter()
        .map(
            |((left_field, left_value, right_field, right_value), support)| {
                let denominator = left_counts
                    .get(&(left_field.clone(), left_value.clone()))
                    .copied()
                    .unwrap_or(support)
                    .max(1);
                json!({
                    "from": {
                        "field": left_field,
                        "value": left_value
                    },
                    "to": {
                        "field": right_field,
                        "value": right_value
                    },
                    "support": support,
                    "confidence": round4(support as f64 / denominator as f64)
                })
            },
        )
        .collect::<Vec<_>>();
    edges.sort_by(|left, right| {
        right["support"]
            .as_u64()
            .cmp(&left["support"].as_u64())
            .then_with(|| {
                right["confidence"]
                    .as_f64()
                    .unwrap_or(0.0)
                    .total_cmp(&left["confidence"].as_f64().unwrap_or(0.0))
            })
    });
    edges.truncate(256);

    json!({
        "ok": true,
        "schemaVersion": "data-viz.associative-graph.v1",
        "datasetId": dataset.dataset_id,
        "rowCount": dataset.row_count,
        "categoricalFieldCount": categorical_fields.len(),
        "nodes": nodes,
        "edges": edges,
        "selectionModel": {
            "analog": "Qlik green/white/gray state",
            "currentStatus": "co-occurrence graph now; persistent selection-state engine planned"
        }
    })
}

fn authorize(
    state: &AppState,
    headers: &HeaderMap,
    permission: rbac::Permission,
) -> Result<rbac::AuthContext, ApiError> {
    let role = role_from_headers(headers)?;
    if state.config.allow_unauthenticated {
        return authorize_role(state, role.unwrap_or(rbac::Role::Admin), permission, true);
    }

    let Some(secret) = &state.config.server_auth_secret else {
        state
            .metrics
            .auth_failures_total
            .fetch_add(1, Ordering::Relaxed);
        return Err(ApiError::unauthorized(
            "SERVER_AUTH_SECRET is not configured and unauthenticated access is disabled",
        ));
    };

    let candidate = header_value(headers, "x-server-auth")
        .or_else(|| header_value(headers, "auth"))
        .or_else(|| {
            header_value(headers, "authorization")
                .and_then(|value| value.strip_prefix("Bearer ").map(str::to_string))
        });
    if candidate.as_deref() != Some(secret.as_str()) {
        state
            .metrics
            .auth_failures_total
            .fetch_add(1, Ordering::Relaxed);
        return Err(ApiError::unauthorized("operator auth failed"));
    }

    authorize_role(state, role.unwrap_or(rbac::Role::Admin), permission, false)
}

fn role_from_headers(headers: &HeaderMap) -> Result<Option<rbac::Role>, ApiError> {
    let Some(raw_role) =
        header_value(headers, "x-data-viz-role").or_else(|| header_value(headers, "x-dd-role"))
    else {
        return Ok(None);
    };
    rbac::Role::from_header(&raw_role)
        .map(Some)
        .ok_or_else(|| ApiError::unauthorized(format!("unknown data viz role `{raw_role}`")))
}

fn authorize_role(
    state: &AppState,
    role: rbac::Role,
    permission: rbac::Permission,
    local_bypass: bool,
) -> Result<rbac::AuthContext, ApiError> {
    if role.allows(permission) {
        return Ok(rbac::AuthContext {
            role,
            permission,
            local_bypass,
        });
    }
    state
        .metrics
        .rbac_denials_total
        .fetch_add(1, Ordering::Relaxed);
    Err(ApiError::unauthorized(format!(
        "role `{role:?}` does not allow `{permission:?}`"
    )))
}

fn filter_matches(dataset: &Dataset, row: usize, filter: Option<&FilterExpr>) -> bool {
    let Some(filter) = filter else {
        return true;
    };
    let left = dataset.value(&filter.field, row);
    match filter.op.as_str() {
        "=" | "==" => scalar_to_label(&left) == scalar_to_label(&filter.value),
        "!=" => scalar_to_label(&left) != scalar_to_label(&filter.value),
        ">" => left.as_f64().unwrap_or(f64::NAN) > filter.value.as_f64().unwrap_or(f64::NAN),
        ">=" => left.as_f64().unwrap_or(f64::NAN) >= filter.value.as_f64().unwrap_or(f64::NAN),
        "<" => left.as_f64().unwrap_or(f64::NAN) < filter.value.as_f64().unwrap_or(f64::NAN),
        "<=" => left.as_f64().unwrap_or(f64::NAN) <= filter.value.as_f64().unwrap_or(f64::NAN),
        _ => true,
    }
}

fn requested_dimensions(dataset: &Dataset, requested: Option<&[String]>) -> Vec<String> {
    let requested = requested
        .map(|fields| {
            fields
                .iter()
                .filter_map(|field| clean_field(field))
                .filter(|field| dataset.columns.contains_key(field))
                .collect::<Vec<_>>()
        })
        .filter(|fields| !fields.is_empty());
    requested.unwrap_or_else(|| {
        let mut numeric = Vec::new();
        let mut categorical = Vec::new();
        for (field, column) in &dataset.columns {
            match column {
                Column::Number(_) => numeric.push(field.clone()),
                _ => categorical.push(field.clone()),
            }
        }
        numeric.extend(categorical);
        numeric
    })
}

fn ai_score_map(evaluations: Option<&[AiEvaluation]>) -> BTreeMap<String, f64> {
    evaluations
        .unwrap_or_default()
        .iter()
        .map(|evaluation| {
            (
                evaluation.candidate_id.clone(),
                evaluation.score.clamp(0.0, 1.0),
            )
        })
        .collect()
}

fn evaluator_prompt(dataset: &Dataset, objective: &str) -> String {
    format!(
        "You are evaluating candidate data visualizations for dataset `{}` with {} rows and {} columns. Objective: {}. Score each candidate from 0.0 to 1.0 for truthful insight, perceptual clarity, dimensional coverage, and executive usefulness. Penalize charts that hide uncertainty, overload color/shape, or imply unsupported causality.",
        dataset.dataset_id,
        dataset.row_count,
        dataset.columns.len(),
        objective
    )
}

fn layout_for_dimensions(dimension_count: usize) -> &'static str {
    match dimension_count {
        0..=2 => "2d-cartesian",
        3 => "3d-scene",
        4 => "4d-encoded-scene",
        5 => "5d-faceted-hypercube",
        _ => "xd-projection-atlas",
    }
}

fn projection_for_dimensions(dimension_count: usize, variant: usize) -> &'static str {
    match (dimension_count, variant % 5) {
        (0..=2, _) => "direct-axis",
        (3, 0) => "orthographic-xyz",
        (3, _) => "rotatable-perspective",
        (4, 0) => "xyz-plus-color",
        (4, _) => "xyz-plus-size",
        (5, 0) => "small-multiple-hypercube",
        (5, _) => "parallel-slices",
        (_, 0) => "random-projection-atlas",
        (_, 1) => "radial-hyper-slices",
        (_, 2) => "parallel-coordinate-brushes",
        (_, 3) => "facet-grid-plus-embedding",
        _ => "semantic-force-layout",
    }
}

fn dimension_label(dimension_count: usize) -> String {
    match dimension_count {
        2 => "2D".to_string(),
        3 => "3D".to_string(),
        4 => "4D".to_string(),
        5 => "5D".to_string(),
        other => format!("{other}D/XD"),
    }
}

fn split_sql_clauses(input: &str) -> (&str, Option<&str>, Option<&str>) {
    let mut source = input.trim();
    let mut where_part = None;
    let mut group_part = None;

    if let Some(where_idx) = find_ascii_case(source, " WHERE ") {
        let before_where = &source[..where_idx];
        let after_where = &source[where_idx + " WHERE ".len()..];
        source = before_where;
        if let Some(group_idx) = find_ascii_case(after_where, " GROUP BY ") {
            where_part = Some(after_where[..group_idx].trim());
            group_part = Some(after_where[group_idx + " GROUP BY ".len()..].trim());
        } else {
            where_part = Some(after_where.trim());
        }
    } else if let Some(group_idx) = find_ascii_case(source, " GROUP BY ") {
        let before_group = &source[..group_idx];
        let after_group = &source[group_idx + " GROUP BY ".len()..];
        source = before_group;
        group_part = Some(after_group.trim());
    }

    (source.trim(), where_part, group_part)
}

fn split_optional_limit(input: &str, default_limit: usize) -> (&str, usize) {
    if let Some(limit_idx) = find_ascii_case(input, " LIMIT ") {
        let limit_part = input[limit_idx + " LIMIT ".len()..]
            .split_whitespace()
            .next()
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(default_limit)
            .clamp(1, MAX_QUERY_ROWS);
        (&input[..limit_idx], limit_part)
    } else {
        (input, default_limit)
    }
}

fn parse_aggregation_item(item: &str) -> Option<AggregationExpr> {
    let trimmed = item.trim();
    let (expression, alias) = split_alias(trimmed);
    let open = expression.find('(')?;
    let close = expression.rfind(')')?;
    let op_text = expression[..open].trim().to_ascii_lowercase();
    let field_text = expression[open + 1..close].trim();
    let op = match op_text.as_str() {
        "count" => AggregationOp::Count,
        "sum" => AggregationOp::Sum,
        "avg" | "mean" => AggregationOp::Avg,
        "min" => AggregationOp::Min,
        "max" => AggregationOp::Max,
        _ => return None,
    };
    let field = if field_text == "*" || op == AggregationOp::Count {
        None
    } else {
        clean_field(field_text)
    };
    Some(AggregationExpr {
        alias: alias.unwrap_or_else(|| {
            field
                .as_ref()
                .map(|field| format!("{}_{}", op.as_str(), field))
                .unwrap_or_else(|| op.as_str().to_string())
        }),
        op,
        field,
    })
}

fn split_alias(expression: &str) -> (&str, Option<String>) {
    if let Some(idx) = find_ascii_case(expression, " AS ") {
        let alias = clean_identifier(expression[idx + " AS ".len()..].trim());
        (&expression[..idx], alias)
    } else {
        (expression, None)
    }
}

fn parse_filter_expr(input: &str) -> Option<FilterExpr> {
    for op in [">=", "<=", "!=", "=", ">", "<"] {
        if let Some(idx) = input.find(op) {
            let field = clean_field(&input[..idx])?;
            let raw_value = input[idx + op.len()..]
                .trim()
                .trim_matches('\'')
                .trim_matches('"');
            let value = raw_value
                .parse::<f64>()
                .map(Value::from)
                .unwrap_or_else(|_| Value::from(raw_value.to_string()));
            return Some(FilterExpr {
                field,
                op: op.to_string(),
                value,
            });
        }
    }
    None
}

fn parse_field_list(input: &str) -> Vec<String> {
    input
        .split(',')
        .flat_map(|item| item.split_whitespace())
        .map(|item| item.trim().trim_matches(',').to_string())
        .filter(|item| !item.is_empty() && !item.eq_ignore_ascii_case("stats"))
        .collect()
}

fn extract_between<'a>(input: &'a str, start: &str, end: &str) -> Option<&'a str> {
    let start_idx = find_ascii_case(input, start)?;
    let after_start = &input[start_idx + start.len()..];
    let end_idx = find_ascii_case(after_start, end)?;
    Some(&after_start[..end_idx])
}

fn extract_arg(input: &str, name: &str) -> Option<String> {
    let needle = format!("{name}:");
    let idx = find_ascii_case(input, &needle)?;
    let after = input[idx + needle.len()..].trim_start();
    if let Some(stripped) = after.strip_prefix('"') {
        return stripped.split('"').next().map(str::to_string);
    }
    if let Some(stripped) = after.strip_prefix('\'') {
        return stripped.split('\'').next().map(str::to_string);
    }
    after
        .split(|ch: char| ch == ',' || ch == ')' || ch == '}' || ch.is_whitespace())
        .next()
        .map(str::to_string)
}

fn extract_arg_after(input: &str, marker: &str, arg: &str) -> Option<String> {
    let marker_idx = find_ascii_case(input, marker)?;
    extract_arg(&input[marker_idx..], arg)
}

fn extract_last_parenthesized(input: &str) -> Option<String> {
    let open = input.rfind('(')?;
    let close = input.rfind(')')?;
    if close > open {
        Some(input[open + 1..close].trim().to_string())
    } else {
        None
    }
}

fn extract_token_value(input: &str, key: &str) -> Option<String> {
    for token in input.split_whitespace() {
        if let Some(value) = token.strip_prefix(&format!("{key}:")) {
            return Some(value.trim_matches('"').trim_matches('\'').to_string());
        }
        if let Some(value) = token.strip_prefix(&format!("{key}=")) {
            return Some(value.trim_matches('"').trim_matches('\'').to_string());
        }
    }
    None
}

fn log_event(severity: &str, event_name: &str, message: &str, attributes: Value) {
    let severity_number = match severity {
        "ERROR" => 17,
        "WARN" => 13,
        "INFO" => 9,
        _ => 5,
    };
    println!(
        "{}",
        json!({
            "schema": "dd.log.v1",
            "timestamp_ms": now_ms(),
            "severity_text": severity,
            "severity_number": severity_number,
            "resource_service_name": SERVICE_NAME,
            "event_name": event_name,
            "body": message,
            "attributes": attributes
        })
    );
}

fn sample_records() -> Vec<BTreeMap<String, Value>> {
    [
        json!({"region":"north","segment":"enterprise","revenue":1200.0,"margin":0.31,"churn":0.04,"latencyMs":44.0}),
        json!({"region":"north","segment":"smb","revenue":760.0,"margin":0.24,"churn":0.08,"latencyMs":51.0}),
        json!({"region":"south","segment":"enterprise","revenue":980.0,"margin":0.29,"churn":0.05,"latencyMs":47.0}),
        json!({"region":"west","segment":"consumer","revenue":1320.0,"margin":0.19,"churn":0.11,"latencyMs":66.0}),
    ]
    .into_iter()
    .map(|value| {
        value
            .as_object()
            .unwrap()
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect()
    })
    .collect()
}

struct Lcg {
    state: u64,
}

impl Lcg {
    fn new(seed: u64) -> Self {
        Self { state: seed.max(1) }
    }

    fn next_u32(&mut self) -> u32 {
        self.state = self
            .state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (self.state >> 32) as u32
    }

    fn choose(&mut self, len: usize) -> usize {
        if len == 0 {
            0
        } else {
            self.next_u32() as usize % len
        }
    }
}

trait Pipe: Sized {
    fn pipe<T>(self, function: impl FnOnce(Self) -> T) -> T {
        function(self)
    }
}

impl<T> Pipe for T {}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_dataset() -> Dataset {
        Dataset::from_request(IngestDatasetRequest {
            dataset_id: "sales-lab".to_string(),
            display_name: Some("Sales Lab".to_string()),
            replace: Some(true),
            records: sample_records(),
        })
        .expect("dataset builds")
    }

    fn test_state() -> AppState {
        AppState {
            config: Arc::new(Config {
                host: DEFAULT_HOST.to_string(),
                port: DEFAULT_PORT,
                server_auth_secret: Some("unit-secret".to_string()),
                allow_unauthenticated: false,
            }),
            metrics: Arc::new(Metrics::default()),
            datasets: Arc::new(RwLock::new(BTreeMap::new())),
            evolution_runs: Arc::new(RwLock::new(BTreeMap::new())),
            dashboards: Arc::new(RwLock::new(BTreeMap::new())),
            alert_rules: Arc::new(RwLock::new(BTreeMap::new())),
            semantic_models: Arc::new(RwLock::new(BTreeMap::new())),
        }
    }

    fn auth_headers(role: Option<&str>) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert("x-server-auth", "unit-secret".parse().unwrap());
        if let Some(role) = role {
            headers.insert("x-data-viz-role", role.parse().unwrap());
        }
        headers
    }

    #[test]
    fn ingest_uses_dictionary_encoding_for_categories() {
        let dataset = sample_dataset();
        assert_eq!(dataset.row_count, 4);
        assert!(matches!(
            dataset.columns.get("region").unwrap(),
            Column::Dictionary { .. }
        ));
        assert!(matches!(
            dataset.columns.get("revenue").unwrap(),
            Column::Number(_)
        ));
        let profile = dataset.metadata();
        assert_eq!(profile.column_count, 6);
    }

    #[test]
    fn sql_aggregation_executes_against_columnar_dataset() {
        let dataset = sample_dataset();
        let request = QueryRequest {
            dialect: QueryDialect::Sql,
            dataset_id: Some("sales-lab".to_string()),
            query: "SELECT region, SUM(revenue) AS totalRevenue, AVG(margin) AS avgMargin FROM sales-lab GROUP BY region LIMIT 10".to_string(),
            limit: None,
        };
        let plan = logical_plan_from_query(&request).expect("plan builds");
        let response = execute_plan(&dataset, plan).expect("query executes");
        assert_eq!(response.row_count, 3);
        assert!(response.rows.iter().any(|row| {
            row.get("region") == Some(&Value::from("north"))
                && row.get("totalRevenue") == Some(&Value::from(1960.0))
        }));
    }

    #[test]
    fn mongo_and_promql_translate_to_same_plan_shape() {
        let mongo = logical_plan_from_query(&QueryRequest {
            dialect: QueryDialect::Mongo,
            dataset_id: Some("sales-lab".to_string()),
            query: r#"[{"$group":{"_id":"$region","total":{"$sum":"$revenue"}}}]"#.to_string(),
            limit: None,
        })
        .expect("mongo plan");
        let promql = logical_plan_from_query(&QueryRequest {
            dialect: QueryDialect::PromQl,
            dataset_id: Some("sales-lab".to_string()),
            query: "sum by (region) (revenue)".to_string(),
            limit: None,
        })
        .expect("promql plan");

        assert_eq!(mongo.source, promql.source);
        assert_eq!(mongo.group_by, promql.group_by);
        assert_eq!(mongo.aggregations[0].op, promql.aggregations[0].op);
    }

    #[test]
    fn visualization_candidates_cover_xd_specs() {
        let dataset = sample_dataset();
        let dims = requested_dimensions(&dataset, None);
        let candidates = visualization_candidates(&dataset, &dims, 7, "compare segments", 6, None);
        assert!(!candidates.is_empty());
        assert!(candidates
            .iter()
            .any(|candidate| candidate.layout == "xd-projection-atlas"));
        assert!(candidates[0].fitness.total > 0.0);
    }

    #[test]
    fn evolution_keeps_population_scored() {
        let dataset = sample_dataset();
        let dims = requested_dimensions(&dataset, None);
        let mut rng = Lcg::new(42);
        let mut child = mutate_visualization(
            &visualization_candidates(&dataset, &dims, 5, "compare", 1, None)[0],
            &dataset,
            &dims,
            &mut rng,
        );
        score_visualization(&mut child, &dataset, "compare", None);
        assert!(child.fitness.total > 0.0);
        assert!(!child.encodings.is_empty());
    }

    #[test]
    fn presentation_export_contains_powerpoint_and_google_layers() {
        let dataset = sample_dataset();
        let dims = requested_dimensions(&dataset, None);
        let spec = visualization_candidates(&dataset, &dims, 5, "compare", 1, None)
            .into_iter()
            .next()
            .unwrap();
        let request = PresentationExportRequest {
            title: "Quarterly Revenue".to_string(),
            subtitle: Some("Generated from dd-data-viz-rs".to_string()),
            narrative: None,
            specs: vec![spec],
            format: Some(PresentationFormat::All),
        };
        let slides = presentation_slides(&request);
        let ppt = powerpoint_open_xml_package(&request, &slides);
        let google = google_slides_batch_update(&request, &slides);
        assert!(ppt.contains_key("ppt/presentation.xml"));
        assert!(ppt.contains_key("ppt/customXml/dd-data-viz-final-layer.json"));
        assert_eq!(
            google["requests"].as_array().unwrap().len(),
            slides.len() * 2
        );
    }

    #[test]
    fn associative_graph_links_categorical_values() {
        let dataset = sample_dataset();
        let graph = dataset_association_graph(&dataset);
        let edges = graph["edges"].as_array().expect("edges array");

        assert_eq!(graph["ok"], true);
        assert_eq!(graph["categoricalFieldCount"], 2);
        assert!(edges.iter().any(|edge| {
            edge["from"]["field"] == "region"
                && edge["from"]["value"] == "north"
                && edge["to"]["field"] == "segment"
                && edge["to"]["value"] == "enterprise"
        }));
    }

    #[test]
    fn route_docs_cover_platform_and_hardening_surfaces() {
        let paths = route_docs()
            .into_iter()
            .map(|route| route.path)
            .collect::<Vec<_>>();

        assert!(paths.contains(&"/capabilities/parity"));
        assert!(paths.contains(&"/semantic/models"));
        assert!(paths.contains(&"/associations/:dataset_id"));
        assert!(paths.contains(&"/security/policy"));
        assert!(paths.contains(&"/security/rbac"));
        assert!(paths.contains(&"/dashboards"));
        assert!(paths.contains(&"/dashboards/:dashboard_id"));
        assert!(paths.contains(&"/associations/select"));
        assert!(paths.contains(&"/alerts/rules"));
        assert!(paths.contains(&"/alerts/rules/:rule_id"));
        assert!(paths.contains(&"/alerts/rules/:rule_id/evaluate"));
        assert!(paths.contains(&"/semantic/registry"));
        assert!(paths.contains(&"/semantic/registry/:model_id"));
        assert!(paths.contains(&"/semantic/registry/:model_id/compile"));
    }

    #[test]
    fn rbac_authorization_allows_reader_and_denies_writer() {
        let state = test_state();

        authorize(
            &state,
            &auth_headers(Some("viewer")),
            rbac::Permission::DatasetRead,
        )
        .expect("viewer can read datasets");
        let error = authorize(
            &state,
            &auth_headers(Some("viewer")),
            rbac::Permission::DatasetWrite,
        )
        .expect_err("viewer cannot write datasets");

        assert_eq!(error.status, StatusCode::UNAUTHORIZED);
        assert_eq!(state.metrics.rbac_denials_total.load(Ordering::Relaxed), 1);
    }
}
