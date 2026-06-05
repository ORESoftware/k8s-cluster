use std::{
    collections::{BTreeMap, BTreeSet},
    env,
    error::Error,
    fmt,
    net::SocketAddr,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use axum::{
    extract::{DefaultBodyLimit, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use dd_nats_subject_defs::{
    FABRICATION_REQUESTS_QUEUE_GROUP, FABRICATION_REQUESTS_SUBJECT, FABRICATION_RESULTS_SUBJECT,
    MDP_OPTIMIZE_SUBJECT, RUNTIME_EVENTS_SUBJECT,
};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::json;

const MAX_HTTP_BODY_BYTES: usize = 512 * 1024;
const MAX_NATS_PAYLOAD_BYTES: usize = 512 * 1024;
const MAX_INSTRUCTION_BYTES: usize = 256 * 1024;
const MAX_EXISTING_INSTRUCTIONS: usize = 16;

#[derive(Clone)]
struct AppState {
    nats: Option<async_nats::Client>,
    result_subject: String,
    event_subject: String,
    mdp_optimize_subject: String,
    mdp_auto_publish: bool,
    metrics: Arc<Metrics>,
}

#[derive(Default)]
struct Metrics {
    requests_total: AtomicU64,
    instruction_validations_total: AtomicU64,
    telemetry_learning_total: AtomicU64,
    generated_instructions_total: AtomicU64,
    findings_total: AtomicU64,
    mdp_delegations_total: AtomicU64,
    errors_total: AtomicU64,
    nats_messages_total: AtomicU64,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
enum MaterialClass {
    Plastic,
    Resin,
    Metal,
    Wood,
    Composite,
    Ceramic,
    Wax,
    Unknown,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
enum MachineKind {
    FdmPrinter,
    SlaPrinter,
    SlsPrinter,
    DlpPrinter,
    BinderJet,
    VerticalMill,
    HorizontalMill,
    Lathe,
    CncRouter,
    LaserCutter,
    Waterjet,
    WireEdm,
    ManualAssembly,
    Unknown,
}

impl MachineKind {
    fn default_instruction_format(self) -> InstructionFormat {
        match self {
            MachineKind::FdmPrinter => InstructionFormat::MarlinGcode,
            MachineKind::SlaPrinter
            | MachineKind::SlsPrinter
            | MachineKind::DlpPrinter
            | MachineKind::BinderJet => InstructionFormat::PrinterJob,
            MachineKind::VerticalMill | MachineKind::HorizontalMill => InstructionFormat::HaasGcode,
            MachineKind::Lathe => InstructionFormat::FanucGcode,
            MachineKind::CncRouter => InstructionFormat::Grbl,
            MachineKind::LaserCutter | MachineKind::Waterjet | MachineKind::WireEdm => {
                InstructionFormat::Gcode
            }
            MachineKind::ManualAssembly | MachineKind::Unknown => InstructionFormat::SetupSheet,
        }
    }

    fn is_printer(self) -> bool {
        matches!(
            self,
            MachineKind::FdmPrinter
                | MachineKind::SlaPrinter
                | MachineKind::SlsPrinter
                | MachineKind::DlpPrinter
                | MachineKind::BinderJet
        )
    }

    fn is_mill_like(self) -> bool {
        matches!(
            self,
            MachineKind::VerticalMill
                | MachineKind::HorizontalMill
                | MachineKind::CncRouter
                | MachineKind::Waterjet
                | MachineKind::WireEdm
        )
    }
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
enum InstructionFormat {
    Gcode,
    MarlinGcode,
    KlipperGcode,
    PrusaGcode,
    HaasGcode,
    FanucGcode,
    Grbl,
    Shopbot,
    Heidenhain,
    Mazatrol,
    PrinterJob,
    SetupSheet,
    Unknown,
}

impl InstructionFormat {
    fn is_gcode_like(self) -> bool {
        matches!(
            self,
            InstructionFormat::Gcode
                | InstructionFormat::MarlinGcode
                | InstructionFormat::KlipperGcode
                | InstructionFormat::PrusaGcode
                | InstructionFormat::HaasGcode
                | InstructionFormat::FanucGcode
                | InstructionFormat::Grbl
        )
    }
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
enum LearningMode {
    None,
    Mdp,
    Pomdp,
    NeuralPolicy,
    Hybrid,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
enum FabricationProcess {
    FdmPrint,
    ResinPrint,
    PowderPrint,
    MillRoughing,
    MillFinishing,
    Turn,
    Route,
    Cut,
    Inspect,
    Assemble,
    ValidateExisting,
    ImproveExisting,
    ManualReview,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
enum BoundarySeverity {
    Info,
    Warning,
    Error,
    Critical,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
enum BoundaryKind {
    WorkEnvelopeExceeded,
    UnsupportedMaterial,
    ToleranceTooTight,
    HumanInterventionRequired,
    ThermalLimit,
    FeedRateLimit,
    SpindleLimit,
    ToolingRequired,
    SetupChangeRequired,
    UnsupportedCommand,
    UnknownInstruction,
    CollisionRisk,
    MissingHoming,
    MissingAbsoluteMode,
    SplitRequired,
    AssemblyRequired,
    NoCapableMachine,
    LearningDataNeeded,
    Improvement,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BoundingBoxMm {
    x: f64,
    y: f64,
    z: f64,
}

impl BoundingBoxMm {
    fn max_axis(&self) -> f64 {
        self.x.max(self.y).max(self.z)
    }

    fn fits_inside(&self, other: &BoundingBoxMm) -> bool {
        self.x <= other.x && self.y <= other.y && self.z <= other.z
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct FabricationRequest {
    request_id: Option<String>,
    objective: FabricationObjective,
    available_machines: Vec<Machine>,
    stock: Option<Vec<MaterialStock>>,
    existing_instructions: Option<Vec<ExistingInstruction>>,
    learning: Option<LearningConfig>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct FabricationObjective {
    name: String,
    description: Option<String>,
    material: Option<String>,
    material_class: Option<MaterialClass>,
    quantity: Option<u32>,
    bounding_box_mm: Option<BoundingBoxMm>,
    tolerance_mm: Option<f64>,
    surface_finish: Option<String>,
    mass_g: Option<f64>,
    rotational_symmetry: Option<bool>,
    hollow: Option<bool>,
    enclosed_cavities: Option<bool>,
    overhang_degrees: Option<f64>,
    min_wall_mm: Option<f64>,
    strength_priority: Option<f64>,
    aesthetic_priority: Option<f64>,
    inspectability_priority: Option<f64>,
    required_features: Option<Vec<String>>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct Machine {
    id: String,
    kind: MachineKind,
    capabilities: Option<MachineCapabilities>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct MachineCapabilities {
    work_envelope_mm: Option<BoundingBoxMm>,
    materials: Option<Vec<String>>,
    axes: Option<u8>,
    nozzle_diameter_mm: Option<f64>,
    tool_diameters_mm: Option<Vec<f64>>,
    max_spindle_rpm: Option<f64>,
    max_feed_mm_min: Option<f64>,
    min_layer_height_mm: Option<f64>,
    min_tolerance_mm: Option<f64>,
    max_extruder_temp_c: Option<f64>,
    max_bed_temp_c: Option<f64>,
    supports_tool_change: Option<bool>,
    supports_auto_homing: Option<bool>,
    notes: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct MaterialStock {
    stock_id: String,
    material: String,
    bounding_box_mm: Option<BoundingBoxMm>,
    diameter_mm: Option<f64>,
    length_mm: Option<f64>,
    quantity: Option<u32>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct ExistingInstruction {
    instruction_id: Option<String>,
    machine_id: Option<String>,
    machine_kind: Option<MachineKind>,
    format: Option<InstructionFormat>,
    content: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct LearningConfig {
    mode: Option<LearningMode>,
    horizon: Option<u32>,
    exploration_budget: Option<f64>,
    hidden_state_hint: Option<Vec<String>>,
    reward_weights: Option<Vec<RewardWeight>>,
    neural_policy_hint: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct RewardWeight {
    signal: String,
    weight: f64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct InstructionValidationRequest {
    request_id: Option<String>,
    machines: Option<Vec<Machine>>,
    instructions: Vec<ExistingInstruction>,
    improve: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct FabricationTelemetryRequest {
    request_id: Option<String>,
    plan_id: Option<String>,
    learning: Option<LearningConfig>,
    events: Vec<FabricationTelemetryEvent>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct FabricationTelemetryEvent {
    operation_id: Option<String>,
    machine_id: Option<String>,
    process: Option<FabricationProcess>,
    metric: String,
    value: f64,
    target: Option<f64>,
    weight: Option<f64>,
    success: Option<bool>,
    observation: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct FabricationResponse {
    ok: bool,
    request_id: String,
    plan_id: String,
    status: String,
    generated_at_ms: u128,
    design: DesignPackage,
    plan: FabricationPlan,
    instructions: Vec<InstructionArtifact>,
    validation: Vec<InstructionAnalysis>,
    boundaries: Vec<BoundaryFinding>,
    learning: LearningPlan,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct DesignPackage {
    design_id: String,
    artifacts: Vec<DesignArtifact>,
    decomposition: Vec<PartSpec>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct DesignArtifact {
    artifact_id: String,
    format: String,
    content: String,
    notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct PartSpec {
    part_id: String,
    name: String,
    material_class: MaterialClass,
    bounding_box_mm: Option<BoundingBoxMm>,
    process_hint: FabricationProcess,
    join_strategy: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct FabricationPlan {
    operations: Vec<FabricationOperation>,
    assembly: Vec<AssemblyStep>,
    expected_human_intervention: bool,
    estimated_risk: f64,
    process_summary: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct FabricationOperation {
    operation_id: String,
    process: FabricationProcess,
    machine_id: Option<String>,
    machine_kind: MachineKind,
    part_id: String,
    intent: String,
    setup_notes: Vec<String>,
    estimated_risk: f64,
    instruction_ref: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct AssemblyStep {
    step_id: String,
    description: String,
    required_after_operations: Vec<String>,
    intervention: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct InstructionArtifact {
    instruction_id: String,
    operation_id: String,
    machine_id: Option<String>,
    machine_kind: MachineKind,
    format: InstructionFormat,
    content: String,
    safety_notes: Vec<String>,
    generated: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct InstructionAnalysis {
    instruction_id: String,
    format: InstructionFormat,
    line_count: usize,
    estimated_bounds_mm: Option<InstructionBounds>,
    commands: Vec<String>,
    findings: Vec<BoundaryFinding>,
    improvement_summary: Vec<String>,
    improved_content: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct InstructionBounds {
    min_x: Option<f64>,
    max_x: Option<f64>,
    min_y: Option<f64>,
    max_y: Option<f64>,
    min_z: Option<f64>,
    max_z: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct BoundaryFinding {
    kind: BoundaryKind,
    severity: BoundarySeverity,
    message: String,
    machine_id: Option<String>,
    operation_id: Option<String>,
    line: Option<usize>,
    recommendation: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct LearningPlan {
    mode: LearningMode,
    mdp_states: Vec<String>,
    actions: Vec<String>,
    observations: Vec<String>,
    reward_signals: Vec<RewardWeight>,
    mdp_request: Option<MdpOptimizationRequest>,
    mdp_publish_subject: Option<String>,
    pomdp_hidden_state_hints: Vec<String>,
    neural_training_hints: Vec<String>,
    delegation_hints: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct MdpOptimizationRequest {
    request_id: Option<String>,
    kind: Option<String>,
    states: Vec<String>,
    actions: Vec<String>,
    transitions: Vec<MdpTransition>,
    rewards: Vec<MdpReward>,
    observations: Option<Vec<String>>,
    observation_model: Option<Vec<MdpObservation>>,
    belief: Option<Vec<MdpBelief>>,
    belief_action: Option<String>,
    observed: Option<String>,
    gamma: Option<f64>,
    tolerance: Option<f64>,
    max_iterations: Option<usize>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct MdpTransition {
    state: String,
    action: String,
    next_state: String,
    probability: f64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct MdpReward {
    state: String,
    action: String,
    value: f64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct MdpObservation {
    action: String,
    next_state: String,
    observation: String,
    probability: f64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct MdpBelief {
    state: String,
    probability: f64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct TelemetryLearningResponse {
    ok: bool,
    request_id: String,
    plan_id: Option<String>,
    reward: f64,
    risk_state: String,
    policy_adjustments: Vec<String>,
    mdp_signals: Vec<RewardWeight>,
    neural_examples: Vec<String>,
    generated_at_ms: u128,
}

impl fmt::Display for MachineKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{self:?}")
    }
}

async fn healthz() -> impl IntoResponse {
    Json(json!({
        "ok": true,
        "service": "dd-fabrication-server",
        "mode": "fabrication-planning-validation-learning",
        "atMs": now_ms(),
    }))
}

async fn schema() -> impl IntoResponse {
    Json(json!({
        "ok": true,
        "schemaVersion": "fabrication.v1",
        "service": "dd-fabrication-server",
        "supportedMachineKinds": [
            "fdmPrinter", "slaPrinter", "slsPrinter", "dlpPrinter", "binderJet",
            "verticalMill", "horizontalMill", "lathe", "cncRouter", "laserCutter",
            "waterjet", "wireEdm", "manualAssembly"
        ],
        "supportedInstructionFormats": [
            "gcode", "marlinGcode", "klipperGcode", "prusaGcode", "haasGcode",
            "fanucGcode", "grbl", "shopbot", "heidenhain", "mazatrol",
            "printerJob", "setupSheet"
        ],
        "primaryRoutes": {
            "fabricate": "POST /fabricate",
            "validateInstructions": "POST /instructions/validate",
            "improveInstructions": "POST /instructions/improve",
            "learnTelemetry": "POST /learn/telemetry"
        }
    }))
}

async fn example() -> impl IntoResponse {
    Json(json!({
        "requestId": "demo-bracket-001",
        "objective": {
            "name": "sensor bracket",
            "description": "Lightweight bracket with a milled datum face and printed body.",
            "material": "PETG",
            "materialClass": "plastic",
            "quantity": 2,
            "boundingBoxMm": { "x": 90.0, "y": 45.0, "z": 28.0 },
            "toleranceMm": 0.08,
            "overhangDegrees": 55.0,
            "minWallMm": 1.2,
            "strengthPriority": 0.8
        },
        "availableMachines": [
            {
                "id": "prusa-xl",
                "kind": "fdmPrinter",
                "capabilities": {
                    "workEnvelopeMm": { "x": 360.0, "y": 360.0, "z": 360.0 },
                    "materials": ["PLA", "PETG", "ABS"],
                    "nozzleDiameterMm": 0.4,
                    "minLayerHeightMm": 0.08,
                    "minToleranceMm": 0.2,
                    "maxExtruderTempC": 300.0,
                    "maxBedTempC": 120.0
                }
            },
            {
                "id": "tm1p",
                "kind": "verticalMill",
                "capabilities": {
                    "workEnvelopeMm": { "x": 760.0, "y": 300.0, "z": 400.0 },
                    "materials": ["aluminum", "steel", "plastic"],
                    "axes": 3,
                    "toolDiametersMm": [3.175, 6.0, 10.0],
                    "maxSpindleRpm": 6000.0,
                    "maxFeedMmMin": 1200.0,
                    "minToleranceMm": 0.03
                }
            }
        ],
        "learning": {
            "mode": "hybrid",
            "horizon": 6,
            "rewardWeights": [
                { "signal": "toleranceHit", "weight": 2.0 },
                { "signal": "humanInterventionMinutes", "weight": -1.0 }
            ]
        }
    }))
}

async fn metrics(State(state): State<AppState>) -> Response {
    let body = format!(
        "# HELP dd_fabrication_server_requests_total HTTP fabrication requests.\n\
         # TYPE dd_fabrication_server_requests_total counter\n\
         dd_fabrication_server_requests_total {}\n\
         # HELP dd_fabrication_server_instruction_validations_total Instruction validation requests.\n\
         # TYPE dd_fabrication_server_instruction_validations_total counter\n\
         dd_fabrication_server_instruction_validations_total {}\n\
         # HELP dd_fabrication_server_telemetry_learning_total Telemetry learning requests.\n\
         # TYPE dd_fabrication_server_telemetry_learning_total counter\n\
         dd_fabrication_server_telemetry_learning_total {}\n\
         # HELP dd_fabrication_server_generated_instructions_total Generated instruction artifacts.\n\
         # TYPE dd_fabrication_server_generated_instructions_total counter\n\
         dd_fabrication_server_generated_instructions_total {}\n\
         # HELP dd_fabrication_server_findings_total Validation and planning findings.\n\
         # TYPE dd_fabrication_server_findings_total counter\n\
         dd_fabrication_server_findings_total {}\n\
         # HELP dd_fabrication_server_mdp_delegations_total MDP/POMDP optimizer requests published.\n\
         # TYPE dd_fabrication_server_mdp_delegations_total counter\n\
         dd_fabrication_server_mdp_delegations_total {}\n\
         # HELP dd_fabrication_server_errors_total Request or processing errors.\n\
         # TYPE dd_fabrication_server_errors_total counter\n\
         dd_fabrication_server_errors_total {}\n\
         # HELP dd_fabrication_server_nats_messages_total NATS fabrication requests received.\n\
         # TYPE dd_fabrication_server_nats_messages_total counter\n\
         dd_fabrication_server_nats_messages_total {}\n",
        state.metrics.requests_total.load(Ordering::Relaxed),
        state
            .metrics
            .instruction_validations_total
            .load(Ordering::Relaxed),
        state.metrics.telemetry_learning_total.load(Ordering::Relaxed),
        state
            .metrics
            .generated_instructions_total
            .load(Ordering::Relaxed),
        state.metrics.findings_total.load(Ordering::Relaxed),
        state.metrics.mdp_delegations_total.load(Ordering::Relaxed),
        state.metrics.errors_total.load(Ordering::Relaxed),
        state.metrics.nats_messages_total.load(Ordering::Relaxed),
    );
    (
        [(
            axum::http::header::CONTENT_TYPE,
            "text/plain; version=0.0.4",
        )],
        body,
    )
        .into_response()
}

async fn fabricate_http(
    State(state): State<AppState>,
    Json(request): Json<FabricationRequest>,
) -> Response {
    state.metrics.requests_total.fetch_add(1, Ordering::Relaxed);
    match plan_fabrication(request) {
        Ok(response) => {
            state
                .metrics
                .generated_instructions_total
                .fetch_add(response.instructions.len() as u64, Ordering::Relaxed);
            state
                .metrics
                .findings_total
                .fetch_add(response.boundaries.len() as u64, Ordering::Relaxed);
            publish_fabrication_result(&state, &response).await;
            Json(response).into_response()
        }
        Err(error) => {
            state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
            (
                StatusCode::BAD_REQUEST,
                Json(json!({ "ok": false, "error": error })),
            )
                .into_response()
        }
    }
}

async fn validate_instructions_http(
    State(state): State<AppState>,
    Json(request): Json<InstructionValidationRequest>,
) -> Response {
    instruction_validation_response(state, request, false).await
}

async fn improve_instructions_http(
    State(state): State<AppState>,
    Json(mut request): Json<InstructionValidationRequest>,
) -> Response {
    request.improve = Some(true);
    instruction_validation_response(state, request, true).await
}

async fn instruction_validation_response(
    state: AppState,
    request: InstructionValidationRequest,
    force_improve: bool,
) -> Response {
    state
        .metrics
        .instruction_validations_total
        .fetch_add(1, Ordering::Relaxed);
    match validate_instruction_request(request, force_improve) {
        Ok(analysis) => {
            let findings = analysis
                .iter()
                .map(|item| item.findings.len() as u64)
                .sum::<u64>();
            state
                .metrics
                .findings_total
                .fetch_add(findings, Ordering::Relaxed);
            Json(json!({
                "ok": true,
                "generatedAtMs": now_ms(),
                "analysis": analysis,
            }))
            .into_response()
        }
        Err(error) => {
            state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
            (
                StatusCode::BAD_REQUEST,
                Json(json!({ "ok": false, "error": error })),
            )
                .into_response()
        }
    }
}

async fn learn_telemetry_http(
    State(state): State<AppState>,
    Json(request): Json<FabricationTelemetryRequest>,
) -> Response {
    state
        .metrics
        .telemetry_learning_total
        .fetch_add(1, Ordering::Relaxed);
    match learn_from_telemetry(request) {
        Ok(response) => Json(response).into_response(),
        Err(error) => {
            state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
            (
                StatusCode::BAD_REQUEST,
                Json(json!({ "ok": false, "error": error })),
            )
                .into_response()
        }
    }
}

async fn api_docs_html() -> axum::response::Html<&'static str> {
    axum::response::Html(include_str!("../generated/api-docs.html"))
}

async fn api_docs_json() -> impl axum::response::IntoResponse {
    (
        [("content-type", "application/json; charset=utf-8")],
        include_str!("../generated/api-docs.json"),
    )
}

fn plan_fabrication(request: FabricationRequest) -> Result<FabricationResponse, String> {
    validate_request_size(&request)?;
    let request_id = request
        .request_id
        .clone()
        .unwrap_or_else(|| format!("fabrication-{}", now_ms()));
    let plan_id = format!(
        "plan-{}-{}",
        slug(&request.objective.name),
        stable_hash(&request_id)
    );
    let material_class = infer_material_class(
        request.objective.material_class,
        request.objective.material.as_deref(),
    );
    let bounding_box = request.objective.bounding_box_mm.clone();
    let quantity = request.objective.quantity.unwrap_or(1).max(1);
    let mut boundaries = Vec::new();

    let existing_instruction_count = request
        .existing_instructions
        .as_ref()
        .map(|items| items.len())
        .unwrap_or(0);
    if existing_instruction_count > MAX_EXISTING_INSTRUCTIONS {
        return Err(format!(
            "too many existingInstructions entries: {existing_instruction_count} > {MAX_EXISTING_INSTRUCTIONS}"
        ));
    }

    let mut decomposition = decompose_parts(&request, material_class, &mut boundaries);
    if decomposition.is_empty() {
        decomposition.push(PartSpec {
            part_id: "part-1".to_string(),
            name: request.objective.name.clone(),
            material_class,
            bounding_box_mm: bounding_box.clone(),
            process_hint: process_hint(&request.objective, material_class),
            join_strategy: None,
        });
    }

    let mut operations = Vec::new();
    let mut instructions = Vec::new();
    let mut validation = Vec::new();
    let mut assembly = Vec::new();

    if let Some(existing) = request.existing_instructions.as_ref() {
        for (index, existing_instruction) in existing.iter().enumerate() {
            let machine =
                find_machine_for_instruction(&request.available_machines, existing_instruction);
            let analysis = analyze_instruction(existing_instruction, machine, true)?;
            boundaries.extend(analysis.findings.clone());
            let operation_id = format!("validate-{}", index + 1);
            operations.push(FabricationOperation {
                operation_id: operation_id.clone(),
                process: FabricationProcess::ValidateExisting,
                machine_id: existing_instruction
                    .machine_id
                    .clone()
                    .or_else(|| machine.map(|item| item.id.clone())),
                machine_kind: existing_instruction
                    .machine_kind
                    .or_else(|| machine.map(|item| item.kind))
                    .unwrap_or(MachineKind::Unknown),
                part_id: decomposition
                    .first()
                    .map(|part| part.part_id.clone())
                    .unwrap_or_else(|| "part-1".to_string()),
                intent: "Validate supplied machine instructions and identify failure boundaries."
                    .to_string(),
                setup_notes: vec![
                    "Existing instructions are treated as operator-supplied source; improvements are advisory."
                        .to_string(),
                ],
                estimated_risk: risk_from_findings(&analysis.findings),
                instruction_ref: existing_instruction.instruction_id.clone(),
            });
            if let Some(improved_content) = analysis.improved_content.clone() {
                instructions.push(InstructionArtifact {
                    instruction_id: format!("improved-{}", index + 1),
                    operation_id,
                    machine_id: existing_instruction.machine_id.clone(),
                    machine_kind: existing_instruction
                        .machine_kind
                        .or_else(|| machine.map(|item| item.kind))
                        .unwrap_or(MachineKind::Unknown),
                    format: existing_instruction.format.unwrap_or_else(|| {
                        existing_instruction
                            .machine_kind
                            .or_else(|| machine.map(|item| item.kind))
                            .unwrap_or(MachineKind::Unknown)
                            .default_instruction_format()
                    }),
                    content: improved_content,
                    safety_notes: analysis.improvement_summary.clone(),
                    generated: true,
                });
            }
            validation.push(analysis);
        }
    }

    for part in &decomposition {
        build_operations_for_part(
            &request,
            part,
            quantity,
            &mut operations,
            &mut instructions,
            &mut assembly,
            &mut boundaries,
        );
    }

    if decomposition.len() > 1 {
        assembly.push(AssemblyStep {
            step_id: "assembly-fit-1".to_string(),
            description: "Dry-fit split parts, deburr mating edges, then bond or fasten according to load direction."
                .to_string(),
            required_after_operations: operations
                .iter()
                .filter(|op| !matches!(op.process, FabricationProcess::ValidateExisting))
                .map(|op| op.operation_id.clone())
                .collect(),
            intervention: true,
        });
        boundaries.push(finding(
            BoundaryKind::AssemblyRequired,
            BoundarySeverity::Warning,
            "The design is decomposed into multiple manufacturable parts and needs an assembly step.",
            None,
            None,
            None,
            Some("Add alignment pins, witness marks, or a fixture datum to make recombination repeatable."),
        ));
    }

    let design = build_design_package(&request, &plan_id, material_class, &decomposition);
    let expected_human_intervention = boundaries.iter().any(|item| {
        matches!(
            item.kind,
            BoundaryKind::HumanInterventionRequired
                | BoundaryKind::AssemblyRequired
                | BoundaryKind::SetupChangeRequired
                | BoundaryKind::ToolingRequired
        )
    });
    let estimated_risk = risk_from_findings(&boundaries);
    let status = if boundaries.iter().any(|item| {
        matches!(
            item.severity,
            BoundarySeverity::Critical | BoundarySeverity::Error
        )
    }) {
        "needsHumanReview"
    } else if expected_human_intervention {
        "plannedWithIntervention"
    } else {
        "planned"
    }
    .to_string();
    let learning = build_learning_plan(
        &request_id,
        &plan_id,
        request.learning.as_ref(),
        &operations,
        &boundaries,
        expected_human_intervention,
    );
    let process_summary = summarize_processes(&operations);

    Ok(FabricationResponse {
        ok: true,
        request_id,
        plan_id,
        status,
        generated_at_ms: now_ms(),
        design,
        plan: FabricationPlan {
            operations,
            assembly,
            expected_human_intervention,
            estimated_risk,
            process_summary,
        },
        instructions,
        validation,
        boundaries,
        learning,
    })
}

fn validate_request_size(request: &FabricationRequest) -> Result<(), String> {
    for instruction in request.existing_instructions.as_ref().into_iter().flatten() {
        if instruction.content.len() > MAX_INSTRUCTION_BYTES {
            return Err(format!(
                "instruction content is too large: {} > {MAX_INSTRUCTION_BYTES}",
                instruction.content.len()
            ));
        }
    }
    Ok(())
}

fn decompose_parts(
    request: &FabricationRequest,
    material_class: MaterialClass,
    boundaries: &mut Vec<BoundaryFinding>,
) -> Vec<PartSpec> {
    let Some(bounding_box) = request.objective.bounding_box_mm.clone() else {
        return vec![PartSpec {
            part_id: "part-1".to_string(),
            name: request.objective.name.clone(),
            material_class,
            bounding_box_mm: None,
            process_hint: process_hint(&request.objective, material_class),
            join_strategy: None,
        }];
    };

    let best_envelope = request
        .available_machines
        .iter()
        .filter(|machine| {
            machine_accepts_material(
                machine,
                material_class,
                request.objective.material.as_deref(),
            )
        })
        .filter_map(|machine| {
            machine
                .capabilities
                .as_ref()
                .and_then(|capabilities| capabilities.work_envelope_mm.as_ref())
        })
        .max_by(|left, right| {
            left.max_axis()
                .partial_cmp(&right.max_axis())
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .cloned();

    let Some(envelope) = best_envelope else {
        boundaries.push(finding(
            BoundaryKind::NoCapableMachine,
            BoundarySeverity::Error,
            "No available machine advertises both a work envelope and compatible material support.",
            None,
            None,
            None,
            Some("Add machine capabilities or supply an external process route before releasing this job."),
        ));
        return vec![PartSpec {
            part_id: "part-1".to_string(),
            name: request.objective.name.clone(),
            material_class,
            bounding_box_mm: Some(bounding_box),
            process_hint: process_hint(&request.objective, material_class),
            join_strategy: None,
        }];
    };

    if bounding_box.fits_inside(&envelope) {
        return vec![PartSpec {
            part_id: "part-1".to_string(),
            name: request.objective.name.clone(),
            material_class,
            bounding_box_mm: Some(bounding_box),
            process_hint: process_hint(&request.objective, material_class),
            join_strategy: None,
        }];
    }

    let split_count = split_count_for_envelope(&bounding_box, &envelope).clamp(2, 16);
    boundaries.push(finding(
        BoundaryKind::SplitRequired,
        BoundarySeverity::Warning,
        format!(
            "Requested envelope {:.1} x {:.1} x {:.1} mm exceeds the best compatible machine envelope {:.1} x {:.1} x {:.1} mm; splitting into {split_count} parts.",
            bounding_box.x, bounding_box.y, bounding_box.z, envelope.x, envelope.y, envelope.z
        ),
        None,
        None,
        None,
        Some("Prefer splits along low-stress axes and add alignment features for post-processing."),
    ));

    (0..split_count)
        .map(|index| {
            let part_box = if bounding_box.x >= bounding_box.y && bounding_box.x >= bounding_box.z {
                BoundingBoxMm {
                    x: bounding_box.x / split_count as f64,
                    y: bounding_box.y,
                    z: bounding_box.z,
                }
            } else if bounding_box.y >= bounding_box.z {
                BoundingBoxMm {
                    x: bounding_box.x,
                    y: bounding_box.y / split_count as f64,
                    z: bounding_box.z,
                }
            } else {
                BoundingBoxMm {
                    x: bounding_box.x,
                    y: bounding_box.y,
                    z: bounding_box.z / split_count as f64,
                }
            };
            PartSpec {
                part_id: format!("part-{}", index + 1),
                name: format!("{} segment {}", request.objective.name, index + 1),
                material_class,
                bounding_box_mm: Some(part_box),
                process_hint: process_hint(&request.objective, material_class),
                join_strategy: Some("bonded lap joint or pinned mechanical alignment".to_string()),
            }
        })
        .collect()
}

fn split_count_for_envelope(part: &BoundingBoxMm, envelope: &BoundingBoxMm) -> usize {
    let x = if part.x > envelope.x {
        (part.x / envelope.x).ceil() as usize
    } else {
        1
    };
    let y = if part.y > envelope.y {
        (part.y / envelope.y).ceil() as usize
    } else {
        1
    };
    let z = if part.z > envelope.z {
        (part.z / envelope.z).ceil() as usize
    } else {
        1
    };
    x.max(y).max(z)
}

fn build_operations_for_part(
    request: &FabricationRequest,
    part: &PartSpec,
    quantity: u32,
    operations: &mut Vec<FabricationOperation>,
    instructions: &mut Vec<InstructionArtifact>,
    assembly: &mut Vec<AssemblyStep>,
    boundaries: &mut Vec<BoundaryFinding>,
) {
    let primary_machine = select_primary_machine(request, part, boundaries);
    let finishing_machine = select_finishing_machine(request, part, primary_machine);
    let tolerance = request
        .objective
        .tolerance_mm
        .unwrap_or(default_tolerance(part.process_hint));
    let primary_process = process_for_machine(
        primary_machine.map(|machine| machine.kind),
        part.process_hint,
    );
    let primary_operation_id = format!(
        "op-{}-{}",
        operations.len() + 1,
        process_slug(primary_process)
    );
    let primary_machine_kind = primary_machine
        .map(|machine| machine.kind)
        .unwrap_or(machine_kind_for_process(primary_process));
    let mut setup_notes = setup_notes_for(
        &request.objective,
        part,
        primary_machine,
        primary_process,
        quantity,
    );
    let mut risk = base_risk(primary_process);

    if machine_needs_human_tooling(primary_machine, primary_process) {
        risk += 0.12;
        boundaries.push(finding(
            BoundaryKind::ToolingRequired,
            BoundarySeverity::Warning,
            "The selected operation requires operator confirmation of tool, fixture, work offset, or resin/filament setup.",
            primary_machine.map(|machine| machine.id.clone()),
            Some(primary_operation_id.clone()),
            None,
            Some("Gate this operation behind an operator checklist before starting unattended execution."),
        ));
    }

    if let Some(part_box) = part.bounding_box_mm.as_ref() {
        if let Some(machine) = primary_machine {
            check_machine_fit(machine, part_box, boundaries, &primary_operation_id);
        }
    }

    add_geometry_boundaries(
        request,
        part,
        primary_machine,
        primary_process,
        &primary_operation_id,
        boundaries,
    );

    let instruction_id = format!("inst-{}", instructions.len() + 1);
    let instruction = generate_instruction(
        request,
        part,
        &primary_operation_id,
        &instruction_id,
        primary_machine,
        primary_machine_kind,
        primary_process,
    );
    setup_notes.extend(instruction.safety_notes.clone());
    operations.push(FabricationOperation {
        operation_id: primary_operation_id.clone(),
        process: primary_process,
        machine_id: primary_machine.map(|machine| machine.id.clone()),
        machine_kind: primary_machine_kind,
        part_id: part.part_id.clone(),
        intent: intent_for_process(primary_process, tolerance),
        setup_notes,
        estimated_risk: risk.min(1.0),
        instruction_ref: Some(instruction_id),
    });
    instructions.push(instruction);

    if let Some(finisher) = finishing_machine {
        let finisher_tolerance = finisher
            .capabilities
            .as_ref()
            .and_then(|capabilities| capabilities.min_tolerance_mm)
            .unwrap_or(0.05);
        if tolerance < finisher_tolerance * 3.0 || primary_machine_kind.is_printer() {
            let operation_id = format!("op-{}-finish", operations.len() + 1);
            let instruction_id = format!("inst-{}", instructions.len() + 1);
            boundaries.push(finding(
                BoundaryKind::ToleranceTooTight,
                BoundarySeverity::Info,
                format!(
                    "Requested tolerance {tolerance:.3} mm is better served by a finishing pass after the primary process."
                ),
                Some(finisher.id.clone()),
                Some(operation_id.clone()),
                None,
                Some("Machine only datum faces and functional holes; leave cosmetic surfaces as-printed unless specified."),
            ));
            let instruction = generate_instruction(
                request,
                part,
                &operation_id,
                &instruction_id,
                Some(finisher),
                finisher.kind,
                FabricationProcess::MillFinishing,
            );
            operations.push(FabricationOperation {
                operation_id: operation_id.clone(),
                process: FabricationProcess::MillFinishing,
                machine_id: Some(finisher.id.clone()),
                machine_kind: finisher.kind,
                part_id: part.part_id.clone(),
                intent: format!("Finish critical datums to requested tolerance {tolerance:.3} mm."),
                setup_notes: vec![
                    "Probe or indicate printed/rough stock before removing finish allowance."
                        .to_string(),
                    "Do not assume the as-printed coordinate frame is square to the machine."
                        .to_string(),
                ],
                estimated_risk: 0.36,
                instruction_ref: Some(instruction_id),
            });
            instructions.push(instruction);
            assembly.push(AssemblyStep {
                step_id: format!("inspect-{}", part.part_id),
                description: format!(
                    "Inspect {} after finishing; feed measured deviations back through /learn/telemetry.",
                    part.name
                ),
                required_after_operations: vec![operation_id],
                intervention: true,
            });
        }
    }
}

fn select_primary_machine<'a>(
    request: &'a FabricationRequest,
    part: &PartSpec,
    boundaries: &mut Vec<BoundaryFinding>,
) -> Option<&'a Machine> {
    let material = request.objective.material.as_deref();
    let preferred = match part.process_hint {
        FabricationProcess::Turn => vec![MachineKind::Lathe, MachineKind::VerticalMill],
        FabricationProcess::MillRoughing | FabricationProcess::MillFinishing => vec![
            MachineKind::VerticalMill,
            MachineKind::HorizontalMill,
            MachineKind::CncRouter,
        ],
        FabricationProcess::ResinPrint => {
            vec![
                MachineKind::SlaPrinter,
                MachineKind::DlpPrinter,
                MachineKind::FdmPrinter,
            ]
        }
        FabricationProcess::PowderPrint => vec![MachineKind::SlsPrinter, MachineKind::BinderJet],
        FabricationProcess::Route => vec![MachineKind::CncRouter, MachineKind::VerticalMill],
        FabricationProcess::Cut => vec![MachineKind::LaserCutter, MachineKind::Waterjet],
        _ => vec![
            MachineKind::FdmPrinter,
            MachineKind::VerticalMill,
            MachineKind::CncRouter,
            MachineKind::Lathe,
        ],
    };

    for kind in preferred {
        if let Some(machine) = request.available_machines.iter().find(|machine| {
            machine.kind == kind && machine_accepts_material(machine, part.material_class, material)
        }) {
            return Some(machine);
        }
    }

    let fallback = request
        .available_machines
        .iter()
        .find(|machine| machine_accepts_material(machine, part.material_class, material));
    if fallback.is_none() {
        boundaries.push(finding(
            BoundaryKind::NoCapableMachine,
            BoundarySeverity::Error,
            format!(
                "No compatible machine found for {} ({:?}).",
                part.name, part.material_class
            ),
            None,
            None,
            None,
            Some("Add a capable machine or change material/process constraints."),
        ));
    }
    fallback
}

fn select_finishing_machine<'a>(
    request: &'a FabricationRequest,
    part: &PartSpec,
    primary_machine: Option<&Machine>,
) -> Option<&'a Machine> {
    request.available_machines.iter().find(|machine| {
        machine.kind.is_mill_like()
            && primary_machine.map(|primary| primary.id.as_str()) != Some(machine.id.as_str())
            && machine_accepts_material(
                machine,
                part.material_class,
                request.objective.material.as_deref(),
            )
    })
}

fn machine_accepts_material(
    machine: &Machine,
    material_class: MaterialClass,
    material: Option<&str>,
) -> bool {
    let Some(capabilities) = machine.capabilities.as_ref() else {
        return true;
    };
    let Some(materials) = capabilities.materials.as_ref() else {
        return true;
    };
    let normalized_material = material.map(normalize_token);
    materials.iter().any(|candidate| {
        let candidate = normalize_token(candidate);
        normalized_material.as_deref() == Some(candidate.as_str())
            || material_class_token(material_class) == candidate
            || (material_class == MaterialClass::Plastic
                && matches!(candidate.as_str(), "pla" | "petg" | "abs" | "nylon"))
            || (material_class == MaterialClass::Metal
                && matches!(
                    candidate.as_str(),
                    "aluminum" | "steel" | "brass" | "titanium"
                ))
    })
}

fn check_machine_fit(
    machine: &Machine,
    part_box: &BoundingBoxMm,
    boundaries: &mut Vec<BoundaryFinding>,
    operation_id: &str,
) {
    let Some(envelope) = machine
        .capabilities
        .as_ref()
        .and_then(|capabilities| capabilities.work_envelope_mm.as_ref())
    else {
        return;
    };
    if !part_box.fits_inside(envelope) {
        boundaries.push(finding(
            BoundaryKind::WorkEnvelopeExceeded,
            BoundarySeverity::Error,
            format!(
                "Part envelope {:.1} x {:.1} x {:.1} mm exceeds {} work envelope {:.1} x {:.1} x {:.1} mm.",
                part_box.x, part_box.y, part_box.z, machine.id, envelope.x, envelope.y, envelope.z
            ),
            Some(machine.id.clone()),
            Some(operation_id.to_string()),
            None,
            Some("Split the part, rotate stock only if fixtures and axes permit, or choose a larger machine."),
        ));
    }
}

fn add_geometry_boundaries(
    request: &FabricationRequest,
    part: &PartSpec,
    machine: Option<&Machine>,
    process: FabricationProcess,
    operation_id: &str,
    boundaries: &mut Vec<BoundaryFinding>,
) {
    if matches!(process, FabricationProcess::FdmPrint) {
        if request.objective.overhang_degrees.unwrap_or(0.0) > 45.0 {
            boundaries.push(finding(
                BoundaryKind::HumanInterventionRequired,
                BoundarySeverity::Warning,
                "FDM overhang exceeds 45 degrees; supports or orientation changes are required.",
                machine.map(|item| item.id.clone()),
                Some(operation_id.to_string()),
                None,
                Some("Generate support material, rotate the part, or split the model to avoid trapped supports."),
            ));
        }
        if let (Some(wall), Some(nozzle)) = (
            request.objective.min_wall_mm,
            machine
                .and_then(|item| item.capabilities.as_ref())
                .and_then(|capabilities| capabilities.nozzle_diameter_mm),
        ) {
            if wall < nozzle * 2.0 {
                boundaries.push(finding(
                    BoundaryKind::ToolingRequired,
                    BoundarySeverity::Warning,
                    format!(
                        "Minimum wall {wall:.2} mm is less than two nozzle widths ({:.2} mm).",
                        nozzle * 2.0
                    ),
                    machine.map(|item| item.id.clone()),
                    Some(operation_id.to_string()),
                    None,
                    Some("Use a smaller nozzle, thicken the wall, or machine that feature after printing."),
                ));
            }
        }
    }

    if matches!(process, FabricationProcess::Turn)
        && !request.objective.rotational_symmetry.unwrap_or(false)
    {
        boundaries.push(finding(
            BoundaryKind::SetupChangeRequired,
            BoundarySeverity::Info,
            "Lathe operation is useful for round datums, but the objective is not fully rotationally symmetric.",
            machine.map(|item| item.id.clone()),
            Some(operation_id.to_string()),
            None,
            Some("Use the lathe only for cylindrical features, then mill non-axisymmetric faces."),
        ));
    }

    if part.join_strategy.is_some() {
        boundaries.push(finding(
            BoundaryKind::AssemblyRequired,
            BoundarySeverity::Info,
            format!("{} carries a join strategy and must preserve alignment features.", part.name),
            machine.map(|item| item.id.clone()),
            Some(operation_id.to_string()),
            None,
            Some("Add dogbone-free pockets or pin bores after roughing so assembly datums survive finishing."),
        ));
    }
}

fn generate_instruction(
    request: &FabricationRequest,
    part: &PartSpec,
    operation_id: &str,
    instruction_id: &str,
    machine: Option<&Machine>,
    machine_kind: MachineKind,
    process: FabricationProcess,
) -> InstructionArtifact {
    let format = machine_kind.default_instruction_format();
    let material = request
        .objective
        .material
        .as_deref()
        .unwrap_or(material_class_token(part.material_class));
    let machine_id = machine.map(|item| item.id.clone());
    let safety_notes = safety_notes_for(process, machine);
    let content = match process {
        FabricationProcess::FdmPrint => generate_fdm_gcode(request, part, machine),
        FabricationProcess::ResinPrint | FabricationProcess::PowderPrint => {
            generate_printer_job(request, part, machine, process)
        }
        FabricationProcess::MillRoughing | FabricationProcess::MillFinishing => {
            generate_mill_gcode(request, part, machine, process)
        }
        FabricationProcess::Turn => generate_lathe_gcode(request, part, machine),
        FabricationProcess::Route | FabricationProcess::Cut => {
            generate_router_or_cut_gcode(request, part, machine, process)
        }
        _ => format!(
            "# dd-fabrication-server setup sheet\noperation: {operation_id}\npart: {}\nmaterial: {material}\nmachine: {}\nnotes:\n- Review generated plan before running hardware.\n",
            part.name,
            machine_id.as_deref().unwrap_or("unassigned")
        ),
    };
    InstructionArtifact {
        instruction_id: instruction_id.to_string(),
        operation_id: operation_id.to_string(),
        machine_id,
        machine_kind,
        format,
        content,
        safety_notes,
        generated: true,
    }
}

fn generate_fdm_gcode(
    request: &FabricationRequest,
    part: &PartSpec,
    machine: Option<&Machine>,
) -> String {
    let material = request.objective.material.as_deref().unwrap_or("PLA");
    let (extruder_temp, bed_temp) = print_temps(material);
    let box_text = part_box_text(part.bounding_box_mm.as_ref());
    let layer_stack = fdm_primitive_toolpath(part);
    format!(
        "; dd-fabrication-server fabrication.v1\n\
         ; part: {}\n\
         ; process: FDM print\n\
         ; machine: {}\n\
         ; stock/material: {material}\n\
         ; envelope: {box_text}\n\
         ; CAM note: slice the generated design artifact with verified profile before production.\n\
         G21 ; millimeters\n\
         G90 ; absolute positioning\n\
         M140 S{bed_temp:.0} ; set bed temperature\n\
         M104 S{extruder_temp:.0} ; set nozzle temperature\n\
         G28 ; home axes\n\
         M190 S{bed_temp:.0} ; wait for bed\n\
         M109 S{extruder_temp:.0} ; wait for nozzle\n\
         G92 E0\n\
         G1 Z0.28 F900\n\
         G1 X8 Y8 F3000\n\
         G1 X120 E8 F900 ; purge line\n\
         {layer_stack}\
         M104 S0\n\
         M140 S0\n\
         G1 X0 Y200 F3000\n\
         M84\n",
        part.name,
        machine.map(|item| item.id.as_str()).unwrap_or("unassigned")
    )
}

fn generate_printer_job(
    request: &FabricationRequest,
    part: &PartSpec,
    machine: Option<&Machine>,
    process: FabricationProcess,
) -> String {
    format!(
        "# dd-fabrication-server fabrication.v1 printer job\n\
         part: {}\n\
         process: {:?}\n\
         machine: {}\n\
         material: {}\n\
         boundingBoxMm: {}\n\
         instructions:\n\
         - Generate support-aware orientation from design artifact.\n\
         - Validate resin/powder profile against exposure, sintering, and shrinkage compensation.\n\
         - Keep wash/cure/depowdering as explicit human-intervention boundaries.\n",
        part.name,
        process,
        machine.map(|item| item.id.as_str()).unwrap_or("unassigned"),
        request
            .objective
            .material
            .as_deref()
            .unwrap_or("unspecified"),
        part_box_text(part.bounding_box_mm.as_ref())
    )
}

fn generate_mill_gcode(
    request: &FabricationRequest,
    part: &PartSpec,
    machine: Option<&Machine>,
    process: FabricationProcess,
) -> String {
    let feed = machine
        .and_then(|item| item.capabilities.as_ref())
        .and_then(|capabilities| capabilities.max_feed_mm_min)
        .unwrap_or(800.0)
        .min(600.0);
    let spindle = machine
        .and_then(|item| item.capabilities.as_ref())
        .and_then(|capabilities| capabilities.max_spindle_rpm)
        .unwrap_or(6000.0)
        .min(4500.0);
    let box_text = part_box_text(part.bounding_box_mm.as_ref());
    let toolpath = mill_primitive_toolpath(part, process, feed);
    format!(
        "(dd-fabrication-server fabrication.v1)\n\
         (part: {})\n\
         (process: {:?})\n\
         (machine: {})\n\
         (material: {})\n\
         (envelope: {box_text})\n\
         G21 G17 G40 G49 G54 G80 G90\n\
         T1 M6 (verify cutter diameter and stickout)\n\
         S{spindle:.0} M3\n\
         G0 X0 Y0 Z25\n\
         M8\n\
         G1 Z2 F200\n\
         {toolpath}\
         G0 Z25\n\
         M9\n\
         M5\n\
         G53 G0 Z0\n\
         M30\n",
        part.name,
        process,
        machine.map(|item| item.id.as_str()).unwrap_or("unassigned"),
        request
            .objective
            .material
            .as_deref()
            .unwrap_or("unspecified"),
    )
}

fn generate_lathe_gcode(
    request: &FabricationRequest,
    part: &PartSpec,
    machine: Option<&Machine>,
) -> String {
    let spindle = machine
        .and_then(|item| item.capabilities.as_ref())
        .and_then(|capabilities| capabilities.max_spindle_rpm)
        .unwrap_or(2500.0)
        .min(1800.0);
    let toolpath = lathe_primitive_toolpath(part);
    format!(
        "(dd-fabrication-server fabrication.v1)\n\
         (part: {})\n\
         (process: turning)\n\
         (machine: {})\n\
         (material: {})\n\
         G21 G18 G40 G80 G90\n\
         G50 S{spindle:.0}\n\
         T0101 (verify insert, stickout, and workholding)\n\
         G97 S{spindle:.0} M3\n\
         G0 X50 Z5\n\
         {toolpath}\
         G0 X55 Z10\n\
         M5\n\
         M30\n",
        part.name,
        machine.map(|item| item.id.as_str()).unwrap_or("unassigned"),
        request
            .objective
            .material
            .as_deref()
            .unwrap_or("unspecified")
    )
}

fn generate_router_or_cut_gcode(
    request: &FabricationRequest,
    part: &PartSpec,
    machine: Option<&Machine>,
    process: FabricationProcess,
) -> String {
    let toolpath = router_or_cut_primitive_toolpath(part, process);
    format!(
        "(dd-fabrication-server fabrication.v1)\n\
         (part: {})\n\
         (process: {:?})\n\
         (machine: {})\n\
         (material: {})\n\
         G21 G90\n\
         G28\n\
         G0 X0 Y0 Z10\n\
         {toolpath}\
         G0 Z10\n\
         M30\n",
        part.name,
        process,
        machine.map(|item| item.id.as_str()).unwrap_or("unassigned"),
        request
            .objective
            .material
            .as_deref()
            .unwrap_or("unspecified")
    )
}

fn fdm_primitive_toolpath(part: &PartSpec) -> String {
    let bbox = bounded_bbox(part);
    let origin_x = 8.0;
    let origin_y = 8.0;
    let width = bbox.x.clamp(5.0, 160.0);
    let depth = bbox.y.clamp(5.0, 160.0);
    let height = bbox.z.clamp(0.6, 12.0);
    let layer_height = 0.28;
    let layers = representative_depths(height, layer_height, 6);
    let mut extrusion = 8.0;
    let mut output = String::new();
    output.push_str("; Primitive FDM path: perimeter plus sparse diagonal infill for the normalized part envelope.\n");
    output.push_str("; Replace with controller-specific slicer output for complex meshes or production hardware.\n");
    for (index, z) in layers.iter().enumerate() {
        let inset = (index as f64 * 0.16).min(width.min(depth) * 0.12);
        let x0 = origin_x + inset;
        let y0 = origin_y + inset;
        let x1 = origin_x + width - inset;
        let y1 = origin_y + depth - inset;
        output.push_str(&format!("; layer {} z={z:.3}\n", index + 1));
        output.push_str(&format!("G1 Z{z:.3} F900\n"));
        output.push_str(&format!("G1 X{x0:.3} Y{y0:.3} F3000\n"));
        for (x, y) in [(x1, y0), (x1, y1), (x0, y1), (x0, y0)] {
            extrusion += ((x - x0).abs() + (y - y0).abs()).max(1.0) * 0.035;
            output.push_str(&format!("G1 X{x:.3} Y{y:.3} E{extrusion:.4} F1200\n"));
        }
        let infill_y = y0 + (y1 - y0) * 0.5;
        extrusion += width.max(1.0) * 0.028;
        output.push_str(&format!(
            "G1 X{x1:.3} Y{infill_y:.3} E{extrusion:.4} F1500 ; envelope infill pass\n"
        ));
        extrusion += width.max(1.0) * 0.028;
        output.push_str(&format!(
            "G1 X{x0:.3} Y{infill_y:.3} E{extrusion:.4} F1500 ; return infill pass\n"
        ));
    }
    output
}

fn mill_primitive_toolpath(part: &PartSpec, process: FabricationProcess, feed: f64) -> String {
    let bbox = bounded_bbox(part);
    let width = bbox.x.clamp(5.0, 180.0);
    let depth = bbox.y.clamp(5.0, 180.0);
    let cut_depth = bbox.z.clamp(0.5, 8.0);
    let step = if process == FabricationProcess::MillFinishing {
        cut_depth
    } else {
        2.0
    };
    let depths = representative_depths(cut_depth, step, 5);
    let finish_allowance = if process == FabricationProcess::MillFinishing {
        0.0
    } else {
        0.35
    };
    let mut output = String::new();
    output.push_str(
        "(Primitive mill path: rectangular pocket/profile based on normalized part envelope)\n",
    );
    output.push_str("(Simulate and repost for cutter compensation, entry strategy, and fixture-specific clearance)\n");
    for (index, depth_z) in depths.iter().enumerate() {
        let x1 = (width - finish_allowance).max(1.0);
        let y1 = (depth - finish_allowance).max(1.0);
        output.push_str(&format!(
            "(rough/finish pass {} depth -{depth_z:.3})\n",
            index + 1
        ));
        output.push_str("G0 X0 Y0\n");
        output.push_str(&format!("G1 Z-{depth_z:.3} F180\n"));
        output.push_str(&format!("G1 X{x1:.3} Y0 F{feed:.0}\n"));
        output.push_str(&format!("G1 X{x1:.3} Y{y1:.3}\n"));
        output.push_str(&format!("G1 X0 Y{y1:.3}\n"));
        output.push_str("G1 X0 Y0\n");
        if process != FabricationProcess::MillFinishing {
            let pocket_x = x1 * 0.5;
            output.push_str(&format!("G1 X{pocket_x:.3} Y0\n"));
            output.push_str(&format!("G1 X{pocket_x:.3} Y{y1:.3} ; clearing raster\n"));
        }
        output.push_str("G0 Z5\n");
    }
    if process != FabricationProcess::MillFinishing {
        output.push_str("(finish allowance left for a downstream finishing operation)\n");
    } else {
        output.push_str("(finish contour complete for datum envelope)\n");
    }
    output
}

fn lathe_primitive_toolpath(part: &PartSpec) -> String {
    let bbox = bounded_bbox(part);
    let target_diameter = bbox.x.min(bbox.y).clamp(2.0, 160.0);
    let stock_diameter = (target_diameter + 4.0).min(180.0);
    let length = bbox.z.clamp(5.0, 250.0);
    let rough_step = ((stock_diameter - target_diameter) / 3.0).max(0.5);
    let mut output = String::new();
    output.push_str("(Primitive lathe path: rough OD passes, face, and finish diameter)\n");
    output.push_str(
        "(Controller post must verify diameter/radius mode, tool nose comp, and workholding)\n",
    );
    let mut diameter = stock_diameter;
    let mut pass_index = 1;
    while diameter - rough_step > target_diameter {
        diameter -= rough_step;
        output.push_str(&format!("(rough OD pass {pass_index} X{diameter:.3})\n"));
        output.push_str(&format!("G0 X{:.3} Z2\n", diameter + 1.0));
        output.push_str(&format!("G1 X{diameter:.3} F0.12\n"));
        output.push_str(&format!("G1 Z-{length:.3} F0.18\n"));
        output.push_str(&format!("G0 X{:.3}\n", stock_diameter + 2.0));
        output.push_str("G0 Z2\n");
        pass_index += 1;
    }
    output.push_str("(face and finish pass)\n");
    output.push_str(&format!("G0 X{:.3} Z0.5\n", stock_diameter + 2.0));
    output.push_str("G1 Z0 F0.10\n");
    output.push_str("G1 X0 F0.08\n");
    output.push_str(&format!("G0 X{:.3} Z1\n", target_diameter + 1.0));
    output.push_str(&format!("G1 X{target_diameter:.3} F0.08\n"));
    output.push_str(&format!("G1 Z-{length:.3} F0.12\n"));
    output
}

fn router_or_cut_primitive_toolpath(part: &PartSpec, process: FabricationProcess) -> String {
    let bbox = bounded_bbox(part);
    let width = bbox.x.clamp(5.0, 240.0);
    let depth = bbox.y.clamp(5.0, 240.0);
    let cut_depth = bbox.z.clamp(0.5, 9.0);
    let feed = if process == FabricationProcess::Cut {
        420.0
    } else {
        650.0
    };
    let depths = representative_depths(cut_depth, 2.5, 5);
    let mut output = String::new();
    output.push_str("(Primitive profile path: nested rectangle with tab-lift boundaries)\n");
    output.push_str(
        "(Review kerf, cutter diameter, tabs, clamps, and sheet hold-down before production)\n",
    );
    for (index, depth_z) in depths.iter().enumerate() {
        output.push_str(&format!(
            "(profile pass {} depth -{depth_z:.3})\n",
            index + 1
        ));
        output.push_str("G0 X0 Y0 Z5\n");
        output.push_str(&format!("G1 Z-{depth_z:.3} F120\n"));
        output.push_str(&format!("G1 X{:.3} Y0 F{feed:.0}\n", width * 0.45));
        output.push_str("G0 Z1.5 (tab lift)\n");
        output.push_str(&format!("G0 X{:.3} Y0\n", width * 0.55));
        output.push_str(&format!("G1 Z-{depth_z:.3} F120\n"));
        output.push_str(&format!("G1 X{width:.3} Y0 F{feed:.0}\n"));
        output.push_str(&format!("G1 X{width:.3} Y{depth:.3}\n"));
        output.push_str(&format!("G1 X0 Y{depth:.3}\n"));
        output.push_str("G1 X0 Y0\n");
    }
    output
}

fn bounded_bbox(part: &PartSpec) -> BoundingBoxMm {
    part.bounding_box_mm.clone().unwrap_or(BoundingBoxMm {
        x: 25.0,
        y: 25.0,
        z: 10.0,
    })
}

fn representative_depths(total: f64, step: f64, max_steps: usize) -> Vec<f64> {
    let total = total.max(0.001);
    let step = step.max(0.001);
    let mut values = Vec::new();
    let mut current = step.min(total);
    while current < total && values.len() + 1 < max_steps {
        values.push(current);
        current += step;
    }
    if values
        .last()
        .map(|value| (total - *value).abs() > 0.0001)
        .unwrap_or(true)
    {
        values.push(total);
    }
    values
}

fn analyze_instruction(
    instruction: &ExistingInstruction,
    machine: Option<&Machine>,
    improve: bool,
) -> Result<InstructionAnalysis, String> {
    if instruction.content.len() > MAX_INSTRUCTION_BYTES {
        return Err(format!(
            "instruction content is too large: {} > {MAX_INSTRUCTION_BYTES}",
            instruction.content.len()
        ));
    }
    let format = instruction.format.unwrap_or_else(|| {
        instruction
            .machine_kind
            .unwrap_or(MachineKind::Unknown)
            .default_instruction_format()
    });
    let instruction_id = instruction
        .instruction_id
        .clone()
        .unwrap_or_else(|| format!("instruction-{}", stable_hash(&instruction.content)));
    if !format.is_gcode_like() {
        let findings = vec![finding(
            BoundaryKind::UnknownInstruction,
            BoundarySeverity::Info,
            "Instruction format is not G-code-like; deep controller validation is advisory only.",
            instruction
                .machine_id
                .clone()
                .or_else(|| machine.map(|item| item.id.clone())),
            None,
            None,
            Some("Route this payload to a controller-specific parser before unattended execution."),
        )];
        return Ok(InstructionAnalysis {
            instruction_id,
            format,
            line_count: instruction.content.lines().count(),
            estimated_bounds_mm: None,
            commands: Vec::new(),
            findings,
            improvement_summary: vec![
                "Non-G-code payload retained unchanged; add a controller-specific post-processor."
                    .to_string(),
            ],
            improved_content: improve.then(|| instruction.content.clone()),
        });
    }

    let mut state = InstructionState::default();
    let mut findings = Vec::new();
    let mut commands = BTreeSet::new();
    for (line_index, raw_line) in instruction.content.lines().enumerate() {
        let line_no = line_index + 1;
        let line = strip_instruction_comment(raw_line);
        if line.trim().is_empty() {
            continue;
        }
        let tokens = parse_words(&line);
        if tokens.is_empty() {
            continue;
        }
        let command = tokens
            .iter()
            .find(|(letter, _)| matches!(*letter, 'G' | 'M' | 'T'))
            .map(|(letter, value)| command_label(*letter, *value));
        if let Some(command) = command.as_ref() {
            commands.insert(command.clone());
            inspect_command(
                command,
                &tokens,
                line_no,
                instruction,
                machine,
                &mut state,
                &mut findings,
            );
        }
        update_motion_bounds(
            &tokens,
            line_no,
            instruction,
            machine,
            &mut state,
            &mut findings,
        );
    }

    if !state.seen_homing {
        findings.push(finding(
            BoundaryKind::MissingHoming,
            BoundarySeverity::Warning,
            "No homing command was found before the job; machine zero may be stale.",
            instruction
                .machine_id
                .clone()
                .or_else(|| machine.map(|item| item.id.clone())),
            None,
            None,
            Some("Insert G28 or an equivalent controller-specific homing/probing sequence."),
        ));
    }
    if state.absolute_mode.is_none() {
        findings.push(finding(
            BoundaryKind::MissingAbsoluteMode,
            BoundarySeverity::Info,
            "The program does not declare absolute or relative positioning.",
            instruction
                .machine_id
                .clone()
                .or_else(|| machine.map(|item| item.id.clone())),
            None,
            None,
            Some("Insert G90 or G91 explicitly so controller state cannot leak across jobs."),
        ));
    }

    let improvement_summary = improvement_summary(&findings);
    let improved_content = if improve {
        Some(improve_gcode(&instruction.content, &state, &findings))
    } else {
        None
    };

    Ok(InstructionAnalysis {
        instruction_id,
        format,
        line_count: instruction.content.lines().count(),
        estimated_bounds_mm: state.bounds.non_empty().then_some(state.bounds),
        commands: commands.into_iter().collect(),
        findings,
        improvement_summary,
        improved_content,
    })
}

fn validate_instruction_request(
    request: InstructionValidationRequest,
    force_improve: bool,
) -> Result<Vec<InstructionAnalysis>, String> {
    if request.instructions.len() > MAX_EXISTING_INSTRUCTIONS {
        return Err(format!(
            "too many instructions: {} > {MAX_EXISTING_INSTRUCTIONS}",
            request.instructions.len()
        ));
    }
    let machines = request.machines.unwrap_or_default();
    let improve = force_improve || request.improve.unwrap_or(false);
    request
        .instructions
        .iter()
        .map(|instruction| {
            let machine = find_machine_for_instruction(&machines, instruction);
            analyze_instruction(instruction, machine, improve)
        })
        .collect()
}

fn inspect_command(
    command: &str,
    tokens: &BTreeMap<char, f64>,
    line_no: usize,
    instruction: &ExistingInstruction,
    machine: Option<&Machine>,
    state: &mut InstructionState,
    findings: &mut Vec<BoundaryFinding>,
) {
    match command {
        "G20" => {
            state.units_mm = false;
            findings.push(finding(
                BoundaryKind::SetupChangeRequired,
                BoundarySeverity::Warning,
                "Program switches to inch units; envelope validation assumes millimeters after conversion is reviewed.",
                machine_id(instruction, machine),
                None,
                Some(line_no),
                Some("Prefer G21 millimeter programs in this cluster unless the post-processor is inch-verified."),
            ));
        }
        "G21" => state.units_mm = true,
        "G28" | "G30" => state.seen_homing = true,
        "G90" => state.absolute_mode = Some(true),
        "G91" => state.absolute_mode = Some(false),
        "M0" | "M1" | "M6" | "M600" => findings.push(finding(
            BoundaryKind::HumanInterventionRequired,
            BoundarySeverity::Warning,
            format!("{command} requires a pause, stop, tool change, or material change."),
            machine_id(instruction, machine),
            None,
            Some(line_no),
            Some("Split the job at this boundary or require a human-ready checkpoint."),
        )),
        "M104" | "M109" => {
            if let Some(temp) = tokens.get(&'S') {
                if let Some(limit) = machine
                    .and_then(|item| item.capabilities.as_ref())
                    .and_then(|capabilities| capabilities.max_extruder_temp_c)
                {
                    if *temp > limit {
                        findings.push(finding(
                            BoundaryKind::ThermalLimit,
                            BoundarySeverity::Error,
                            format!("Extruder temperature {temp:.1} C exceeds configured limit {limit:.1} C."),
                            machine_id(instruction, machine),
                            None,
                            Some(line_no),
                            Some("Choose a compatible material profile or a hotend rated for this temperature."),
                        ));
                    }
                }
            }
        }
        "M140" | "M190" => {
            if let Some(temp) = tokens.get(&'S') {
                if let Some(limit) = machine
                    .and_then(|item| item.capabilities.as_ref())
                    .and_then(|capabilities| capabilities.max_bed_temp_c)
                {
                    if *temp > limit {
                        findings.push(finding(
                            BoundaryKind::ThermalLimit,
                            BoundarySeverity::Error,
                            format!("Bed temperature {temp:.1} C exceeds configured limit {limit:.1} C."),
                            machine_id(instruction, machine),
                            None,
                            Some(line_no),
                            Some("Lower the bed profile or use a machine with a hotter bed."),
                        ));
                    }
                }
            }
        }
        "M3" | "M4" => {
            if let Some(spindle) = tokens.get(&'S') {
                if let Some(limit) = machine
                    .and_then(|item| item.capabilities.as_ref())
                    .and_then(|capabilities| capabilities.max_spindle_rpm)
                {
                    if *spindle > limit {
                        findings.push(finding(
                            BoundaryKind::SpindleLimit,
                            BoundarySeverity::Error,
                            format!("Spindle speed {spindle:.1} rpm exceeds configured limit {limit:.1} rpm."),
                            machine_id(instruction, machine),
                            None,
                            Some(line_no),
                            Some("Regenerate feeds/speeds for the actual spindle envelope."),
                        ));
                    }
                }
            }
        }
        _ => {
            if command.starts_with('G') || command.starts_with('M') {
                let known = matches!(
                    command,
                    "G0" | "G00"
                        | "G1"
                        | "G01"
                        | "G2"
                        | "G02"
                        | "G3"
                        | "G03"
                        | "G4"
                        | "G17"
                        | "G18"
                        | "G19"
                        | "G20"
                        | "G21"
                        | "G28"
                        | "G30"
                        | "G40"
                        | "G49"
                        | "G53"
                        | "G54"
                        | "G80"
                        | "G90"
                        | "G91"
                        | "G92"
                        | "M3"
                        | "M4"
                        | "M5"
                        | "M8"
                        | "M9"
                        | "M30"
                        | "M82"
                        | "M83"
                        | "M84"
                        | "M104"
                        | "M109"
                        | "M140"
                        | "M190"
                );
                if !known {
                    findings.push(finding(
                        BoundaryKind::UnsupportedCommand,
                        BoundarySeverity::Info,
                        format!("{command} is controller-specific or unsupported by the generic validator."),
                        machine_id(instruction, machine),
                        None,
                        Some(line_no),
                        Some("Check this command against the target controller before unattended execution."),
                    ));
                }
            }
        }
    }
}

fn update_motion_bounds(
    tokens: &BTreeMap<char, f64>,
    line_no: usize,
    instruction: &ExistingInstruction,
    machine: Option<&Machine>,
    state: &mut InstructionState,
    findings: &mut Vec<BoundaryFinding>,
) {
    let motion = tokens
        .get(&'G')
        .map(|value| matches!(value.round() as i64, 0 | 1 | 2 | 3))
        .unwrap_or(false);
    if !motion {
        return;
    }
    if !state.seen_homing {
        state.moved_before_homing = true;
    }
    if let Some(feed) = tokens.get(&'F') {
        if let Some(limit) = machine
            .and_then(|item| item.capabilities.as_ref())
            .and_then(|capabilities| capabilities.max_feed_mm_min)
        {
            if *feed > limit {
                findings.push(finding(
                    BoundaryKind::FeedRateLimit,
                    BoundarySeverity::Warning,
                    format!(
                        "Feed rate {feed:.1} mm/min exceeds configured limit {limit:.1} mm/min."
                    ),
                    machine_id(instruction, machine),
                    None,
                    Some(line_no),
                    Some("Regenerate feed moves using target-machine feed limits."),
                ));
            }
        }
    }
    let absolute = state.absolute_mode.unwrap_or(true);
    for axis in ['X', 'Y', 'Z'] {
        if let Some(value) = tokens.get(&axis) {
            let position = if absolute {
                *value
            } else {
                state.current_position(axis) + *value
            };
            state.set_position(axis, position);
            state.bounds.record(axis, position);
            check_axis_limit(axis, position, line_no, instruction, machine, findings);
        }
    }
}

fn check_axis_limit(
    axis: char,
    position: f64,
    line_no: usize,
    instruction: &ExistingInstruction,
    machine: Option<&Machine>,
    findings: &mut Vec<BoundaryFinding>,
) {
    let Some(envelope) = machine
        .and_then(|item| item.capabilities.as_ref())
        .and_then(|capabilities| capabilities.work_envelope_mm.as_ref())
    else {
        return;
    };
    let limit = match axis {
        'X' => envelope.x,
        'Y' => envelope.y,
        'Z' => envelope.z,
        _ => return,
    };
    if position > limit || position < -0.001 {
        findings.push(finding(
            BoundaryKind::WorkEnvelopeExceeded,
            BoundarySeverity::Error,
            format!("Axis {axis} position {position:.3} mm is outside machine envelope 0..{limit:.3} mm."),
            machine_id(instruction, machine),
            None,
            Some(line_no),
            Some("Repost with the target work envelope, revise work offset, or split the part."),
        ));
    }
}

fn improve_gcode(content: &str, state: &InstructionState, findings: &[BoundaryFinding]) -> String {
    let mut output = String::new();
    output.push_str("; dd-fabrication-server improved safety preflight\n");
    output.push_str("; Review all generated comments before running hardware.\n");
    if !state.units_mm {
        output.push_str("G21 ; enforce millimeters\n");
    }
    if state.absolute_mode.is_none() {
        output.push_str("G90 ; explicit absolute positioning\n");
    }
    if !state.seen_homing {
        output.push_str("G28 ; home axes before job\n");
    }
    for finding in findings.iter().filter(|item| {
        matches!(
            item.severity,
            BoundarySeverity::Warning | BoundarySeverity::Error | BoundarySeverity::Critical
        )
    }) {
        output.push_str("; REVIEW ");
        output.push_str(&format!("{:?}: {}\n", finding.kind, finding.message));
    }
    output.push_str(content);
    if !content.ends_with('\n') {
        output.push('\n');
    }
    output.push_str("; dd-fabrication-server end of advisory wrapper\n");
    output
}

fn learn_from_telemetry(
    request: FabricationTelemetryRequest,
) -> Result<TelemetryLearningResponse, String> {
    if request.events.is_empty() {
        return Err("events must not be empty".to_string());
    }
    if request.events.len() > 256 {
        return Err("events must contain at most 256 entries".to_string());
    }
    let request_id = request
        .request_id
        .clone()
        .unwrap_or_else(|| format!("fabrication-telemetry-{}", now_ms()));
    let mut reward = 0.0;
    let mut weight_total = 0.0;
    let mut policy_adjustments = Vec::new();
    let mut mdp_signals = Vec::new();
    let mut neural_examples = Vec::new();

    for event in &request.events {
        if !event.value.is_finite() {
            return Err(format!("metric {} has non-finite value", event.metric));
        }
        let weight = event.weight.unwrap_or(1.0).clamp(0.0, 100.0);
        let metric_reward = metric_reward(event);
        reward += metric_reward * weight;
        weight_total += weight.max(0.0001);
        mdp_signals.push(RewardWeight {
            signal: normalize_token(&event.metric),
            weight: metric_reward * weight,
        });
        if metric_reward < -0.25 {
            policy_adjustments.push(format!(
                "Reduce policy preference for {:?} on {:?}: metric {} scored {:.3}.",
                event.process, event.machine_id, event.metric, metric_reward
            ));
        } else if metric_reward > 0.35 {
            policy_adjustments.push(format!(
                "Increase policy preference for {:?} on {:?}: metric {} scored {:.3}.",
                event.process, event.machine_id, event.metric, metric_reward
            ));
        }
        neural_examples.push(format!(
            "features(metric={}, value={:.4}, target={:?}, process={:?}, machine={:?}) -> reward={:.4}",
            event.metric, event.value, event.target, event.process, event.machine_id, metric_reward
        ));
    }
    reward /= weight_total;
    let risk_state = if reward < -0.5 {
        "critical"
    } else if reward < -0.15 {
        "elevated"
    } else if reward < 0.25 {
        "nominal"
    } else {
        "improving"
    }
    .to_string();
    let mode = request
        .learning
        .as_ref()
        .and_then(|learning| learning.mode)
        .unwrap_or(LearningMode::Hybrid);
    if matches!(mode, LearningMode::Pomdp | LearningMode::Hybrid) {
        policy_adjustments.push(
            "Track hidden state estimates for tool wear, material variability, fixture rigidity, and operator availability."
                .to_string(),
        );
    }
    if matches!(mode, LearningMode::NeuralPolicy | LearningMode::Hybrid) {
        policy_adjustments.push(
            "Append neural examples to supervised preference data after redacting operator notes."
                .to_string(),
        );
    }

    Ok(TelemetryLearningResponse {
        ok: true,
        request_id,
        plan_id: request.plan_id,
        reward,
        risk_state,
        policy_adjustments,
        mdp_signals,
        neural_examples,
        generated_at_ms: now_ms(),
    })
}

fn metric_reward(event: &FabricationTelemetryEvent) -> f64 {
    if let Some(success) = event.success {
        if success {
            return 1.0;
        }
        return -1.0;
    }
    let Some(target) = event.target else {
        return if event.value <= 0.0 {
            0.2
        } else {
            -event.value.tanh()
        };
    };
    if target.abs() < f64::EPSILON {
        return -event.value.abs().tanh();
    }
    let normalized_error = (event.value - target) / target.abs();
    let lower_is_better = lower_is_better_metric(&event.metric);
    if lower_is_better {
        (-normalized_error).clamp(-1.0, 1.0)
    } else {
        normalized_error.clamp(-1.0, 1.0)
    }
}

fn lower_is_better_metric(metric: &str) -> bool {
    let metric = normalize_token(metric);
    metric.contains("scrap")
        || metric.contains("error")
        || metric.contains("intervention")
        || metric.contains("deviation")
        || metric.contains("cycle")
        || metric.contains("wear")
}

fn build_design_package(
    request: &FabricationRequest,
    plan_id: &str,
    material_class: MaterialClass,
    decomposition: &[PartSpec],
) -> DesignPackage {
    let design_id = format!("design-{plan_id}");
    let mut artifacts = Vec::new();
    artifacts.push(DesignArtifact {
        artifact_id: "intent-json".to_string(),
        format: "fabricationIntentJson".to_string(),
        content: json!({
            "name": request.objective.name,
            "material": request.objective.material,
            "materialClass": material_class,
            "quantity": request.objective.quantity.unwrap_or(1),
            "boundingBoxMm": request.objective.bounding_box_mm,
            "requiredFeatures": request.objective.required_features,
        })
        .to_string(),
        notes: vec!["Source intent normalized for CAM and learning policy selection.".to_string()],
    });
    artifacts.push(DesignArtifact {
        artifact_id: "pseudo-openscad".to_string(),
        format: "openscadSketch".to_string(),
        content: pseudo_openscad(&request.objective, decomposition),
        notes: vec![
            "This is a parametric placeholder for downstream mesh/CAD generation.".to_string(),
            "Replace with a checked CAD kernel artifact before production hardware execution."
                .to_string(),
        ],
    });
    DesignPackage {
        design_id,
        artifacts,
        decomposition: decomposition.to_vec(),
    }
}

fn pseudo_openscad(objective: &FabricationObjective, parts: &[PartSpec]) -> String {
    let mut output = String::new();
    output
        .push_str("// Generated by dd-fabrication-server as a parametric manufacturing sketch.\n");
    output.push_str("// Use as CAM intent, not as production geometry without review.\n");
    for part in parts {
        let module_name = slug(&part.part_id).replace('-', "_");
        output.push_str(&format!("module {module_name}() {{\n"));
        if objective.rotational_symmetry.unwrap_or(false) {
            let bbox = part.bounding_box_mm.as_ref();
            let radius = bbox.map(|item| item.x.min(item.y) / 2.0).unwrap_or(10.0);
            let height = bbox.map(|item| item.z).unwrap_or(20.0);
            output.push_str(&format!(
                "  cylinder(h = {height:.3}, r = {radius:.3}, $fn = 96);\n"
            ));
        } else {
            let bbox = part.bounding_box_mm.clone().unwrap_or(BoundingBoxMm {
                x: 10.0,
                y: 10.0,
                z: 10.0,
            });
            output.push_str(&format!(
                "  cube([{:.3}, {:.3}, {:.3}], center = false);\n",
                bbox.x, bbox.y, bbox.z
            ));
        }
        output.push_str("}\n\n");
    }
    output
}

fn build_learning_plan(
    request_id: &str,
    plan_id: &str,
    config: Option<&LearningConfig>,
    operations: &[FabricationOperation],
    boundaries: &[BoundaryFinding],
    intervention: bool,
) -> LearningPlan {
    let mode = config
        .and_then(|item| item.mode)
        .unwrap_or(LearningMode::Hybrid);
    let mut states = vec![
        "designRequested".to_string(),
        "processSelected".to_string(),
        "instructionsGenerated".to_string(),
        "machineReady".to_string(),
        "jobComplete".to_string(),
    ];
    if intervention {
        states.push("awaitingHumanIntervention".to_string());
    }
    if boundaries.iter().any(|item| {
        matches!(
            item.severity,
            BoundarySeverity::Error | BoundarySeverity::Critical
        )
    }) {
        states.push("blockedByBoundary".to_string());
    }
    let actions = operations
        .iter()
        .map(|operation| format!("{:?}:{}", operation.process, operation.machine_kind))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let observations = vec![
        "measuredToleranceMm".to_string(),
        "cycleTimeSeconds".to_string(),
        "humanInterventionMinutes".to_string(),
        "scrapOrReworkCount".to_string(),
        "machineAlarmCode".to_string(),
        "surfaceFinishRaUm".to_string(),
    ];
    let reward_signals = config
        .and_then(|item| item.reward_weights.clone())
        .unwrap_or_else(|| {
            vec![
                RewardWeight {
                    signal: "toleranceHit".to_string(),
                    weight: 2.0,
                },
                RewardWeight {
                    signal: "cycleTimeSeconds".to_string(),
                    weight: -0.4,
                },
                RewardWeight {
                    signal: "humanInterventionMinutes".to_string(),
                    weight: -1.2,
                },
                RewardWeight {
                    signal: "scrapOrReworkCount".to_string(),
                    weight: -2.0,
                },
            ]
        });
    let mut hidden_state_hints = config
        .and_then(|item| item.hidden_state_hint.clone())
        .unwrap_or_default();
    hidden_state_hints.extend([
        "toolWear".to_string(),
        "materialBatchVariation".to_string(),
        "fixtureRigidity".to_string(),
        "operatorAvailability".to_string(),
    ]);
    hidden_state_hints.sort();
    hidden_state_hints.dedup();
    let mut neural_training_hints = vec![
        "Encode geometry summaries, material class, machine capabilities, and boundary findings as features."
            .to_string(),
        "Train only on post-run telemetry with measured outcomes; do not treat generated plans as ground truth."
            .to_string(),
    ];
    if let Some(hint) = config.and_then(|item| item.neural_policy_hint.clone()) {
        neural_training_hints.push(hint);
    }
    let delegation_hints = vec![
        "This response includes a concrete dd-mdp-optimizer request and the deployment can publish it to dd.remote.mdp.optimize when NATS is configured."
            .to_string(),
        "Keep neural model inference behind an explicit policy service; this server emits features and reward labels."
            .to_string(),
    ];
    let mdp_request = matches!(
        mode,
        LearningMode::Mdp | LearningMode::Pomdp | LearningMode::Hybrid
    )
    .then(|| {
        build_mdp_request(
            request_id,
            plan_id,
            mode,
            &states,
            &actions,
            &observations,
            &reward_signals,
            operations,
            boundaries,
            intervention,
        )
    });
    LearningPlan {
        mode,
        mdp_states: states,
        actions,
        observations,
        reward_signals,
        mdp_request,
        mdp_publish_subject: Some(MDP_OPTIMIZE_SUBJECT.to_string()),
        pomdp_hidden_state_hints: hidden_state_hints,
        neural_training_hints,
        delegation_hints,
    }
}

fn build_mdp_request(
    request_id: &str,
    plan_id: &str,
    mode: LearningMode,
    states: &[String],
    actions: &[String],
    observations: &[String],
    reward_signals: &[RewardWeight],
    operations: &[FabricationOperation],
    boundaries: &[BoundaryFinding],
    intervention: bool,
) -> MdpOptimizationRequest {
    let states = if states.is_empty() {
        vec!["designRequested".to_string(), "jobComplete".to_string()]
    } else {
        states.to_vec()
    };
    let actions = if actions.is_empty() {
        vec!["ManualReview:Unknown".to_string()]
    } else {
        actions.to_vec()
    };
    let risk = risk_from_findings(boundaries);
    let human_penalty = if intervention { 0.7 } else { 0.0 };
    let mut transitions = Vec::new();
    let mut rewards = Vec::new();

    for (state_index, state) in states.iter().enumerate() {
        for action in &actions {
            let terminal = state == "jobComplete";
            let blocked = state == "blockedByBoundary";
            let intervention_state = state == "awaitingHumanIntervention";
            let next_state = if terminal {
                state.clone()
            } else if blocked {
                if action.contains("ManualReview") || action.contains("Inspect") {
                    "awaitingHumanIntervention".to_string()
                } else {
                    "blockedByBoundary".to_string()
                }
            } else if intervention_state {
                "jobComplete".to_string()
            } else {
                states
                    .get((state_index + 1).min(states.len() - 1))
                    .cloned()
                    .unwrap_or_else(|| "jobComplete".to_string())
            };
            transitions.push(MdpTransition {
                state: state.clone(),
                action: action.clone(),
                next_state: next_state.clone(),
                probability: 1.0,
            });
            let operation_risk = action_risk(action, operations);
            let terminal_reward = if next_state == "jobComplete" {
                2.0
            } else {
                0.0
            };
            let blocked_penalty = if next_state == "blockedByBoundary" {
                3.0
            } else {
                0.0
            };
            let intervention_penalty = if next_state == "awaitingHumanIntervention" {
                1.0 + human_penalty
            } else {
                0.0
            };
            let custom_reward = reward_signals
                .iter()
                .map(|signal| signal.weight.signum() * signal.weight.abs().min(2.0) * 0.05)
                .sum::<f64>();
            rewards.push(MdpReward {
                state: state.clone(),
                action: action.clone(),
                value: terminal_reward
                    - blocked_penalty
                    - intervention_penalty
                    - operation_risk
                    - risk
                    + custom_reward,
            });
        }
    }

    let (observations, observation_model, belief, observed) =
        if matches!(mode, LearningMode::Pomdp | LearningMode::Hybrid) {
            let mut observation_labels = observations.to_vec();
            observation_labels.extend([
                "operatorAvailable".to_string(),
                "toolWearHigh".to_string(),
                "fixtureUncertain".to_string(),
            ]);
            observation_labels.sort();
            observation_labels.dedup();
            let mut observation_model = Vec::new();
            for action in &actions {
                for next_state in &states {
                    let observation_weights = observation_labels
                        .iter()
                        .map(|observation| observation_weight(next_state, observation))
                        .collect::<Vec<_>>();
                    let observation_weight_total =
                        observation_weights.iter().sum::<f64>().max(0.0001);
                    for observation in &observation_labels {
                        observation_model.push(MdpObservation {
                            action: action.clone(),
                            next_state: next_state.clone(),
                            observation: observation.clone(),
                            probability: observation_weight(next_state, observation)
                                / observation_weight_total,
                        });
                    }
                }
            }
            let belief = normalize_belief(
                states
                    .iter()
                    .map(|state| MdpBelief {
                        state: state.clone(),
                        probability: if state == "blockedByBoundary" {
                            risk.max(0.05)
                        } else if state == "awaitingHumanIntervention" {
                            if intervention {
                                0.25
                            } else {
                                0.05
                            }
                        } else if state == "jobComplete" {
                            0.01
                        } else {
                            0.25
                        },
                    })
                    .collect(),
            );
            (
                Some(observation_labels),
                Some(observation_model),
                Some(belief),
                Some(if intervention {
                    "operatorAvailable".to_string()
                } else {
                    "measuredToleranceMm".to_string()
                }),
            )
        } else {
            (None, None, None, None)
        };

    MdpOptimizationRequest {
        request_id: Some(format!("fabrication-policy-{plan_id}-{request_id}")),
        kind: Some(
            if matches!(mode, LearningMode::Pomdp | LearningMode::Hybrid) {
                "pomdp.fabrication-policy".to_string()
            } else {
                "mdp.fabrication-policy".to_string()
            },
        ),
        states,
        actions,
        transitions,
        rewards,
        observations,
        observation_model,
        belief,
        belief_action: None,
        observed,
        gamma: Some(0.86),
        tolerance: Some(0.0001),
        max_iterations: Some(2000),
    }
}

fn action_risk(action: &str, operations: &[FabricationOperation]) -> f64 {
    operations
        .iter()
        .find(|operation| action == format!("{:?}:{}", operation.process, operation.machine_kind))
        .map(|operation| operation.estimated_risk)
        .unwrap_or(0.4)
        .clamp(0.0, 1.0)
}

fn observation_weight(next_state: &str, observation: &str) -> f64 {
    if next_state == "blockedByBoundary"
        && matches!(
            observation,
            "machineAlarmCode" | "toolWearHigh" | "fixtureUncertain"
        )
    {
        0.34
    } else if next_state == "awaitingHumanIntervention" && observation == "operatorAvailable" {
        0.45
    } else if next_state == "jobComplete"
        && matches!(observation, "measuredToleranceMm" | "surfaceFinishRaUm")
    {
        0.35
    } else {
        0.05
    }
}

fn normalize_belief(mut belief: Vec<MdpBelief>) -> Vec<MdpBelief> {
    let total = belief
        .iter()
        .map(|item| item.probability.max(0.0))
        .sum::<f64>();
    if total <= f64::EPSILON {
        let uniform = 1.0 / belief.len().max(1) as f64;
        for item in &mut belief {
            item.probability = uniform;
        }
        return belief;
    }
    for item in &mut belief {
        item.probability = item.probability.max(0.0) / total;
    }
    belief
}

fn process_hint(
    objective: &FabricationObjective,
    material_class: MaterialClass,
) -> FabricationProcess {
    if objective.rotational_symmetry.unwrap_or(false)
        && matches!(
            material_class,
            MaterialClass::Metal | MaterialClass::Wood | MaterialClass::Wax
        )
    {
        return FabricationProcess::Turn;
    }
    match material_class {
        MaterialClass::Metal | MaterialClass::Composite | MaterialClass::Ceramic => {
            FabricationProcess::MillRoughing
        }
        MaterialClass::Resin => FabricationProcess::ResinPrint,
        MaterialClass::Plastic | MaterialClass::Wax => FabricationProcess::FdmPrint,
        MaterialClass::Wood => FabricationProcess::Route,
        MaterialClass::Unknown => FabricationProcess::ManualReview,
    }
}

fn process_for_machine(
    machine_kind: Option<MachineKind>,
    fallback: FabricationProcess,
) -> FabricationProcess {
    match machine_kind {
        Some(MachineKind::FdmPrinter) => FabricationProcess::FdmPrint,
        Some(MachineKind::SlaPrinter | MachineKind::DlpPrinter) => FabricationProcess::ResinPrint,
        Some(MachineKind::SlsPrinter | MachineKind::BinderJet) => FabricationProcess::PowderPrint,
        Some(MachineKind::VerticalMill | MachineKind::HorizontalMill) => {
            FabricationProcess::MillRoughing
        }
        Some(MachineKind::Lathe) => FabricationProcess::Turn,
        Some(MachineKind::CncRouter) => FabricationProcess::Route,
        Some(MachineKind::LaserCutter | MachineKind::Waterjet | MachineKind::WireEdm) => {
            FabricationProcess::Cut
        }
        _ => fallback,
    }
}

fn machine_kind_for_process(process: FabricationProcess) -> MachineKind {
    match process {
        FabricationProcess::FdmPrint => MachineKind::FdmPrinter,
        FabricationProcess::ResinPrint => MachineKind::SlaPrinter,
        FabricationProcess::PowderPrint => MachineKind::SlsPrinter,
        FabricationProcess::MillRoughing | FabricationProcess::MillFinishing => {
            MachineKind::VerticalMill
        }
        FabricationProcess::Turn => MachineKind::Lathe,
        FabricationProcess::Route => MachineKind::CncRouter,
        FabricationProcess::Cut => MachineKind::Waterjet,
        FabricationProcess::Assemble => MachineKind::ManualAssembly,
        _ => MachineKind::Unknown,
    }
}

fn intent_for_process(process: FabricationProcess, tolerance: f64) -> String {
    match process {
        FabricationProcess::FdmPrint => {
            format!(
                "Print near-net geometry with allowance for tolerance target {tolerance:.3} mm."
            )
        }
        FabricationProcess::ResinPrint | FabricationProcess::PowderPrint => {
            format!("Print high-detail near-net geometry and reserve post-processing for tolerance target {tolerance:.3} mm.")
        }
        FabricationProcess::MillRoughing => {
            "Rough stock to near-net geometry with safe setup and workholding checks.".to_string()
        }
        FabricationProcess::MillFinishing => {
            format!("Finish datums, bores, and fit-critical surfaces to {tolerance:.3} mm.")
        }
        FabricationProcess::Turn => {
            "Turn cylindrical features, shoulders, and datum diameters.".to_string()
        }
        FabricationProcess::Route => {
            "Route sheet or wood/composite profile with tabs and fixture verification.".to_string()
        }
        FabricationProcess::Cut => {
            "Cut 2.5D profile after nesting, kerf compensation, and fixture review.".to_string()
        }
        _ => "Review and execute manual or controller-specific operation.".to_string(),
    }
}

fn setup_notes_for(
    objective: &FabricationObjective,
    part: &PartSpec,
    machine: Option<&Machine>,
    process: FabricationProcess,
    quantity: u32,
) -> Vec<String> {
    let mut notes = vec![
        format!("Quantity: {quantity}; part: {}", part.name),
        format!("Material class: {:?}", part.material_class),
    ];
    if let Some(machine) = machine {
        notes.push(format!(
            "Target machine: {} ({:?})",
            machine.id, machine.kind
        ));
    } else {
        notes.push("No machine selected; operator must route this operation.".to_string());
    }
    if objective.hollow.unwrap_or(false) || objective.enclosed_cavities.unwrap_or(false) {
        notes.push(
            "Inspect hollow/enclosed features for trapped support, chips, powder, or resin."
                .to_string(),
        );
    }
    if matches!(
        process,
        FabricationProcess::MillRoughing | FabricationProcess::MillFinishing
    ) {
        notes.push("Verify work offset, tool length, cutter diameter, clearance plane, and fixture clamps.".to_string());
    }
    notes
}

fn safety_notes_for(process: FabricationProcess, machine: Option<&Machine>) -> Vec<String> {
    let mut notes =
        vec!["Dry-run or simulate instructions before energizing hardware.".to_string()];
    if let Some(machine) = machine {
        notes.push(format!(
            "Use only with validated machine profile '{}'.",
            machine.id
        ));
    }
    match process {
        FabricationProcess::FdmPrint
        | FabricationProcess::ResinPrint
        | FabricationProcess::PowderPrint => notes.push(
            "Thermal, resin, powder, wash, cure, and ventilation controls remain operator-owned."
                .to_string(),
        ),
        FabricationProcess::MillRoughing
        | FabricationProcess::MillFinishing
        | FabricationProcess::Turn
        | FabricationProcess::Route
        | FabricationProcess::Cut => notes.push(
            "Verify stock, fixtures, toolpath simulation, coolant/air, and emergency stop reach before cycle start."
                .to_string(),
        ),
        _ => {}
    }
    notes
}

fn machine_needs_human_tooling(machine: Option<&Machine>, process: FabricationProcess) -> bool {
    if matches!(
        process,
        FabricationProcess::MillRoughing
            | FabricationProcess::MillFinishing
            | FabricationProcess::Turn
            | FabricationProcess::Route
            | FabricationProcess::Cut
    ) {
        return true;
    }
    machine
        .and_then(|item| item.capabilities.as_ref())
        .and_then(|capabilities| capabilities.supports_tool_change)
        .unwrap_or(false)
}

fn default_tolerance(process: FabricationProcess) -> f64 {
    match process {
        FabricationProcess::FdmPrint => 0.25,
        FabricationProcess::ResinPrint | FabricationProcess::PowderPrint => 0.12,
        FabricationProcess::MillRoughing | FabricationProcess::Route | FabricationProcess::Cut => {
            0.2
        }
        FabricationProcess::MillFinishing | FabricationProcess::Turn => 0.05,
        _ => 0.5,
    }
}

fn base_risk(process: FabricationProcess) -> f64 {
    match process {
        FabricationProcess::FdmPrint => 0.22,
        FabricationProcess::ResinPrint | FabricationProcess::PowderPrint => 0.3,
        FabricationProcess::MillRoughing | FabricationProcess::Route | FabricationProcess::Cut => {
            0.45
        }
        FabricationProcess::MillFinishing => 0.38,
        FabricationProcess::Turn => 0.42,
        _ => 0.5,
    }
}

fn risk_from_findings(findings: &[BoundaryFinding]) -> f64 {
    let mut risk: f64 = 0.0;
    for finding in findings {
        risk += match finding.severity {
            BoundarySeverity::Info => 0.03,
            BoundarySeverity::Warning => 0.12,
            BoundarySeverity::Error => 0.28,
            BoundarySeverity::Critical => 0.45,
        };
    }
    risk.min(1.0)
}

fn summarize_processes(operations: &[FabricationOperation]) -> Vec<String> {
    operations
        .iter()
        .map(|operation| format!("{:?} on {:?}", operation.process, operation.machine_kind))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn find_machine_for_instruction<'a>(
    machines: &'a [Machine],
    instruction: &ExistingInstruction,
) -> Option<&'a Machine> {
    if let Some(machine_id) = instruction.machine_id.as_deref() {
        if let Some(machine) = machines.iter().find(|machine| machine.id == machine_id) {
            return Some(machine);
        }
    }
    instruction
        .machine_kind
        .and_then(|kind| machines.iter().find(|machine| machine.kind == kind))
}

fn infer_material_class(provided: Option<MaterialClass>, material: Option<&str>) -> MaterialClass {
    if let Some(provided) = provided {
        return provided;
    }
    let Some(material) = material else {
        return MaterialClass::Unknown;
    };
    let material = normalize_token(material);
    if matches!(
        material.as_str(),
        "pla" | "petg" | "abs" | "asa" | "nylon" | "pc" | "polycarbonate" | "plastic"
    ) {
        MaterialClass::Plastic
    } else if matches!(material.as_str(), "resin" | "photopolymer" | "sla") {
        MaterialClass::Resin
    } else if matches!(
        material.as_str(),
        "aluminum"
            | "aluminium"
            | "steel"
            | "stainless"
            | "brass"
            | "copper"
            | "titanium"
            | "metal"
    ) {
        MaterialClass::Metal
    } else if matches!(material.as_str(), "wood" | "plywood" | "mdf" | "hardwood") {
        MaterialClass::Wood
    } else if matches!(
        material.as_str(),
        "carbonfiber" | "fiberglass" | "composite"
    ) {
        MaterialClass::Composite
    } else if matches!(material.as_str(), "ceramic" | "clay" | "porcelain") {
        MaterialClass::Ceramic
    } else if matches!(material.as_str(), "wax" | "machinablewax") {
        MaterialClass::Wax
    } else {
        MaterialClass::Unknown
    }
}

fn material_class_token(material_class: MaterialClass) -> &'static str {
    match material_class {
        MaterialClass::Plastic => "plastic",
        MaterialClass::Resin => "resin",
        MaterialClass::Metal => "metal",
        MaterialClass::Wood => "wood",
        MaterialClass::Composite => "composite",
        MaterialClass::Ceramic => "ceramic",
        MaterialClass::Wax => "wax",
        MaterialClass::Unknown => "unknown",
    }
}

fn print_temps(material: &str) -> (f64, f64) {
    match normalize_token(material).as_str() {
        "petg" => (240.0, 80.0),
        "abs" | "asa" => (250.0, 105.0),
        "nylon" => (265.0, 80.0),
        "pla" => (210.0, 60.0),
        _ => (215.0, 60.0),
    }
}

fn part_box_text(part_box: Option<&BoundingBoxMm>) -> String {
    part_box
        .map(|item| format!("{:.1} x {:.1} x {:.1} mm", item.x, item.y, item.z))
        .unwrap_or_else(|| "unspecified".to_string())
}

fn strip_instruction_comment(line: &str) -> String {
    let mut cleaned = String::new();
    let mut in_paren = false;
    for char in line.chars() {
        if char == ';' {
            break;
        }
        if char == '(' {
            in_paren = true;
            continue;
        }
        if char == ')' {
            in_paren = false;
            continue;
        }
        if !in_paren {
            cleaned.push(char);
        }
    }
    cleaned
}

fn parse_words(line: &str) -> BTreeMap<char, f64> {
    let mut words = BTreeMap::new();
    for token in line.split_whitespace() {
        let mut chars = token.chars();
        let Some(letter) = chars.next().map(|char| char.to_ascii_uppercase()) else {
            continue;
        };
        if !letter.is_ascii_alphabetic() {
            continue;
        }
        let value = chars.as_str().trim();
        if let Ok(parsed) = value.parse::<f64>() {
            words.insert(letter, parsed);
        }
    }
    words
}

fn command_label(letter: char, value: f64) -> String {
    let rounded = value.round();
    if (value - rounded).abs() < 0.0001 {
        format!("{letter}{}", rounded as i64)
    } else {
        format!("{letter}{value:.3}")
    }
}

#[derive(Debug, Clone)]
struct InstructionState {
    seen_homing: bool,
    moved_before_homing: bool,
    units_mm: bool,
    absolute_mode: Option<bool>,
    position_x: f64,
    position_y: f64,
    position_z: f64,
    bounds: InstructionBounds,
}

impl Default for InstructionState {
    fn default() -> Self {
        Self {
            seen_homing: false,
            moved_before_homing: false,
            units_mm: true,
            absolute_mode: None,
            position_x: 0.0,
            position_y: 0.0,
            position_z: 0.0,
            bounds: InstructionBounds::default(),
        }
    }
}

impl InstructionState {
    fn current_position(&self, axis: char) -> f64 {
        match axis {
            'X' => self.position_x,
            'Y' => self.position_y,
            'Z' => self.position_z,
            _ => 0.0,
        }
    }

    fn set_position(&mut self, axis: char, value: f64) {
        match axis {
            'X' => self.position_x = value,
            'Y' => self.position_y = value,
            'Z' => self.position_z = value,
            _ => {}
        }
    }
}

impl InstructionBounds {
    fn record(&mut self, axis: char, value: f64) {
        match axis {
            'X' => {
                self.min_x = Some(
                    self.min_x
                        .map(|current| current.min(value))
                        .unwrap_or(value),
                );
                self.max_x = Some(
                    self.max_x
                        .map(|current| current.max(value))
                        .unwrap_or(value),
                );
            }
            'Y' => {
                self.min_y = Some(
                    self.min_y
                        .map(|current| current.min(value))
                        .unwrap_or(value),
                );
                self.max_y = Some(
                    self.max_y
                        .map(|current| current.max(value))
                        .unwrap_or(value),
                );
            }
            'Z' => {
                self.min_z = Some(
                    self.min_z
                        .map(|current| current.min(value))
                        .unwrap_or(value),
                );
                self.max_z = Some(
                    self.max_z
                        .map(|current| current.max(value))
                        .unwrap_or(value),
                );
            }
            _ => {}
        }
    }

    fn non_empty(&self) -> bool {
        self.min_x.is_some()
            || self.max_x.is_some()
            || self.min_y.is_some()
            || self.max_y.is_some()
            || self.min_z.is_some()
            || self.max_z.is_some()
    }
}

fn improvement_summary(findings: &[BoundaryFinding]) -> Vec<String> {
    if findings.is_empty() {
        return vec!["No blocking issue found by the generic instruction validator.".to_string()];
    }
    findings
        .iter()
        .filter_map(|finding| finding.recommendation.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn machine_id(instruction: &ExistingInstruction, machine: Option<&Machine>) -> Option<String> {
    instruction
        .machine_id
        .clone()
        .or_else(|| machine.map(|item| item.id.clone()))
}

fn finding(
    kind: BoundaryKind,
    severity: BoundarySeverity,
    message: impl Into<String>,
    machine_id: Option<String>,
    operation_id: Option<String>,
    line: Option<usize>,
    recommendation: Option<&str>,
) -> BoundaryFinding {
    BoundaryFinding {
        kind,
        severity,
        message: message.into(),
        machine_id,
        operation_id,
        line,
        recommendation: recommendation.map(|value| value.to_string()),
    }
}

fn process_slug(process: FabricationProcess) -> &'static str {
    match process {
        FabricationProcess::FdmPrint => "fdm",
        FabricationProcess::ResinPrint => "resin",
        FabricationProcess::PowderPrint => "powder",
        FabricationProcess::MillRoughing => "mill-rough",
        FabricationProcess::MillFinishing => "mill-finish",
        FabricationProcess::Turn => "turn",
        FabricationProcess::Route => "route",
        FabricationProcess::Cut => "cut",
        FabricationProcess::Inspect => "inspect",
        FabricationProcess::Assemble => "assemble",
        FabricationProcess::ValidateExisting => "validate",
        FabricationProcess::ImproveExisting => "improve",
        FabricationProcess::ManualReview => "review",
    }
}

fn normalize_token(value: &str) -> String {
    value
        .chars()
        .filter(|char| char.is_ascii_alphanumeric())
        .flat_map(|char| char.to_lowercase())
        .collect()
}

fn slug(value: &str) -> String {
    let mut output = String::new();
    let mut last_dash = false;
    for char in value.chars() {
        if char.is_ascii_alphanumeric() {
            output.push(char.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash {
            output.push('-');
            last_dash = true;
        }
    }
    output.trim_matches('-').to_string()
}

fn stable_hash(value: &str) -> u64 {
    let mut hash = 14_695_981_039_346_656_037_u64;
    for byte in value.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(1_099_511_628_211);
    }
    hash
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn env_value(name: &str, default: &str) -> String {
    env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| default.to_string())
}

fn env_bool(name: &str, default: bool) -> bool {
    env::var(name)
        .ok()
        .and_then(|value| match normalize_token(&value).as_str() {
            "1" | "true" | "yes" | "on" => Some(true),
            "0" | "false" | "no" | "off" => Some(false),
            _ => None,
        })
        .unwrap_or(default)
}

async fn publish_fabrication_result(state: &AppState, response: &FabricationResponse) {
    let Some(nats) = state.nats.as_ref() else {
        return;
    };
    let Ok(payload) = serde_json::to_vec(response) else {
        return;
    };
    if let Err(error) = nats
        .publish(state.result_subject.clone(), payload.into())
        .await
    {
        eprintln!("failed to publish fabrication result: {error}");
        return;
    }
    let _ = nats
        .publish(
            state.event_subject.clone(),
            json!({
                "type": "fabrication.result",
                "source": "dd-fabrication-server",
                "requestId": response.request_id,
                "planId": response.plan_id,
                "status": response.status,
                "findings": response.boundaries.len(),
                "instructions": response.instructions.len(),
                "atMs": now_ms(),
            })
            .to_string()
            .into(),
        )
        .await;
    publish_mdp_request(state, response).await;
}

async fn publish_mdp_request(state: &AppState, response: &FabricationResponse) {
    if !state.mdp_auto_publish {
        return;
    }
    let Some(nats) = state.nats.as_ref() else {
        return;
    };
    let Some(mdp_request) = response.learning.mdp_request.as_ref() else {
        return;
    };
    let Ok(payload) = serde_json::to_vec(mdp_request) else {
        return;
    };
    if let Err(error) = nats
        .publish(state.mdp_optimize_subject.clone(), payload.into())
        .await
    {
        eprintln!("failed to publish fabrication mdp request: {error}");
        return;
    }
    state
        .metrics
        .mdp_delegations_total
        .fetch_add(1, Ordering::Relaxed);
    let _ = nats
        .publish(
            state.event_subject.clone(),
            json!({
                "type": "fabrication.mdp.optimize.requested",
                "source": "dd-fabrication-server",
                "requestId": response.request_id,
                "planId": response.plan_id,
                "mdpSubject": state.mdp_optimize_subject.as_str(),
                "states": mdp_request.states.len(),
                "actions": mdp_request.actions.len(),
                "atMs": now_ms(),
            })
            .to_string()
            .into(),
        )
        .await;
}

async fn run_nats_loop(state: AppState, subject: String, queue_group: String) {
    let Some(nats) = state.nats.clone() else {
        println!("fabrication server nats loop disabled: NATS_URL is not configured");
        return;
    };
    println!(
        "fabrication server nats loop starting: subject={subject} queue_group={queue_group} resultSubject={}",
        state.result_subject
    );
    let mut subscription = match nats.queue_subscribe(subject, queue_group).await {
        Ok(subscription) => subscription,
        Err(error) => {
            eprintln!("fabrication server nats subscribe failed: {error}");
            return;
        }
    };
    while let Some(message) = subscription.next().await {
        state
            .metrics
            .nats_messages_total
            .fetch_add(1, Ordering::Relaxed);
        let payload = message.payload.to_vec();
        if payload.len() > MAX_NATS_PAYLOAD_BYTES {
            state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
            eprintln!(
                "fabrication server rejected oversize nats request: bytes={} max={MAX_NATS_PAYLOAD_BYTES}",
                payload.len()
            );
            continue;
        }
        let task_state = state.clone();
        tokio::spawn(async move {
            match serde_json::from_slice::<FabricationRequest>(&payload) {
                Ok(request) => match plan_fabrication(request) {
                    Ok(response) => {
                        task_state
                            .metrics
                            .requests_total
                            .fetch_add(1, Ordering::Relaxed);
                        task_state
                            .metrics
                            .generated_instructions_total
                            .fetch_add(response.instructions.len() as u64, Ordering::Relaxed);
                        task_state
                            .metrics
                            .findings_total
                            .fetch_add(response.boundaries.len() as u64, Ordering::Relaxed);
                        publish_fabrication_result(&task_state, &response).await;
                    }
                    Err(error) => {
                        task_state
                            .metrics
                            .errors_total
                            .fetch_add(1, Ordering::Relaxed);
                        eprintln!("fabrication server failed nats request: {error}");
                    }
                },
                Err(error) => {
                    task_state
                        .metrics
                        .errors_total
                        .fetch_add(1, Ordering::Relaxed);
                    eprintln!("fabrication server invalid nats request: {error}");
                }
            }
        });
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    let host = env_value("HOST", "0.0.0.0");
    let port = env_value("PORT", "8113").parse::<u16>()?;
    let nats = match env::var("NATS_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
    {
        Some(url) => Some(async_nats::connect(url).await?),
        None => None,
    };
    let state = AppState {
        nats,
        result_subject: env_value("FABRICATION_RESULT_SUBJECT", FABRICATION_RESULTS_SUBJECT),
        event_subject: env_value("FABRICATION_EVENT_SUBJECT", RUNTIME_EVENTS_SUBJECT),
        mdp_optimize_subject: env_value("FABRICATION_MDP_OPTIMIZE_SUBJECT", MDP_OPTIMIZE_SUBJECT),
        mdp_auto_publish: env_bool("FABRICATION_MDP_AUTOPUBLISH", true),
        metrics: Arc::new(Metrics::default()),
    };
    let nats_subject = env_value("FABRICATION_REQUEST_SUBJECT", FABRICATION_REQUESTS_SUBJECT);
    let queue_group = env_value("FABRICATION_QUEUE_GROUP", FABRICATION_REQUESTS_QUEUE_GROUP);
    tokio::spawn(run_nats_loop(state.clone(), nats_subject, queue_group));

    let app = Router::new()
        .route("/", get(healthz))
        .route("/healthz", get(healthz))
        .route("/schema", get(schema))
        .route("/example", get(example))
        .route("/docs/api", get(api_docs_html))
        .route("/api/docs", get(api_docs_html))
        .route("/api/docs.json", get(api_docs_json))
        .route("/metrics", get(metrics))
        .route("/fabricate", post(fabricate_http))
        .route("/instructions/validate", post(validate_instructions_http))
        .route("/instructions/improve", post(improve_instructions_http))
        .route("/learn/telemetry", post(learn_telemetry_http))
        .layer(DefaultBodyLimit::max(MAX_HTTP_BODY_BYTES))
        .with_state(state)
        .merge(dd_runtime_config_client::router());

    tokio::spawn(dd_runtime_config_client::register_with_control_plane());

    let addr: SocketAddr = format!("{host}:{port}").parse()?;
    println!("dd-fabrication-server listening on http://{addr}");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    tokio::time::sleep(Duration::from_millis(10)).await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn machine(
        id: &str,
        kind: MachineKind,
        envelope: BoundingBoxMm,
        materials: &[&str],
    ) -> Machine {
        Machine {
            id: id.to_string(),
            kind,
            capabilities: Some(MachineCapabilities {
                work_envelope_mm: Some(envelope),
                materials: Some(materials.iter().map(|item| item.to_string()).collect()),
                axes: Some(3),
                nozzle_diameter_mm: matches!(kind, MachineKind::FdmPrinter).then_some(0.4),
                tool_diameters_mm: matches!(
                    kind,
                    MachineKind::VerticalMill
                        | MachineKind::HorizontalMill
                        | MachineKind::CncRouter
                )
                .then_some(vec![3.0, 6.0]),
                max_spindle_rpm: matches!(
                    kind,
                    MachineKind::VerticalMill | MachineKind::HorizontalMill | MachineKind::Lathe
                )
                .then_some(5000.0),
                max_feed_mm_min: Some(1000.0),
                min_layer_height_mm: matches!(kind, MachineKind::FdmPrinter).then_some(0.08),
                min_tolerance_mm: Some(if matches!(kind, MachineKind::FdmPrinter) {
                    0.2
                } else {
                    0.03
                }),
                max_extruder_temp_c: matches!(kind, MachineKind::FdmPrinter).then_some(285.0),
                max_bed_temp_c: matches!(kind, MachineKind::FdmPrinter).then_some(110.0),
                supports_tool_change: Some(!matches!(kind, MachineKind::FdmPrinter)),
                supports_auto_homing: Some(true),
                notes: None,
            }),
        }
    }

    fn request_for_plastic() -> FabricationRequest {
        FabricationRequest {
            request_id: Some("req-1".to_string()),
            objective: FabricationObjective {
                name: "bracket".to_string(),
                description: None,
                material: Some("PETG".to_string()),
                material_class: None,
                quantity: Some(1),
                bounding_box_mm: Some(BoundingBoxMm {
                    x: 100.0,
                    y: 40.0,
                    z: 20.0,
                }),
                tolerance_mm: Some(0.1),
                surface_finish: None,
                mass_g: None,
                rotational_symmetry: Some(false),
                hollow: Some(false),
                enclosed_cavities: Some(false),
                overhang_degrees: Some(55.0),
                min_wall_mm: Some(0.6),
                strength_priority: Some(0.8),
                aesthetic_priority: None,
                inspectability_priority: None,
                required_features: None,
            },
            available_machines: vec![
                machine(
                    "printer",
                    MachineKind::FdmPrinter,
                    BoundingBoxMm {
                        x: 220.0,
                        y: 220.0,
                        z: 250.0,
                    },
                    &["PLA", "PETG"],
                ),
                machine(
                    "mill",
                    MachineKind::VerticalMill,
                    BoundingBoxMm {
                        x: 500.0,
                        y: 250.0,
                        z: 300.0,
                    },
                    &["plastic", "aluminum"],
                ),
            ],
            stock: None,
            existing_instructions: None,
            learning: Some(LearningConfig {
                mode: Some(LearningMode::Hybrid),
                horizon: Some(4),
                exploration_budget: None,
                hidden_state_hint: None,
                reward_weights: None,
                neural_policy_hint: None,
            }),
        }
    }

    #[test]
    fn plans_printed_part_with_finishing_and_boundaries() {
        let response = plan_fabrication(request_for_plastic()).expect("plan");
        assert_eq!(response.status, "plannedWithIntervention");
        assert!(response
            .plan
            .operations
            .iter()
            .any(|operation| operation.process == FabricationProcess::FdmPrint));
        assert!(response
            .plan
            .operations
            .iter()
            .any(|operation| operation.process == FabricationProcess::MillFinishing));
        assert!(response
            .boundaries
            .iter()
            .any(|finding| finding.kind == BoundaryKind::HumanInterventionRequired));
        assert!(response
            .learning
            .actions
            .iter()
            .any(|item| item.contains("FdmPrint")));
        let generated_content = response
            .instructions
            .iter()
            .map(|instruction| instruction.content.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        let placeholder_marker = format!("{}_{}", "TODO", "CAM");
        assert!(!generated_content.contains(&placeholder_marker));
        assert!(generated_content.contains("; layer 1 z="));
        assert!(generated_content.contains("Primitive mill path"));
        let mdp_request = response
            .learning
            .mdp_request
            .as_ref()
            .expect("hybrid learning should include an optimizer request");
        assert_eq!(
            mdp_request.kind.as_deref(),
            Some("pomdp.fabrication-policy")
        );
        assert_eq!(mdp_request.transitions.len(), mdp_request.rewards.len());
        assert!(mdp_request
            .observations
            .as_ref()
            .is_some_and(|observations| observations.contains(&"toolWearHigh".to_string())));
        assert!(mdp_request.belief.as_ref().is_some_and(|belief| {
            (belief.iter().map(|item| item.probability).sum::<f64>() - 1.0).abs() < 0.0001
        }));
    }

    #[test]
    fn splits_part_that_exceeds_machine_envelope() {
        let mut request = request_for_plastic();
        request.objective.bounding_box_mm = Some(BoundingBoxMm {
            x: 600.0,
            y: 80.0,
            z: 20.0,
        });
        request.available_machines = vec![machine(
            "printer",
            MachineKind::FdmPrinter,
            BoundingBoxMm {
                x: 200.0,
                y: 200.0,
                z: 200.0,
            },
            &["PETG"],
        )];
        let response = plan_fabrication(request).expect("plan");
        assert!(response.design.decomposition.len() >= 3);
        assert!(response
            .boundaries
            .iter()
            .any(|finding| finding.kind == BoundaryKind::SplitRequired));
    }

    #[test]
    fn chooses_lathe_for_rotational_metal() {
        let request = FabricationRequest {
            request_id: Some("shaft".to_string()),
            objective: FabricationObjective {
                name: "shaft".to_string(),
                description: None,
                material: Some("aluminum".to_string()),
                material_class: None,
                quantity: Some(1),
                bounding_box_mm: Some(BoundingBoxMm {
                    x: 25.0,
                    y: 25.0,
                    z: 100.0,
                }),
                tolerance_mm: Some(0.02),
                surface_finish: None,
                mass_g: None,
                rotational_symmetry: Some(true),
                hollow: None,
                enclosed_cavities: None,
                overhang_degrees: None,
                min_wall_mm: None,
                strength_priority: None,
                aesthetic_priority: None,
                inspectability_priority: None,
                required_features: None,
            },
            available_machines: vec![machine(
                "lathe",
                MachineKind::Lathe,
                BoundingBoxMm {
                    x: 300.0,
                    y: 300.0,
                    z: 500.0,
                },
                &["aluminum", "steel"],
            )],
            stock: None,
            existing_instructions: None,
            learning: None,
        };
        let response = plan_fabrication(request).expect("plan");
        assert!(response
            .plan
            .operations
            .iter()
            .any(|operation| operation.process == FabricationProcess::Turn));
        assert!(response
            .instructions
            .iter()
            .any(|instruction| instruction.content.contains("Primitive lathe path")));
        assert!(response.instructions.iter().all(|instruction| !instruction
            .content
            .contains(&format!("{}_{}", "TODO", "CAM"))));
    }

    #[test]
    fn validates_gcode_boundaries_and_improves() {
        let instruction = ExistingInstruction {
            instruction_id: Some("bad-gcode".to_string()),
            machine_id: Some("printer".to_string()),
            machine_kind: Some(MachineKind::FdmPrinter),
            format: Some(InstructionFormat::MarlinGcode),
            content: "G1 X250 Y10 F2000\nM109 S310\nM600\n".to_string(),
        };
        let machine = machine(
            "printer",
            MachineKind::FdmPrinter,
            BoundingBoxMm {
                x: 220.0,
                y: 220.0,
                z: 250.0,
            },
            &["PLA"],
        );
        let analysis = analyze_instruction(&instruction, Some(&machine), true).expect("analysis");
        assert!(analysis
            .findings
            .iter()
            .any(|finding| finding.kind == BoundaryKind::WorkEnvelopeExceeded));
        assert!(analysis
            .findings
            .iter()
            .any(|finding| finding.kind == BoundaryKind::ThermalLimit));
        assert!(analysis
            .findings
            .iter()
            .any(|finding| finding.kind == BoundaryKind::HumanInterventionRequired));
        assert!(analysis.improved_content.unwrap().contains("G28"));
    }

    #[test]
    fn learns_from_negative_telemetry() {
        let response = learn_from_telemetry(FabricationTelemetryRequest {
            request_id: Some("learn".to_string()),
            plan_id: Some("plan".to_string()),
            learning: Some(LearningConfig {
                mode: Some(LearningMode::Pomdp),
                horizon: None,
                exploration_budget: None,
                hidden_state_hint: None,
                reward_weights: None,
                neural_policy_hint: None,
            }),
            events: vec![FabricationTelemetryEvent {
                operation_id: Some("op-1".to_string()),
                machine_id: Some("mill".to_string()),
                process: Some(FabricationProcess::MillFinishing),
                metric: "scrapRate".to_string(),
                value: 0.4,
                target: Some(0.05),
                weight: Some(1.0),
                success: None,
                observation: None,
            }],
        })
        .expect("learn");
        assert!(response.reward < 0.0);
        assert_eq!(response.risk_state, "critical");
        assert!(response
            .policy_adjustments
            .iter()
            .any(|item| item.contains("tool wear") || item.contains("toolWear")));
    }
}
