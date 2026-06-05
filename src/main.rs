use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    env,
    error::Error,
    net::SocketAddr,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, RwLock,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use axum::{
    extract::{DefaultBodyLimit, Path, State},
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
use serde_json::{json, Value};

const SERVICE_NAME: &str = "dd-fabrication-server";
const SCHEMA_VERSION: &str = "fabrication.plan.v1";
const MAX_HTTP_BODY_BYTES: usize = 512 * 1024;
const MAX_NATS_PAYLOAD_BYTES: usize = 512 * 1024;
const MAX_REQUEST_ID_LEN: usize = 128;
const MAX_TEXT_LEN: usize = 8_192;
const MAX_LABEL_LEN: usize = 96;
const MAX_MACHINES: usize = 32;
const MAX_PARTS: usize = 64;
const MAX_PROGRAMS: usize = 32;
const MAX_PROGRAM_LINES: usize = 8_000;
const MAX_STORED_JOBS: usize = 128;
const MAX_LEARNING_OUTCOMES: usize = 512;
const MAX_LEARNING_SIGNALS: usize = 128;
const DEFAULT_TOLERANCE_MM: f64 = 0.2;

#[derive(Clone)]
struct AppState {
    nats: Option<async_nats::Client>,
    request_subject: String,
    queue_group: String,
    result_subject: String,
    event_subject: String,
    mdp_subject: String,
    mdp_autopublish: bool,
    metrics: Arc<Metrics>,
    jobs: Arc<RwLock<FabricationJobStore>>,
    learning: Arc<RwLock<LearningMemory>>,
}

#[derive(Default)]
struct Metrics {
    plan_requests_total: AtomicU64,
    analysis_requests_total: AtomicU64,
    learning_requests_total: AtomicU64,
    generated_programs_total: AtomicU64,
    validation_findings_total: AtomicU64,
    failure_boundaries_total: AtomicU64,
    errors_total: AtomicU64,
    nats_messages_total: AtomicU64,
    nats_published_total: AtomicU64,
    nats_results_published_total: AtomicU64,
    mdp_published_total: AtomicU64,
    jobs_stored_total: AtomicU64,
    artifacts_stored_total: AtomicU64,
    artifact_requests_total: AtomicU64,
    learning_events_stored_total: AtomicU64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FabricationPlanRequest {
    request_id: Option<String>,
    objective: String,
    material: Option<MaterialSpec>,
    stock: Option<StockSpec>,
    tolerance_mm: Option<f64>,
    quantity: Option<u32>,
    machines: Option<Vec<MachineProfile>>,
    constraints: Option<FabricationConstraints>,
    parts: Option<Vec<RequestedPart>>,
    existing_instructions: Option<Vec<InstructionProgram>>,
    learning: Option<LearningHints>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct MaterialSpec {
    name: String,
    family: Option<String>,
    hardness: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct StockSpec {
    form: String,
    dimensions_mm: Option<Vec<f64>>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct MachineProfile {
    id: String,
    kind: String,
    controller: Option<String>,
    materials: Option<Vec<String>>,
    work_envelope_mm: Option<Vec<f64>>,
    axes: Option<u8>,
    operations: Option<Vec<String>>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FabricationConstraints {
    max_setups: Option<u32>,
    allow_human_intervention: Option<bool>,
    allow_multi_part_assembly: Option<bool>,
    require_dry_run: Option<bool>,
    preferred_methods: Option<Vec<String>>,
    preferred_assembly_strategy: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RequestedPart {
    id: String,
    description: String,
    material: Option<MaterialSpec>,
    preferred_method: Option<String>,
    tolerance_mm: Option<f64>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LearningHints {
    policy_hint: Option<String>,
    model_family: Option<String>,
    reward_weights: Option<BTreeMap<String, f64>>,
    observations: Option<Vec<String>>,
    prior_successes: Option<Vec<String>>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct InstructionProgram {
    id: Option<String>,
    machine_id: Option<String>,
    machine_kind: Option<String>,
    language: Option<String>,
    instructions: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct InstructionAnalysisRequest {
    request_id: Option<String>,
    programs: Vec<InstructionProgram>,
    machines: Option<Vec<MachineProfile>>,
    material: Option<MaterialSpec>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FabricationOutcomeRequest {
    request_id: Option<String>,
    source_job_id: Option<String>,
    source_artifact_id: Option<String>,
    part_id: Option<String>,
    program_id: Option<String>,
    machine_id: Option<String>,
    machine_kind: Option<String>,
    material: Option<MaterialSpec>,
    outcome: String,
    completed: Option<bool>,
    machine_failure: Option<bool>,
    scrap: Option<bool>,
    human_intervention_required: Option<bool>,
    intervention_minutes: Option<f64>,
    duration_minutes: Option<f64>,
    dimensional_error_mm: Option<f64>,
    surface_quality: Option<f64>,
    observations: Option<Vec<String>>,
    notes: Option<Vec<String>>,
    reward_weights: Option<BTreeMap<String, f64>>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct FabricationPlanResponse {
    ok: bool,
    job_id: String,
    request_id: String,
    schema_version: &'static str,
    objective: String,
    material: MaterialSpec,
    quantity: u32,
    design: DesignSummary,
    process_plan: Vec<ProcessStep>,
    generated_programs: Vec<GeneratedProgram>,
    validation: ValidationReport,
    simulation: SimulationReport,
    assembly: AssemblyPlan,
    learning: LearningPlan,
    warnings: Vec<String>,
    generated_at_ms: u128,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct InstructionAnalysisResponse {
    ok: bool,
    job_id: String,
    request_id: String,
    programs: Vec<AnalyzedProgram>,
    validation: ValidationReport,
    simulation: SimulationReport,
    improvements: Vec<InstructionImprovement>,
    improved_programs: Vec<ImprovedInstructionProgram>,
    generated_at_ms: u128,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct FabricationLearningResponse {
    ok: bool,
    job_id: String,
    request_id: String,
    source_job_id: Option<String>,
    source_artifact_id: Option<String>,
    outcome: String,
    state: String,
    recommended_action: String,
    reward: f64,
    reward_terms: Vec<LearningRewardTerm>,
    observations: Vec<String>,
    mdp_update: Value,
    neural_example: Value,
    warnings: Vec<String>,
    generated_at_ms: u128,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct LearningRewardTerm {
    name: String,
    value: f64,
    weight: f64,
    contribution: f64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct FabricationJobRecord {
    job_id: String,
    request_id: String,
    kind: String,
    status: String,
    ok: bool,
    severity: String,
    summary: String,
    artifact_count: usize,
    artifact_ids: Vec<String>,
    created_at_ms: u128,
    updated_at_ms: u128,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct FabricationArtifactSummary {
    artifact_id: String,
    kind: String,
    media_type: String,
    part_id: Option<String>,
    program_id: Option<String>,
    machine_kind: Option<String>,
    draft: bool,
    machine_ready: bool,
    line_count: Option<usize>,
    created_at_ms: u128,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct FabricationArtifact {
    artifact_id: String,
    kind: String,
    media_type: String,
    part_id: Option<String>,
    program_id: Option<String>,
    machine_kind: Option<String>,
    draft: bool,
    machine_ready: bool,
    line_count: Option<usize>,
    content: Value,
    notes: Vec<String>,
    created_at_ms: u128,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct StoredFabricationJob {
    record: FabricationJobRecord,
    plan: Option<FabricationPlanResponse>,
    analysis: Option<InstructionAnalysisResponse>,
    learning: Option<FabricationLearningResponse>,
    artifacts: BTreeMap<String, FabricationArtifact>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct FabricationJobDetail {
    record: FabricationJobRecord,
    plan: Option<FabricationPlanResponse>,
    analysis: Option<InstructionAnalysisResponse>,
    learning: Option<FabricationLearningResponse>,
    artifacts: Vec<FabricationArtifactSummary>,
}

#[derive(Default)]
struct FabricationJobStore {
    order: VecDeque<String>,
    jobs: BTreeMap<String, StoredFabricationJob>,
    max_jobs: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ImprovedInstructionProgram {
    program_id: String,
    machine_kind: String,
    language: String,
    changed: bool,
    machine_ready: bool,
    source_line_count: usize,
    instructions: Vec<String>,
    notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct DesignSummary {
    representation: String,
    object_id: String,
    parts: Vec<PartPlan>,
    join_strategy: String,
    manufacturability_notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct PartPlan {
    id: String,
    role: String,
    material: MaterialSpec,
    manufacturing_method: String,
    machine_kind: String,
    tolerance_mm: f64,
    interfaces: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProcessStep {
    step: u32,
    part_id: String,
    machine_id: String,
    machine_kind: String,
    operation: String,
    setup: String,
    expected_minutes: u32,
    requires_human_intervention: bool,
    notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct GeneratedProgram {
    program_id: String,
    part_id: String,
    machine_id: String,
    machine_kind: String,
    language: String,
    draft: bool,
    machine_ready: bool,
    instructions: Vec<String>,
    safety_notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ValidationReport {
    ok: bool,
    severity: String,
    findings: Vec<ValidationFinding>,
    failure_boundaries: Vec<FailureBoundary>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SimulationReport {
    ok: bool,
    severity: String,
    programs: Vec<SimulationProgramTrace>,
    findings: Vec<ValidationFinding>,
    failure_boundaries: Vec<FailureBoundary>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SimulationProgramTrace {
    program_id: String,
    machine_id: Option<String>,
    machine_kind: String,
    language: String,
    motion_line_count: usize,
    work_envelope_mm: Option<Vec<f64>>,
    axis_extents: Vec<SimulationAxisExtent>,
    safe_clearance_observed: bool,
    spindle_or_heatup_observed: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SimulationAxisExtent {
    axis: String,
    min_mm: f64,
    max_mm: f64,
    limit_mm: Option<f64>,
    exceeds_limit: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ValidationFinding {
    severity: String,
    code: String,
    program_id: Option<String>,
    line: Option<usize>,
    message: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct FailureBoundary {
    kind: String,
    severity: String,
    program_id: Option<String>,
    line: Option<usize>,
    reason: String,
    requires_human_intervention: bool,
    suggested_resolution: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct InstructionImprovement {
    program_id: Option<String>,
    line: Option<usize>,
    action: String,
    reason: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct AssemblyPlan {
    strategy: String,
    combine_candidates: Vec<String>,
    split_candidates: Vec<String>,
    joints: Vec<String>,
    notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct NeuralActionScore {
    action: String,
    score: f64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct NeuralPolicySketch {
    schema_version: String,
    model_family: String,
    feature_vector: Vec<f64>,
    hidden_activations: Vec<f64>,
    action_scores: Vec<NeuralActionScore>,
    notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct LearningPlan {
    model_family: String,
    mdp_states: Vec<String>,
    pomdp_observations: Vec<String>,
    actions: Vec<String>,
    reward_terms: Vec<String>,
    neural_features: Vec<String>,
    neural_policy: NeuralPolicySketch,
    training_examples: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LearningOutcomeRequest {
    request_id: Option<String>,
    job_id: Option<String>,
    objective: Option<String>,
    material: Option<MaterialSpec>,
    manufacturing_methods: Option<Vec<String>>,
    assembly_strategy: Option<String>,
    success: bool,
    reward: Option<f64>,
    observations: Option<Vec<String>>,
    notes: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct LearningOutcomeRecord {
    outcome_id: String,
    request_id: String,
    job_id: Option<String>,
    objective: Option<String>,
    material: Option<MaterialSpec>,
    manufacturing_methods: Vec<String>,
    assembly_strategy: Option<String>,
    success: bool,
    reward: f64,
    observations: Vec<String>,
    notes: Vec<String>,
    created_at_ms: u128,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct LearningPreference {
    key: String,
    samples: u64,
    successes: u64,
    failures: u64,
    average_reward: f64,
    recommendation: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct LearningPolicySnapshot {
    outcome_count: usize,
    successes: u64,
    failures: u64,
    average_reward: f64,
    method_preferences: Vec<LearningPreference>,
    method_combination_preferences: Vec<LearningPreference>,
    assembly_preferences: Vec<LearningPreference>,
    neural_training_examples: Vec<String>,
}

#[derive(Default)]
struct LearningMemory {
    outcomes: VecDeque<LearningOutcomeRecord>,
    max_outcomes: usize,
}

#[derive(Default)]
struct LearningAggregate {
    samples: u64,
    successes: u64,
    reward_sum: f64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct AnalyzedProgram {
    program_id: String,
    machine_kind: String,
    language: String,
    line_count: usize,
    has_units_mode: bool,
    has_positioning_mode: bool,
    has_homing_or_fixture_reference: bool,
    has_spindle_or_heatup: bool,
    has_program_end: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MachineClass {
    Additive,
    Mill,
    Lathe,
    Router,
    SheetCut,
    Other,
}

#[derive(Default)]
struct TextInstructionSignals {
    has_setup_reference: bool,
    has_process_preparation: bool,
    has_completion_marker: bool,
}

fn env_value(key: &str, fallback: &str) -> String {
    env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| fallback.to_string())
}

fn env_bool(key: &str, fallback: bool) -> bool {
    env::var(key)
        .ok()
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(fallback)
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn request_id(input: Option<&String>, prefix: &str) -> String {
    input
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .unwrap_or(prefix)
        .chars()
        .take(MAX_REQUEST_ID_LEN)
        .collect()
}

fn validate_text(value: &str, label: &str, max_len: usize) -> Result<String, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(format!("{label} must not be empty"));
    }
    if trimmed.len() > max_len {
        return Err(format!("{label} must be at most {max_len} bytes"));
    }
    if trimmed
        .chars()
        .any(|character| character.is_control() && character != '\n')
    {
        return Err(format!("{label} must not contain control characters"));
    }
    Ok(trimmed.to_string())
}

fn validate_label(value: &str, label: &str) -> Result<String, String> {
    let trimmed = validate_text(value, label, MAX_LABEL_LEN)?;
    if !trimmed
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.'))
    {
        return Err(format!(
            "{label} may contain only ASCII letters, numbers, '-', '_', or '.'"
        ));
    }
    Ok(trimmed)
}

fn finite_positive(value: f64, label: &str) -> Result<f64, String> {
    if !value.is_finite() || value <= 0.0 {
        return Err(format!("{label} must be finite and positive"));
    }
    Ok(value)
}

fn finite_non_negative(value: f64, label: &str) -> Result<f64, String> {
    if !value.is_finite() || value < 0.0 {
        return Err(format!("{label} must be finite and non-negative"));
    }
    Ok(value)
}

fn finite_ratio(value: f64, label: &str) -> Result<f64, String> {
    if !value.is_finite() || !(0.0..=1.0).contains(&value) {
        return Err(format!("{label} must be finite and in [0, 1]"));
    }
    Ok(value)
}

fn bounded(value: f64, min: f64, max: f64) -> f64 {
    value.max(min).min(max)
}

fn validate_optional_label(value: Option<String>, label: &str) -> Result<Option<String>, String> {
    value.map(|value| validate_label(&value, label)).transpose()
}

fn validate_optional_text(
    value: Option<String>,
    label: &str,
    max_len: usize,
) -> Result<Option<String>, String> {
    value
        .map(|value| validate_text(&value, label, max_len))
        .transpose()
}

fn validate_signal_list(
    values: Option<Vec<String>>,
    label: &str,
    max_len: usize,
) -> Result<Vec<String>, String> {
    let values = values.unwrap_or_default();
    if values.len() > MAX_LEARNING_SIGNALS {
        return Err(format!(
            "{label} must contain at most {MAX_LEARNING_SIGNALS} entries"
        ));
    }
    values
        .into_iter()
        .enumerate()
        .map(|(index, value)| validate_text(&value, &format!("{label}[{index}]"), max_len))
        .collect()
}

fn stock_envelope_excesses(
    stock_dimensions: &[f64],
    work_envelope: &[f64],
) -> Vec<(usize, f64, f64)> {
    stock_dimensions
        .iter()
        .zip(work_envelope.iter())
        .enumerate()
        .filter_map(|(index, (stock, limit))| {
            if stock > limit {
                Some((index, *stock, *limit))
            } else {
                None
            }
        })
        .collect()
}

fn normalize_token(value: &str) -> String {
    value
        .trim()
        .to_ascii_lowercase()
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character
            } else {
                '-'
            }
        })
        .collect::<String>()
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

fn safe_job_id(kind: &str, request_id: &str, generated_at_ms: u128) -> String {
    let request = normalize_token(request_id);
    let request = if request.is_empty() {
        "request".to_string()
    } else {
        request
    };
    format!("{}-{}-{}", normalize_token(kind), request, generated_at_ms)
        .chars()
        .take(180)
        .collect()
}

fn summary_text(value: &str) -> String {
    let text = value.trim();
    if text.chars().count() <= 240 {
        text.to_string()
    } else {
        format!("{}...", text.chars().take(237).collect::<String>())
    }
}

impl FabricationArtifact {
    fn summary(&self) -> FabricationArtifactSummary {
        FabricationArtifactSummary {
            artifact_id: self.artifact_id.clone(),
            kind: self.kind.clone(),
            media_type: self.media_type.clone(),
            part_id: self.part_id.clone(),
            program_id: self.program_id.clone(),
            machine_kind: self.machine_kind.clone(),
            draft: self.draft,
            machine_ready: self.machine_ready,
            line_count: self.line_count,
            created_at_ms: self.created_at_ms,
        }
    }
}

impl FabricationJobStore {
    fn new(max_jobs: usize) -> Self {
        Self {
            order: VecDeque::new(),
            jobs: BTreeMap::new(),
            max_jobs: max_jobs.max(1),
        }
    }

    fn insert(&mut self, job: StoredFabricationJob) {
        let job_id = job.record.job_id.clone();
        self.order.retain(|existing| existing != &job_id);
        self.order.push_back(job_id.clone());
        self.jobs.insert(job_id, job);
        while self.order.len() > self.max_jobs {
            if let Some(oldest) = self.order.pop_front() {
                self.jobs.remove(&oldest);
            }
        }
    }

    fn list(&self) -> Vec<FabricationJobRecord> {
        self.order
            .iter()
            .rev()
            .filter_map(|job_id| self.jobs.get(job_id))
            .map(|job| job.record.clone())
            .collect()
    }

    fn detail(&self, job_id: &str) -> Option<FabricationJobDetail> {
        self.jobs.get(job_id).map(|job| FabricationJobDetail {
            record: job.record.clone(),
            plan: job.plan.clone(),
            analysis: job.analysis.clone(),
            learning: job.learning.clone(),
            artifacts: job
                .artifacts
                .values()
                .map(FabricationArtifact::summary)
                .collect(),
        })
    }

    fn artifact(&self, job_id: &str, artifact_id: &str) -> Option<FabricationArtifact> {
        self.jobs
            .get(job_id)
            .and_then(|job| job.artifacts.get(artifact_id))
            .cloned()
    }

    fn counts(&self) -> (usize, usize) {
        let artifact_count = self.jobs.values().map(|job| job.artifacts.len()).sum();
        (self.jobs.len(), artifact_count)
    }
}

impl LearningAggregate {
    fn add(&mut self, outcome: &LearningOutcomeRecord) {
        self.samples += 1;
        if outcome.success {
            self.successes += 1;
        }
        self.reward_sum += outcome.reward;
    }

    fn preference(&self, key: String) -> LearningPreference {
        let failures = self.samples.saturating_sub(self.successes);
        let average_reward = if self.samples == 0 {
            0.0
        } else {
            self.reward_sum / self.samples as f64
        };
        let success_rate = if self.samples == 0 {
            0.0
        } else {
            self.successes as f64 / self.samples as f64
        };
        let recommendation = if self.samples < 2 {
            "explore".to_string()
        } else if success_rate >= 0.66 && average_reward >= 0.0 {
            "prefer".to_string()
        } else if success_rate < 0.4 || average_reward < 0.0 {
            "review-or-avoid".to_string()
        } else {
            "keep-exploring".to_string()
        };
        LearningPreference {
            key,
            samples: self.samples,
            successes: self.successes,
            failures,
            average_reward,
            recommendation,
        }
    }
}

impl LearningMemory {
    fn new(max_outcomes: usize) -> Self {
        Self {
            outcomes: VecDeque::new(),
            max_outcomes: max_outcomes.max(1),
        }
    }

    fn insert(&mut self, outcome: LearningOutcomeRecord) {
        self.outcomes.push_back(outcome);
        while self.outcomes.len() > self.max_outcomes {
            self.outcomes.pop_front();
        }
    }

    fn count(&self) -> usize {
        self.outcomes.len()
    }

    fn snapshot(&self) -> LearningPolicySnapshot {
        let mut methods = BTreeMap::<String, LearningAggregate>::new();
        let mut method_combinations = BTreeMap::<String, LearningAggregate>::new();
        let mut assemblies = BTreeMap::<String, LearningAggregate>::new();
        let mut successes = 0_u64;
        let mut reward_sum = 0.0;

        for outcome in &self.outcomes {
            if outcome.success {
                successes += 1;
            }
            reward_sum += outcome.reward;
            for method in &outcome.manufacturing_methods {
                methods.entry(method.clone()).or_default().add(outcome);
            }
            if let Some(combination) = method_combination_key(&outcome.manufacturing_methods) {
                method_combinations
                    .entry(combination)
                    .or_default()
                    .add(outcome);
            }
            if let Some(strategy) = outcome.assembly_strategy.as_ref() {
                assemblies.entry(strategy.clone()).or_default().add(outcome);
            }
        }

        let outcome_count = self.outcomes.len();
        let failures = outcome_count as u64 - successes;
        let average_reward = if outcome_count == 0 {
            0.0
        } else {
            reward_sum / outcome_count as f64
        };
        let mut method_preferences = methods
            .into_iter()
            .map(|(key, aggregate)| aggregate.preference(key))
            .collect::<Vec<_>>();
        method_preferences.sort_by(|left, right| {
            right
                .average_reward
                .partial_cmp(&left.average_reward)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| right.samples.cmp(&left.samples))
                .then_with(|| left.key.cmp(&right.key))
        });
        let mut method_combination_preferences = method_combinations
            .into_iter()
            .map(|(key, aggregate)| aggregate.preference(key))
            .collect::<Vec<_>>();
        method_combination_preferences.sort_by(|left, right| {
            right
                .average_reward
                .partial_cmp(&left.average_reward)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| right.samples.cmp(&left.samples))
                .then_with(|| left.key.cmp(&right.key))
        });
        let mut assembly_preferences = assemblies
            .into_iter()
            .map(|(key, aggregate)| aggregate.preference(key))
            .collect::<Vec<_>>();
        assembly_preferences.sort_by(|left, right| {
            right
                .average_reward
                .partial_cmp(&left.average_reward)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| right.samples.cmp(&left.samples))
                .then_with(|| left.key.cmp(&right.key))
        });
        let neural_training_examples = self
            .outcomes
            .iter()
            .rev()
            .take(32)
            .map(|outcome| {
                format!(
                    "job={} success={} reward={:.3} methods={} assembly={} observations={}",
                    outcome.job_id.as_deref().unwrap_or("none"),
                    outcome.success,
                    outcome.reward,
                    outcome.manufacturing_methods.join("+"),
                    outcome.assembly_strategy.as_deref().unwrap_or("none"),
                    outcome.observations.join("|")
                )
            })
            .collect::<Vec<_>>();

        LearningPolicySnapshot {
            outcome_count,
            successes,
            failures,
            average_reward,
            method_preferences,
            method_combination_preferences,
            assembly_preferences,
            neural_training_examples,
        }
    }
}

fn material_or_default(material: Option<MaterialSpec>) -> Result<MaterialSpec, String> {
    let material = material.unwrap_or(MaterialSpec {
        name: "pla".to_string(),
        family: Some("polymer".to_string()),
        hardness: None,
    });
    Ok(MaterialSpec {
        name: validate_text(&material.name, "material.name", MAX_LABEL_LEN)?,
        family: material
            .family
            .map(|family| validate_text(&family, "material.family", MAX_LABEL_LEN))
            .transpose()?,
        hardness: material
            .hardness
            .map(|hardness| validate_text(&hardness, "material.hardness", MAX_LABEL_LEN))
            .transpose()?,
    })
}

fn is_metal(material: &MaterialSpec) -> bool {
    let name = normalize_token(&material.name);
    let family = material.family.as_deref().map(normalize_token);
    family.as_deref() == Some("metal")
        || matches!(
            name.as_str(),
            "aluminum"
                | "aluminium"
                | "steel"
                | "stainless-steel"
                | "brass"
                | "bronze"
                | "titanium"
                | "copper"
        )
}

fn is_polymer(material: &MaterialSpec) -> bool {
    let name = normalize_token(&material.name);
    let family = material.family.as_deref().map(normalize_token);
    family.as_deref() == Some("polymer")
        || matches!(
            name.as_str(),
            "pla" | "petg" | "abs" | "nylon" | "resin" | "asa" | "pc"
        )
}

fn is_router_material(material: &MaterialSpec) -> bool {
    let name = normalize_token(&material.name);
    let family = material.family.as_deref().map(normalize_token);
    family
        .as_deref()
        .is_some_and(|family| matches!(family, "wood" | "foam" | "plastic" | "polymer"))
        || matches!(
            name.as_str(),
            "wood" | "plywood" | "mdf" | "acrylic" | "foam" | "hdpe" | "polycarbonate"
        )
}

fn wants_resin_printing(value: &str) -> bool {
    let token = normalize_token(value);
    token.contains("resin")
        || token.contains("sla")
        || token.contains("msla")
        || token.contains("dlp")
        || token.contains("photopolymer")
}

fn wants_powder_bed_printing(value: &str) -> bool {
    let token = normalize_token(value);
    token.contains("sls")
        || token.contains("mjf")
        || token.contains("powder")
        || token.contains("pa12")
        || token.contains("nylon")
}

fn wants_sheet_cutting(value: &str) -> bool {
    let token = normalize_token(value);
    token.contains("laser")
        || token.contains("waterjet")
        || token.contains("water-jet")
        || token.contains("plasma")
        || token.contains("sheet-cut")
        || token.contains("sheet-cutter")
        || token.contains("knife-cut")
        || token.contains("die-cut")
        || token.contains("kerf")
        || token.contains("stencil")
        || token.contains("gasket")
}

fn wants_laser_cutting(value: &str) -> bool {
    normalize_token(value).contains("laser")
}

fn wants_waterjet_cutting(value: &str) -> bool {
    let token = normalize_token(value);
    token.contains("waterjet") || token.contains("water-jet")
}

fn wants_plasma_cutting(value: &str) -> bool {
    normalize_token(value).contains("plasma")
}

fn is_horizontal_mill_kind(kind: &str) -> bool {
    let token = normalize_token(kind);
    token.contains("horizontal-mill")
        || token.contains("horizontal-machining")
        || (token.contains("horizontal") && token.contains("mill"))
}

fn is_resin_printer_kind(kind: &str) -> bool {
    wants_resin_printing(kind)
}

fn is_powder_bed_printer_kind(kind: &str) -> bool {
    wants_powder_bed_printing(kind)
}

fn is_sheet_cutter_kind(kind: &str) -> bool {
    wants_sheet_cutting(kind)
}

fn is_laser_cutter_kind(kind: &str) -> bool {
    wants_laser_cutting(kind)
}

fn is_waterjet_cutter_kind(kind: &str) -> bool {
    wants_waterjet_cutting(kind)
}

fn is_plasma_cutter_kind(kind: &str) -> bool {
    wants_plasma_cutting(kind)
}

fn wants_horizontal_milling(value: &str) -> bool {
    let token = normalize_token(value);
    token.contains("horizontal")
        || token.contains("side-mill")
        || token.contains("side-milling")
        || token.contains("keyway")
        || token.contains("slot")
        || token.contains("slitting")
}

fn machine_class(kind: &str) -> MachineClass {
    let token = normalize_token(kind);
    if token.contains("printer")
        || token.contains("fdm")
        || token.contains("sla")
        || token.contains("sls")
        || token.contains("additive")
    {
        MachineClass::Additive
    } else if token.contains("lathe") || token.contains("turn") {
        MachineClass::Lathe
    } else if token.contains("mill") || token.contains("machining-center") {
        MachineClass::Mill
    } else if token.contains("router") {
        MachineClass::Router
    } else if is_sheet_cutter_kind(&token) {
        MachineClass::SheetCut
    } else {
        MachineClass::Other
    }
}

fn default_machines() -> Vec<MachineProfile> {
    vec![
        MachineProfile {
            id: "fdm-printer-1".to_string(),
            kind: "fdm-printer".to_string(),
            controller: Some("marlin".to_string()),
            materials: Some(vec![
                "pla".to_string(),
                "petg".to_string(),
                "abs".to_string(),
                "nylon".to_string(),
            ]),
            work_envelope_mm: Some(vec![220.0, 220.0, 250.0]),
            axes: Some(3),
            operations: Some(vec!["additive-print".to_string()]),
        },
        MachineProfile {
            id: "sla-printer-1".to_string(),
            kind: "sla-printer".to_string(),
            controller: Some("sla-job".to_string()),
            materials: Some(vec![
                "resin".to_string(),
                "photopolymer".to_string(),
                "polymer".to_string(),
            ]),
            work_envelope_mm: Some(vec![145.0, 145.0, 175.0]),
            axes: Some(3),
            operations: Some(vec![
                "resin-print".to_string(),
                "wash".to_string(),
                "uv-cure".to_string(),
                "support-removal".to_string(),
            ]),
        },
        MachineProfile {
            id: "sls-printer-1".to_string(),
            kind: "sls-printer".to_string(),
            controller: Some("sls-job".to_string()),
            materials: Some(vec![
                "nylon".to_string(),
                "pa12".to_string(),
                "polymer".to_string(),
                "powder".to_string(),
            ]),
            work_envelope_mm: Some(vec![300.0, 300.0, 300.0]),
            axes: Some(3),
            operations: Some(vec![
                "powder-bed-print".to_string(),
                "cooldown".to_string(),
                "depowder".to_string(),
                "bead-blast".to_string(),
            ]),
        },
        MachineProfile {
            id: "vertical-mill-1".to_string(),
            kind: "vertical-mill".to_string(),
            controller: Some("haas-gcode".to_string()),
            materials: Some(vec![
                "aluminum".to_string(),
                "steel".to_string(),
                "brass".to_string(),
                "plastic".to_string(),
            ]),
            work_envelope_mm: Some(vec![500.0, 300.0, 300.0]),
            axes: Some(3),
            operations: Some(vec![
                "face".to_string(),
                "pocket".to_string(),
                "drill".to_string(),
                "contour".to_string(),
            ]),
        },
        MachineProfile {
            id: "lathe-1".to_string(),
            kind: "lathe".to_string(),
            controller: Some("fanuc-gcode".to_string()),
            materials: Some(vec![
                "aluminum".to_string(),
                "steel".to_string(),
                "brass".to_string(),
                "plastic".to_string(),
            ]),
            work_envelope_mm: Some(vec![300.0, 750.0]),
            axes: Some(2),
            operations: Some(vec![
                "turn".to_string(),
                "face".to_string(),
                "bore".to_string(),
                "thread".to_string(),
            ]),
        },
        MachineProfile {
            id: "horizontal-mill-1".to_string(),
            kind: "horizontal-mill".to_string(),
            controller: Some("iso-gcode".to_string()),
            materials: Some(vec![
                "aluminum".to_string(),
                "steel".to_string(),
                "brass".to_string(),
            ]),
            work_envelope_mm: Some(vec![600.0, 400.0, 400.0]),
            axes: Some(4),
            operations: Some(vec![
                "slot".to_string(),
                "heavy-roughing".to_string(),
                "side-mill".to_string(),
            ]),
        },
        MachineProfile {
            id: "cnc-router-1".to_string(),
            kind: "cnc-router".to_string(),
            controller: Some("grbl-gcode".to_string()),
            materials: Some(vec![
                "wood".to_string(),
                "plywood".to_string(),
                "mdf".to_string(),
                "acrylic".to_string(),
                "plastic".to_string(),
                "foam".to_string(),
                "aluminum".to_string(),
            ]),
            work_envelope_mm: Some(vec![1200.0, 800.0, 100.0]),
            axes: Some(3),
            operations: Some(vec![
                "profile".to_string(),
                "pocket".to_string(),
                "engrave".to_string(),
                "tab-cut".to_string(),
            ]),
        },
        MachineProfile {
            id: "laser-cutter-1".to_string(),
            kind: "laser-cutter".to_string(),
            controller: Some("laser-job".to_string()),
            materials: Some(vec![
                "acrylic".to_string(),
                "plywood".to_string(),
                "mdf".to_string(),
                "cardboard".to_string(),
                "paper".to_string(),
                "leather".to_string(),
                "plastic".to_string(),
            ]),
            work_envelope_mm: Some(vec![900.0, 600.0, 12.0]),
            axes: Some(3),
            operations: Some(vec![
                "laser-cut".to_string(),
                "laser-engrave".to_string(),
                "pierce".to_string(),
                "kerf-test".to_string(),
            ]),
        },
        MachineProfile {
            id: "waterjet-cutter-1".to_string(),
            kind: "waterjet-cutter".to_string(),
            controller: Some("waterjet-job".to_string()),
            materials: Some(vec![
                "metal".to_string(),
                "aluminum".to_string(),
                "steel".to_string(),
                "stainless-steel".to_string(),
                "brass".to_string(),
                "titanium".to_string(),
                "copper".to_string(),
                "stone".to_string(),
                "glass".to_string(),
                "plastic".to_string(),
            ]),
            work_envelope_mm: Some(vec![1500.0, 1000.0, 75.0]),
            axes: Some(3),
            operations: Some(vec![
                "waterjet-cut".to_string(),
                "abrasive-pierce".to_string(),
                "kerf-test".to_string(),
                "tab-cut".to_string(),
            ]),
        },
        MachineProfile {
            id: "plasma-cutter-1".to_string(),
            kind: "plasma-cutter".to_string(),
            controller: Some("plasma-job".to_string()),
            materials: Some(vec![
                "metal".to_string(),
                "steel".to_string(),
                "stainless-steel".to_string(),
                "aluminum".to_string(),
            ]),
            work_envelope_mm: Some(vec![1250.0, 1250.0, 25.0]),
            axes: Some(3),
            operations: Some(vec![
                "plasma-cut".to_string(),
                "arc-start".to_string(),
                "pierce".to_string(),
                "kerf-test".to_string(),
            ]),
        },
    ]
}

fn validate_machines(input: Option<Vec<MachineProfile>>) -> Result<Vec<MachineProfile>, String> {
    let machines = input.unwrap_or_else(default_machines);
    if machines.is_empty() {
        return Err("machines must not be empty".to_string());
    }
    if machines.len() > MAX_MACHINES {
        return Err(format!(
            "machines must contain at most {MAX_MACHINES} entries"
        ));
    }

    let mut seen = BTreeSet::new();
    let mut validated = Vec::with_capacity(machines.len());
    for machine in machines {
        let id = validate_label(&machine.id, "machine.id")?;
        if !seen.insert(id.clone()) {
            return Err(format!("machines must have unique ids; duplicate {id}"));
        }
        let kind = validate_text(&machine.kind, "machine.kind", MAX_LABEL_LEN)?;
        let controller = machine
            .controller
            .map(|value| validate_text(&value, "machine.controller", MAX_LABEL_LEN))
            .transpose()?;
        let materials = machine
            .materials
            .map(|materials| {
                materials
                    .iter()
                    .map(|value| validate_text(value, "machine.materials", MAX_LABEL_LEN))
                    .collect::<Result<Vec<_>, _>>()
            })
            .transpose()?;
        let work_envelope_mm = machine
            .work_envelope_mm
            .map(|values| {
                if values.is_empty() || values.len() > 4 {
                    return Err("machine.workEnvelopeMm must have 1 to 4 values".to_string());
                }
                values
                    .iter()
                    .map(|value| finite_positive(*value, "machine.workEnvelopeMm"))
                    .collect::<Result<Vec<_>, _>>()
            })
            .transpose()?;
        let operations = machine
            .operations
            .map(|operations| {
                operations
                    .iter()
                    .map(|value| validate_text(value, "machine.operations", MAX_LABEL_LEN))
                    .collect::<Result<Vec<_>, _>>()
            })
            .transpose()?;

        validated.push(MachineProfile {
            id,
            kind,
            controller,
            materials,
            work_envelope_mm,
            axes: machine.axes,
            operations,
        });
    }
    Ok(validated)
}

fn validate_request_parts(
    input: Option<Vec<RequestedPart>>,
    objective: &str,
    material: &MaterialSpec,
    tolerance_mm: f64,
) -> Result<Vec<RequestedPart>, String> {
    let parts = input.unwrap_or_else(|| infer_requested_parts(objective, material, tolerance_mm));
    if parts.is_empty() {
        return Err("parts must not be empty".to_string());
    }
    if parts.len() > MAX_PARTS {
        return Err(format!("parts must contain at most {MAX_PARTS} entries"));
    }
    let mut seen = BTreeSet::new();
    let mut validated = Vec::with_capacity(parts.len());
    for part in parts {
        let id = validate_label(&part.id, "part.id")?;
        if !seen.insert(id.clone()) {
            return Err(format!("parts must have unique ids; duplicate {id}"));
        }
        validated.push(RequestedPart {
            id,
            description: validate_text(&part.description, "part.description", MAX_TEXT_LEN)?,
            material: part
                .material
                .map(|material| material_or_default(Some(material)))
                .transpose()?,
            preferred_method: part
                .preferred_method
                .map(|method| validate_text(&method, "part.preferredMethod", MAX_LABEL_LEN))
                .transpose()?,
            tolerance_mm: part
                .tolerance_mm
                .map(|value| finite_positive(value, "part.toleranceMm"))
                .transpose()?,
        });
    }
    Ok(validated)
}

fn infer_requested_parts(
    objective: &str,
    material: &MaterialSpec,
    tolerance_mm: f64,
) -> Vec<RequestedPart> {
    let objective_token = normalize_token(objective);
    let mut parts = Vec::new();

    let wants_resin_part =
        wants_resin_printing(&objective_token) || wants_resin_printing(&material.name);
    let wants_powder_bed_part =
        wants_powder_bed_printing(&objective_token) || wants_powder_bed_printing(&material.name);
    let needs_turned_part = objective_token.contains("shaft")
        || objective_token.contains("bushing")
        || objective_token.contains("bearing")
        || objective_token.contains("cylind")
        || objective_token.contains("thread");
    let needs_horizontal_milled_part = wants_horizontal_milling(&objective_token);
    let needs_milled_part = objective_token.contains("bracket")
        || objective_token.contains("plate")
        || objective_token.contains("pocket")
        || objective_token.contains("housing")
        || objective_token.contains("fixture")
        || objective_token.contains("datum")
        || needs_horizontal_milled_part
        || tolerance_mm <= 0.08
        || is_metal(material);
    let needs_sheet_cut_part = wants_sheet_cutting(&objective_token);
    let needs_routed_part = !wants_resin_part
        && !wants_powder_bed_part
        && !needs_sheet_cut_part
        && (objective_token.contains("router")
            || objective_token.contains("routed")
            || objective_token.contains("sign")
            || objective_token.contains("panel")
            || objective_token.contains("sheet")
            || objective_token.contains("profile")
            || objective_token.contains("engrave")
            || objective_token.contains("tabbed")
            || is_router_material(material));
    let needs_printed_part = wants_resin_part
        || wants_powder_bed_part
        || objective_token.contains("prototype")
        || objective_token.contains("case")
        || objective_token.contains("cover")
        || objective_token.contains("organic")
        || objective_token.contains("ergonomic")
        || (is_polymer(material) && !needs_routed_part && !needs_sheet_cut_part)
        || (!needs_milled_part && !needs_routed_part && !needs_sheet_cut_part);

    if needs_printed_part {
        let preferred_method = if wants_resin_part {
            "resin-print"
        } else if wants_powder_bed_part {
            "powder-bed-print"
        } else {
            "additive-print"
        };
        parts.push(RequestedPart {
            id: "printed-body".to_string(),
            description: "additive body or prototype shell inferred from objective".to_string(),
            material: Some(material.clone()),
            preferred_method: Some(preferred_method.to_string()),
            tolerance_mm: Some(tolerance_mm.max(0.15)),
        });
    }
    if needs_routed_part {
        parts.push(RequestedPart {
            id: "routed-sheet-profile".to_string(),
            description:
                "routed sheet, sign, profile, engraving, or tabbed panel inferred from objective"
                    .to_string(),
            material: Some(material.clone()),
            preferred_method: Some("routing".to_string()),
            tolerance_mm: Some(tolerance_mm.max(0.12)),
        });
    }
    if needs_sheet_cut_part {
        let preferred_method =
            if objective_token.contains("waterjet") || objective_token.contains("water-jet") {
                "waterjet-cutting"
            } else if objective_token.contains("plasma") {
                "plasma-cutting"
            } else {
                "laser-cutting"
            };
        parts.push(RequestedPart {
            id: "sheet-cut-profile".to_string(),
            description: "laser, waterjet, plasma, knife, stencil, gasket, or kerf-controlled sheet profile inferred from objective"
                .to_string(),
            material: Some(material.clone()),
            preferred_method: Some(preferred_method.to_string()),
            tolerance_mm: Some(tolerance_mm.max(0.10)),
        });
    }
    if needs_horizontal_milled_part {
        parts.push(RequestedPart {
            id: "horizontal-slotted-feature".to_string(),
            description: "horizontal-milled side slot, keyway, spline, or heavy side feature"
                .to_string(),
            material: Some(material.clone()),
            preferred_method: Some("horizontal-milling".to_string()),
            tolerance_mm: Some(tolerance_mm),
        });
    } else if needs_milled_part {
        parts.push(RequestedPart {
            id: "milled-datum".to_string(),
            description: "machined datum, pocket, flat, or tight-tolerance feature".to_string(),
            material: Some(material.clone()),
            preferred_method: Some("milling".to_string()),
            tolerance_mm: Some(tolerance_mm),
        });
    }
    if needs_turned_part {
        parts.push(RequestedPart {
            id: "turned-axisymmetric-insert".to_string(),
            description: "turned shaft, bushing, bearing, thread, or cylindrical insert"
                .to_string(),
            material: Some(material.clone()),
            preferred_method: Some("turning".to_string()),
            tolerance_mm: Some(tolerance_mm),
        });
    }

    if parts.is_empty() {
        parts.push(RequestedPart {
            id: "primary-part".to_string(),
            description: "primary fabricated part inferred from objective".to_string(),
            material: Some(material.clone()),
            preferred_method: None,
            tolerance_mm: Some(tolerance_mm),
        });
    }
    parts
}

fn material_supported(machine: &MachineProfile, material: &MaterialSpec) -> bool {
    machine
        .materials
        .as_ref()
        .map(|materials| {
            let material_name = normalize_token(&material.name);
            let material_family = material.family.as_deref().map(normalize_token);
            materials.iter().any(|candidate| {
                let token = normalize_token(candidate);
                token == material_name || material_family.as_ref() == Some(&token)
            })
        })
        .unwrap_or(true)
}

fn choose_machine<'a>(
    part: &RequestedPart,
    machines: &'a [MachineProfile],
    material: &MaterialSpec,
    constraints: Option<&FabricationConstraints>,
) -> &'a MachineProfile {
    let preferred = part.preferred_method.as_deref().map(normalize_token);
    let preferred_methods = constraints
        .and_then(|constraints| constraints.preferred_methods.as_ref())
        .map(|methods| {
            methods
                .iter()
                .map(|value| normalize_token(value))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let wants_horizontal_mill = preferred.as_deref().is_some_and(wants_horizontal_milling)
        || preferred_methods
            .iter()
            .any(|value| wants_horizontal_milling(value));
    let wants_resin_printer = preferred.as_deref().is_some_and(wants_resin_printing)
        || preferred_methods
            .iter()
            .any(|value| wants_resin_printing(value));
    let wants_powder_bed_printer = preferred.as_deref().is_some_and(wants_powder_bed_printing)
        || preferred_methods
            .iter()
            .any(|value| wants_powder_bed_printing(value));
    let wants_laser_cutter = preferred.as_deref().is_some_and(wants_laser_cutting)
        || preferred_methods
            .iter()
            .any(|value| wants_laser_cutting(value));
    let wants_waterjet_cutter = preferred.as_deref().is_some_and(wants_waterjet_cutting)
        || preferred_methods
            .iter()
            .any(|value| wants_waterjet_cutting(value));
    let wants_plasma_cutter = preferred.as_deref().is_some_and(wants_plasma_cutting)
        || preferred_methods
            .iter()
            .any(|value| wants_plasma_cutting(value));
    let wants_sheet_cutter = preferred.as_deref().is_some_and(wants_sheet_cutting)
        || preferred_methods
            .iter()
            .any(|value| wants_sheet_cutting(value));

    if wants_horizontal_mill {
        if let Some(machine) = machines.iter().find(|machine| {
            is_horizontal_mill_kind(&machine.kind) && material_supported(machine, material)
        }) {
            return machine;
        }
    }
    if wants_resin_printer {
        if let Some(machine) = machines.iter().find(|machine| {
            is_resin_printer_kind(&machine.kind) && material_supported(machine, material)
        }) {
            return machine;
        }
    }
    if wants_powder_bed_printer {
        if let Some(machine) = machines.iter().find(|machine| {
            is_powder_bed_printer_kind(&machine.kind) && material_supported(machine, material)
        }) {
            return machine;
        }
    }
    if wants_waterjet_cutter {
        if let Some(machine) = machines.iter().find(|machine| {
            is_waterjet_cutter_kind(&machine.kind) && material_supported(machine, material)
        }) {
            return machine;
        }
    }
    if wants_plasma_cutter {
        if let Some(machine) = machines.iter().find(|machine| {
            is_plasma_cutter_kind(&machine.kind) && material_supported(machine, material)
        }) {
            return machine;
        }
    }
    if wants_laser_cutter {
        if let Some(machine) = machines.iter().find(|machine| {
            is_laser_cutter_kind(&machine.kind) && material_supported(machine, material)
        }) {
            return machine;
        }
    }
    if wants_sheet_cutter {
        if let Some(machine) = machines.iter().find(|machine| {
            is_sheet_cutter_kind(&machine.kind) && material_supported(machine, material)
        }) {
            return machine;
        }
    }

    let wants_class = if preferred
        .as_deref()
        .is_some_and(|value| value.contains("turn") || value.contains("lathe"))
    {
        Some(MachineClass::Lathe)
    } else if preferred.as_deref().is_some_and(|value| {
        value.contains("router") || value.contains("routing") || value.contains("rout")
    }) {
        Some(MachineClass::Router)
    } else if preferred.as_deref().is_some_and(wants_sheet_cutting) {
        Some(MachineClass::SheetCut)
    } else if preferred
        .as_deref()
        .is_some_and(|value| value.contains("mill") || value.contains("machin"))
    {
        Some(MachineClass::Mill)
    } else if preferred
        .as_deref()
        .is_some_and(|value| value.contains("print") || value.contains("additive"))
    {
        Some(MachineClass::Additive)
    } else if preferred_methods
        .iter()
        .any(|value| value.contains("turn") || value.contains("lathe"))
    {
        Some(MachineClass::Lathe)
    } else if preferred_methods.iter().any(|value| {
        value.contains("router") || value.contains("routing") || value.contains("rout")
    }) {
        Some(MachineClass::Router)
    } else if preferred_methods
        .iter()
        .any(|value| wants_sheet_cutting(value))
    {
        Some(MachineClass::SheetCut)
    } else if preferred_methods
        .iter()
        .any(|value| value.contains("mill") || value.contains("machin"))
    {
        Some(MachineClass::Mill)
    } else if preferred_methods
        .iter()
        .any(|value| value.contains("print") || value.contains("additive"))
    {
        Some(MachineClass::Additive)
    } else {
        None
    };

    if let Some(class) = wants_class {
        if let Some(machine) = machines.iter().find(|machine| {
            machine_class(&machine.kind) == class && material_supported(machine, material)
        }) {
            return machine;
        }
    }

    if part.tolerance_mm.unwrap_or(DEFAULT_TOLERANCE_MM) <= 0.08 || is_metal(material) {
        if let Some(machine) = machines.iter().find(|machine| {
            matches!(
                machine_class(&machine.kind),
                MachineClass::Mill | MachineClass::Lathe
            ) && material_supported(machine, material)
        }) {
            return machine;
        }
    }

    machines
        .iter()
        .find(|machine| {
            machine_class(&machine.kind) == MachineClass::Additive
                && material_supported(machine, material)
        })
        .or_else(|| {
            machines
                .iter()
                .find(|machine| material_supported(machine, material))
        })
        .unwrap_or(&machines[0])
}

fn part_method(class: MachineClass) -> &'static str {
    match class {
        MachineClass::Additive => "additive-print",
        MachineClass::Mill => "subtractive-milling",
        MachineClass::Lathe => "turning",
        MachineClass::Router => "subtractive-routing",
        MachineClass::SheetCut => "sheet-cutting",
        MachineClass::Other => "manual-or-special-process",
    }
}

fn operation_for_part(part: &PartPlan) -> &'static str {
    match machine_class(&part.machine_kind) {
        MachineClass::Additive if is_resin_printer_kind(&part.machine_kind) => {
            "orient, support, resin print, wash, and UV cure"
        }
        MachineClass::Additive if is_powder_bed_printer_kind(&part.machine_kind) => {
            "nest, powder-bed print, cool down, depowder, and finish"
        }
        MachineClass::Additive => "slice, support, and print",
        MachineClass::Mill if is_horizontal_mill_kind(&part.machine_kind) => {
            "index fixture, side-mill slots, and finish horizontal features"
        }
        MachineClass::Mill => "face, rough, contour, and finish critical features",
        MachineClass::Lathe => "face, rough turn, finish turn, and bore/thread if needed",
        MachineClass::Router => "profile, pocket, and tab-cut",
        MachineClass::SheetCut => "kerf-test, pierce, cut/engrave sheet profile, and inspect",
        MachineClass::Other => "prepare operator-reviewed special process",
    }
}

fn expected_minutes(class: MachineClass, tolerance_mm: f64) -> u32 {
    let base = match class {
        MachineClass::Additive => 120.0,
        MachineClass::Mill => 55.0,
        MachineClass::Lathe => 35.0,
        MachineClass::Router => 40.0,
        MachineClass::SheetCut => 25.0,
        MachineClass::Other => 60.0,
    };
    let tolerance_factor = if tolerance_mm <= 0.05 {
        1.8
    } else if tolerance_mm <= 0.1 {
        1.35
    } else {
        1.0
    };
    (base * tolerance_factor) as u32
}

fn generate_program(part: &PartPlan, machine: &MachineProfile) -> GeneratedProgram {
    let class = machine_class(&machine.kind);
    let program_id = format!("{}-{}", part.id, normalize_token(&machine.kind));
    let (language, instructions, safety_notes) = match class {
        MachineClass::Additive if is_resin_printer_kind(&machine.kind) => (
            machine
                .controller
                .clone()
                .unwrap_or_else(|| "sla-job".to_string()),
            vec![
                "; draft resin SLA/MSLA job generated by dd-fabrication-server".to_string(),
                "CHECKPOINT [setup-boundary]: verify resin, vat film, build plate, PPE, and ventilation".to_string(),
                "ORIENT part with drain paths and support touchpoints reviewed".to_string(),
                "SLICE layer_height_mm=0.050 exposure_s=2.4 lift_mm=6.0".to_string(),
                "PRINT resin job with operator-reviewed anti-aliasing and compensation".to_string(),
                "CHECKPOINT [process-split-boundary]: drip, remove build plate, and transfer to wash station".to_string(),
                "WASH ipa_minutes=8; keep uncured resin waste contained".to_string(),
                "CHECKPOINT [human-intervention]: remove supports after wash and inspect fragile features".to_string(),
                "UV_CURE minutes=12 rotation=on; verify material datasheet before final cure".to_string(),
                "COMPLETE record cure cycle, dimensional inspection, and resin batch".to_string(),
            ],
            vec![
                "Draft only: generate the final SLA/MSLA job from the actual mesh, resin profile, supports, and exposure calibration."
                    .to_string(),
                "Human signoff is required for resin handling, wash/cure timing, support removal, and dimensional inspection."
                    .to_string(),
            ],
        ),
        MachineClass::Additive if is_powder_bed_printer_kind(&machine.kind) => (
            machine
                .controller
                .clone()
                .unwrap_or_else(|| "sls-job".to_string()),
            vec![
                "; draft powder-bed additive job generated by dd-fabrication-server".to_string(),
                "CHECKPOINT [setup-boundary]: verify powder lot, refresh ratio, nitrogen/thermal profile, and build volume".to_string(),
                "NEST parts with thermal spacing and unpacking access reviewed".to_string(),
                "PRINT powder-bed job layer_height_mm=0.100 energy_profile=operator-reviewed".to_string(),
                "CHECKPOINT [cooldown-boundary]: hold closed build chamber until safe unpack temperature".to_string(),
                "DEPOWDER using approved PPE, grounded vacuum, and powder recovery workflow".to_string(),
                "CHECKPOINT [process-split-boundary]: bead blast, dye, seal, or tumble only after first-article inspection".to_string(),
                "COMPLETE record powder reuse state, cooldown curve, and dimensional inspection".to_string(),
            ],
            vec![
                "Draft only: final SLS/MJF parameters must come from the printer vendor profile and material batch validation."
                    .to_string(),
                "Human signoff is required for thermal cooldown, depowdering, powder reuse, and post-processing gates."
                    .to_string(),
            ],
        ),
        MachineClass::Additive => (
            machine
                .controller
                .clone()
                .unwrap_or_else(|| "marlin-gcode".to_string()),
            vec![
                "; draft additive program generated by dd-fabrication-server".to_string(),
                "G21 ; millimeters".to_string(),
                "G90 ; absolute positioning".to_string(),
                "M104 S205 ; set nozzle temperature for operator-reviewed material".to_string(),
                "M140 S60 ; set bed temperature".to_string(),
                "G28 ; home axes".to_string(),
                "M109 S205 ; wait for nozzle temperature".to_string(),
                "M190 S60 ; wait for bed temperature".to_string(),
                "G1 Z0.28 F1200 ; first-layer height".to_string(),
                "G1 X20 Y20 E0.8 F900 ; prime and begin perimeter".to_string(),
                "G1 X80 Y20 E4.0 F1500".to_string(),
                "G1 X80 Y80 E7.2 F1500".to_string(),
                "G1 X20 Y80 E10.4 F1500".to_string(),
                "G1 X20 Y20 E13.6 F1500".to_string(),
                "M104 S0 ; cool nozzle".to_string(),
                "M140 S0 ; cool bed".to_string(),
                "M84 ; disable motors".to_string(),
            ],
            vec![
                "Draft only: slice against the actual mesh, nozzle, filament, and bed profile before running."
                    .to_string(),
                "Human signoff is required for temperatures, supports, bed adhesion, and collision clearance."
                    .to_string(),
            ],
        ),
        MachineClass::Mill if is_horizontal_mill_kind(&machine.kind) => (
            machine
                .controller
                .clone()
                .unwrap_or_else(|| "iso-gcode".to_string()),
            vec![
                "(draft horizontal milling program generated by dd-fabrication-server)".to_string(),
                "G21 G90 G17 ; millimeters, absolute, XY plane".to_string(),
                "G54 ; operator-verified tombstone or fixture offset".to_string(),
                "T3 M6 ; side-and-face cutter or slab mill".to_string(),
                "S6500 M3 ; horizontal spindle on clockwise".to_string(),
                "G0 X0 Y0 Z25 ; clear fixture and arbor".to_string(),
                "M0 ; verify arbor, overarm, guards, and side-clearance before slotting".to_string(),
                "G0 X-5 Y0 Z5".to_string(),
                "G1 Z-6.0 F80 ; conservative slot depth".to_string(),
                "G1 X120 F260 ; side slot roughing pass".to_string(),
                "G0 Z20".to_string(),
                "M0 ; index fixture or inspect keyway before finish pass".to_string(),
                "G0 X-5 Y0 Z4".to_string(),
                "G1 Z-6.4 F60 ; finish side feature".to_string(),
                "G1 X120 F180".to_string(),
                "G0 Z30".to_string(),
                "M5".to_string(),
                "M30".to_string(),
            ],
            vec![
                "Draft only: verify arbor support, side cutter width, fixture indexing, overarm clearance, and chip evacuation before running."
                    .to_string(),
                "Horizontal mill operations need inspection at each programmed stop before index or finish passes."
                    .to_string(),
            ],
        ),
        MachineClass::Mill => (
            machine
                .controller
                .clone()
                .unwrap_or_else(|| "iso-gcode".to_string()),
            vec![
                "(draft milling program generated by dd-fabrication-server)".to_string(),
                "G21 G90 G17 ; millimeters, absolute, XY plane".to_string(),
                "G54 ; operator-verified work offset".to_string(),
                "T1 M6 ; face/roughing tool".to_string(),
                "S8000 M3 ; spindle on clockwise".to_string(),
                "G0 X0 Y0 Z15".to_string(),
                "G1 Z-0.5 F120 ; conservative facing pass".to_string(),
                "G1 X60 Y0 F450".to_string(),
                "G1 X60 Y40".to_string(),
                "G1 X0 Y40".to_string(),
                "G0 Z15".to_string(),
                "M5 ; spindle stop".to_string(),
                "M0 ; inspect workholding and chips before finish pass".to_string(),
                "T2 M6 ; finishing tool".to_string(),
                "S10000 M3".to_string(),
                "G0 X0 Y0 Z10".to_string(),
                "G1 Z-0.2 F90 ; finish critical face".to_string(),
                "G0 Z25".to_string(),
                "M5".to_string(),
                "M30".to_string(),
            ],
            vec![
                "Draft only: generate final CAM from verified stock, fixtures, tools, feeds, speeds, and postprocessor."
                    .to_string(),
                "Human signoff is required after the programmed stop and before any fixture change."
                    .to_string(),
            ],
        ),
        MachineClass::Router => (
            machine
                .controller
                .clone()
                .unwrap_or_else(|| "grbl-gcode".to_string()),
            vec![
                "(draft router profile program generated by dd-fabrication-server)".to_string(),
                "G21 G90 G17 ; millimeters, absolute, XY plane".to_string(),
                "G54 ; operator-verified spoilboard work offset".to_string(),
                "S18000 M3 ; router spindle on clockwise".to_string(),
                "G0 X0 Y0 Z12 ; safe clearance above clamps".to_string(),
                "G1 Z-2.0 F180 ; first profile depth".to_string(),
                "G1 X180 Y0 F900 ; profile edge".to_string(),
                "G1 X180 Y90".to_string(),
                "G1 X0 Y90".to_string(),
                "G1 X0 Y0".to_string(),
                "G0 Z6 ; lift over tab boundary".to_string(),
                "G0 X45 Y0 ; skip retained tab".to_string(),
                "G1 Z-4.0 F160 ; second profile depth".to_string(),
                "G1 X180 Y0 F800".to_string(),
                "G1 X180 Y90".to_string(),
                "G1 X0 Y90".to_string(),
                "G1 X0 Y0".to_string(),
                "M0 ; inspect tabs, clamps, dust collection, and chip evacuation".to_string(),
                "G0 Z15".to_string(),
                "M5 ; spindle stop".to_string(),
                "M30".to_string(),
            ],
            vec![
                "Draft only: verify hold-down, tab placement, cutter diameter, dust collection, and spoilboard clearance before running."
                    .to_string(),
                "Router paths need a controller-specific postprocessor and dry-run because clamps and tabs are machine-specific."
                    .to_string(),
            ],
        ),
        MachineClass::SheetCut => {
            if is_waterjet_cutter_kind(&machine.kind) {
                (
                    machine
                        .controller
                        .clone()
                        .unwrap_or_else(|| "waterjet-job".to_string()),
                    vec![
                        "; draft waterjet sheet-cutting job generated by dd-fabrication-server"
                            .to_string(),
                        "CHECKPOINT [setup-boundary]: verify sheet material, thickness, slats, garnet hopper, nozzle/orifice, water level, and part catch"
                            .to_string(),
                        "KERF_TEST coupon_width_mm=25 abrasive=operator-reviewed pressure=operator-reviewed feed=operator-reviewed"
                            .to_string(),
                        "ABRASIVE_FLOW_TEST confirm garnet feed, water pressure, and nozzle health before piercing"
                            .to_string(),
                        "PIERCE_DELAY at lead-in points with low-pressure pierce or predrill when material requires it"
                            .to_string(),
                        "WATERJET_CUT outside profile with tabs/bridges and verified taper/kerf compensation"
                            .to_string(),
                        "CHECKPOINT [sheet-cutting-boundary]: inspect pierce blowout, taper, abrasive feed, slat collision risk, and part release"
                            .to_string(),
                        "COMPLETE record material lot, kerf coupon, garnet usage, and edge inspection"
                            .to_string(),
                    ],
                    vec![
                        "Draft only: final waterjet pressure, abrasive flow, standoff, pierce delay, and taper compensation must come from the machine/material database."
                            .to_string(),
                        "Human signoff is required for high-pressure water, garnet handling, slat support, catcher state, and part-retention risk before cutting."
                            .to_string(),
                    ],
                )
            } else if is_plasma_cutter_kind(&machine.kind) {
                (
                    machine
                        .controller
                        .clone()
                        .unwrap_or_else(|| "plasma-job".to_string()),
                    vec![
                        "; draft plasma sheet-cutting job generated by dd-fabrication-server"
                            .to_string(),
                        "CHECKPOINT [setup-boundary]: verify conductive work clamp, torch consumables, gas, pierce height, cut height, ventilation, and fire watch"
                            .to_string(),
                        "KERF_TEST coupon_width_mm=25 amperage=operator-reviewed gas=operator-reviewed feed=operator-reviewed"
                            .to_string(),
                        "PIERCE_HEIGHT set from material table; wait for ARC_OK before feed motion"
                            .to_string(),
                        "PLASMA_CUT outside profile with lead-ins, dross allowance, and verified kerf compensation"
                            .to_string(),
                        "CHECKPOINT [sheet-cutting-boundary]: inspect arc transfer, dross, fumes, heat distortion, retained tabs, and part release"
                            .to_string(),
                        "COMPLETE record material lot, kerf coupon, consumable state, and edge inspection"
                            .to_string(),
                    ],
                    vec![
                        "Draft only: final plasma amperage, gas, torch height, pierce delay, and feed rates must come from machine-specific cut charts."
                            .to_string(),
                        "Human signoff is required for conductive workholding, fumes/ventilation, fire watch, consumables, and thermal distortion before cutting."
                            .to_string(),
                    ],
                )
            } else {
                (
                    machine
                        .controller
                        .clone()
                        .unwrap_or_else(|| "laser-job".to_string()),
                    vec![
                        "; draft laser sheet-cutting job generated by dd-fabrication-server"
                            .to_string(),
                        "CHECKPOINT [setup-boundary]: verify sheet material, thickness, lens/focus, ventilation, fire watch, and honeycomb bed"
                            .to_string(),
                        "KERF_TEST coupon_width_mm=20 power=operator-reviewed speed=operator-reviewed"
                            .to_string(),
                        "PIERCE at lead-in points only after focus and assist-air check"
                            .to_string(),
                        "VECTOR_ENGRAVE optional marks before through-cut; preserve datums"
                            .to_string(),
                        "VECTOR_CUT outside profile with tabs/bridges and verified kerf compensation"
                            .to_string(),
                        "CHECKPOINT [sheet-cutting-boundary]: inspect flame, fumes, pierce quality, retained tabs, and part release"
                            .to_string(),
                        "COMPLETE record material lot, kerf coupon, and edge inspection".to_string(),
                    ],
                    vec![
                        "Draft only: final laser settings must come from machine-specific material, thickness, kerf, focus, and assist-gas validation."
                            .to_string(),
                        "Human signoff is required for fire watch, fumes/ventilation, material certification, and sheet hold-down before cutting."
                            .to_string(),
                    ],
                )
            }
        }
        MachineClass::Lathe => (
            machine
                .controller
                .clone()
                .unwrap_or_else(|| "fanuc-gcode".to_string()),
            vec![
                "(draft turning program generated by dd-fabrication-server)".to_string(),
                "G21 G90 ; millimeters, absolute".to_string(),
                "G54 ; operator-verified work offset".to_string(),
                "T0101 ; rough turning tool".to_string(),
                "G50 S3000 ; spindle speed limit".to_string(),
                "G96 S180 M3 ; constant surface speed".to_string(),
                "G0 X42 Z2".to_string(),
                "G1 Z-40 F0.20 ; rough turn".to_string(),
                "G0 X45 Z5".to_string(),
                "M0 ; measure diameter before finish cut".to_string(),
                "T0202 ; finishing tool".to_string(),
                "G96 S220 M3".to_string(),
                "G0 X40.2 Z1".to_string(),
                "G1 Z-40 F0.08 ; finish pass".to_string(),
                "M5".to_string(),
                "M30".to_string(),
            ],
            vec![
                "Draft only: verify stock stick-out, chuck clearance, tool nose radius, and spindle limits."
                    .to_string(),
                "Human measurement is required at the programmed stop before the finish pass.".to_string(),
            ],
        ),
        MachineClass::Other => (
            "operator-instructions".to_string(),
            vec![
                "Draft process note: no automatic program is available for this machine kind.".to_string(),
                "Prepare a machine-specific postprocessor and run a dry simulation before use.".to_string(),
            ],
            vec!["Human programming is required for unsupported machine kinds.".to_string()],
        ),
    };

    GeneratedProgram {
        program_id,
        part_id: part.id.clone(),
        machine_id: machine.id.clone(),
        machine_kind: machine.kind.clone(),
        language,
        draft: true,
        machine_ready: false,
        instructions,
        safety_notes,
    }
}

fn validate_programs(programs: &[InstructionProgram]) -> Result<Vec<InstructionProgram>, String> {
    if programs.len() > MAX_PROGRAMS {
        return Err(format!(
            "programs must contain at most {MAX_PROGRAMS} entries"
        ));
    }
    programs
        .iter()
        .enumerate()
        .map(|(index, program)| {
            if program.instructions.len() > MAX_PROGRAM_LINES {
                return Err(format!(
                    "program {index} must contain at most {MAX_PROGRAM_LINES} instruction lines"
                ));
            }
            Ok(InstructionProgram {
                id: program
                    .id
                    .as_ref()
                    .map(|id| validate_label(id, "program.id"))
                    .transpose()?,
                machine_id: program
                    .machine_id
                    .as_ref()
                    .map(|id| validate_label(id, "program.machineId"))
                    .transpose()?,
                machine_kind: program
                    .machine_kind
                    .as_ref()
                    .map(|kind| validate_text(kind, "program.machineKind", MAX_LABEL_LEN))
                    .transpose()?,
                language: program
                    .language
                    .as_ref()
                    .map(|language| validate_text(language, "program.language", MAX_LABEL_LEN))
                    .transpose()?,
                instructions: program
                    .instructions
                    .iter()
                    .map(|line| validate_text(line, "program.instructions", MAX_TEXT_LEN))
                    .collect::<Result<Vec<_>, _>>()?,
            })
        })
        .collect()
}

fn strip_comment(line: &str) -> String {
    let mut output = String::new();
    let mut in_paren = false;
    for character in line.chars() {
        match character {
            '(' => in_paren = true,
            ')' => in_paren = false,
            ';' if !in_paren => break,
            _ if !in_paren => output.push(character),
            _ => {}
        }
    }
    output.trim().to_ascii_uppercase()
}

fn line_mentions(line: &str, needle: &str) -> bool {
    line.to_ascii_lowercase().contains(needle)
}

fn contains_code(line: &str, code: &str) -> bool {
    strip_comment(line)
        .split_whitespace()
        .any(|token| token == code || token.starts_with(&format!("{code}.")))
}

fn has_any_code(line: &str, codes: &[&str]) -> bool {
    codes.iter().any(|code| contains_code(line, code))
}

fn has_numeric_tool_select(line: &str) -> bool {
    strip_comment(line).split_whitespace().any(|token| {
        token.strip_prefix('T').is_some_and(|suffix| {
            !suffix.is_empty() && suffix.chars().all(|character| character.is_ascii_digit())
        })
    })
}

fn has_tool_length_compensation(line: &str) -> bool {
    let stripped = strip_comment(line);
    has_any_code(&stripped, &["G43", "G43.1"]) || number_after(&stripped, 'H').is_some()
}

fn number_after(line: &str, axis: char) -> Option<f64> {
    let stripped = strip_comment(line);
    for token in stripped.split_whitespace() {
        if token.starts_with(axis) {
            if let Ok(value) = token[axis.len_utf8()..].parse::<f64>() {
                return Some(value);
            }
        }
    }
    None
}

fn is_machine_code_language(language: &str) -> bool {
    let token = normalize_token(language);
    token.contains("gcode")
        || token.contains("g-code")
        || matches!(
            token.as_str(),
            "nc" | "cnc"
                | "fanuc"
                | "haas"
                | "grbl"
                | "marlin"
                | "klipper"
                | "prusa"
                | "shopbot"
                | "heidenhain"
                | "mazatrol"
        )
}

fn program_id_at(program: &InstructionProgram, index: usize) -> String {
    program
        .id
        .clone()
        .unwrap_or_else(|| format!("program-{}", index + 1))
}

fn machine_for_program<'a>(
    program: &InstructionProgram,
    machines: &'a [MachineProfile],
) -> Option<&'a MachineProfile> {
    if let Some(machine_id) = program.machine_id.as_deref() {
        if let Some(machine) = machines.iter().find(|machine| machine.id == machine_id) {
            return Some(machine);
        }
    }
    if let Some(machine_kind) = program.machine_kind.as_deref() {
        if let Some(machine) = machines.iter().find(|machine| machine.kind == machine_kind) {
            return Some(machine);
        }
        let class = machine_class(machine_kind);
        if class != MachineClass::Other {
            return machines
                .iter()
                .find(|machine| machine_class(&machine.kind) == class);
        }
    }
    None
}

fn axis_limit(class: MachineClass, envelope: &[f64], axis: char) -> Option<f64> {
    match class {
        MachineClass::Lathe => match axis {
            'X' => envelope.first().copied(),
            'Z' => envelope.get(1).copied(),
            _ => None,
        },
        _ => match axis {
            'X' => envelope.first().copied(),
            'Y' => envelope.get(1).copied(),
            'Z' => envelope.get(2).copied(),
            _ => None,
        },
    }
}

fn coordinate_exceeds_limit(class: MachineClass, axis: char, value: f64, limit: f64) -> bool {
    let tolerance = 0.001;
    match axis {
        'X' | 'Y' => value < -tolerance || value > limit + tolerance,
        'Z' if class == MachineClass::Additive => value < -tolerance || value > limit + tolerance,
        'Z' => value.abs() > limit + tolerance,
        _ => false,
    }
}

fn simulate_instruction_programs(
    programs: &[InstructionProgram],
    machines: &[MachineProfile],
) -> SimulationReport {
    let mut traces = Vec::with_capacity(programs.len());
    let mut findings = Vec::new();
    let mut boundaries = Vec::new();

    for (program_index, program) in programs.iter().enumerate() {
        let program_id = program_id_at(program, program_index);
        let machine_kind = program
            .machine_kind
            .clone()
            .unwrap_or_else(|| "unknown".to_string());
        let language = program
            .language
            .clone()
            .unwrap_or_else(|| "gcode".to_string());
        let machine = machine_for_program(program, machines);
        let class = machine_class(
            machine
                .map(|machine| machine.kind.as_str())
                .unwrap_or(machine_kind.as_str()),
        );
        let work_envelope_mm = machine.and_then(|machine| machine.work_envelope_mm.clone());
        let mut absolute = true;
        let mut unit_scale = 1.0;
        let mut current = BTreeMap::<char, f64>::new();
        let mut min_axis = BTreeMap::<char, f64>::new();
        let mut max_axis = BTreeMap::<char, f64>::new();
        let mut motion_line_count = 0_usize;
        let mut safe_clearance_observed = false;
        let mut spindle_or_heatup_observed = false;
        let mut reported_axes = BTreeSet::<char>::new();

        if !is_machine_code_language(&language) {
            traces.push(SimulationProgramTrace {
                program_id,
                machine_id: program.machine_id.clone(),
                machine_kind,
                language,
                motion_line_count,
                work_envelope_mm,
                axis_extents: Vec::new(),
                safe_clearance_observed,
                spindle_or_heatup_observed,
            });
            continue;
        }

        for (line_index, raw_line) in program.instructions.iter().enumerate() {
            let line_number = line_index + 1;
            let stripped = strip_comment(raw_line);
            if stripped.is_empty() {
                continue;
            }
            if has_any_code(&stripped, &["G20"]) {
                unit_scale = 25.4;
            }
            if has_any_code(&stripped, &["G21"]) {
                unit_scale = 1.0;
            }
            if has_any_code(&stripped, &["G90"]) {
                absolute = true;
            }
            if has_any_code(&stripped, &["G91"]) {
                absolute = false;
            }
            if has_any_code(
                &stripped,
                &["M3", "M4", "M03", "M04", "M104", "M109", "M140", "M190"],
            ) {
                spindle_or_heatup_observed = true;
            }
            if number_after(&stripped, 'Z').is_some_and(|z| z * unit_scale > 0.0)
                && has_any_code(&stripped, &["G0", "G00"])
            {
                safe_clearance_observed = true;
            }
            if !has_any_code(
                &stripped,
                &["G0", "G00", "G1", "G01", "G2", "G02", "G3", "G03"],
            ) {
                continue;
            }

            motion_line_count += 1;
            for axis in ['X', 'Y', 'Z', 'E'] {
                let Some(raw_value) = number_after(&stripped, axis) else {
                    continue;
                };
                let scaled = raw_value * unit_scale;
                let next = if absolute {
                    scaled
                } else {
                    current.get(&axis).copied().unwrap_or_default() + scaled
                };
                current.insert(axis, next);
                min_axis
                    .entry(axis)
                    .and_modify(|value| *value = value.min(next))
                    .or_insert(next);
                max_axis
                    .entry(axis)
                    .and_modify(|value| *value = value.max(next))
                    .or_insert(next);

                let Some(envelope) = work_envelope_mm.as_ref() else {
                    continue;
                };
                let Some(limit) = axis_limit(class, envelope, axis) else {
                    continue;
                };
                if coordinate_exceeds_limit(class, axis, next, limit) && reported_axes.insert(axis)
                {
                    findings.push(ValidationFinding {
                        severity: "error".to_string(),
                        code: "simulated-axis-envelope-exceeded".to_string(),
                        program_id: Some(program_id.clone()),
                        line: Some(line_number),
                        message: format!(
                            "simulated {axis} position {next:.3} mm exceeds machine envelope limit {limit:.3} mm"
                        ),
                    });
                    boundaries.push(FailureBoundary {
                        kind: "simulated-machine-envelope".to_string(),
                        severity: "error".to_string(),
                        program_id: Some(program_id.clone()),
                        line: Some(line_number),
                        reason: format!(
                            "toolpath simulation places axis {axis} at {next:.3} mm outside the retained machine envelope"
                        ),
                        requires_human_intervention: true,
                        suggested_resolution:
                            "choose a larger machine, split/reorient the part, revise work offsets, or regenerate CAM with verified travel limits"
                                .to_string(),
                    });
                }
            }
        }

        let axis_extents = ['X', 'Y', 'Z', 'E']
            .into_iter()
            .filter_map(|axis| {
                let min = min_axis.get(&axis).copied()?;
                let max = max_axis.get(&axis).copied()?;
                let limit = work_envelope_mm
                    .as_ref()
                    .and_then(|envelope| axis_limit(class, envelope, axis));
                let exceeds_limit = limit.is_some_and(|limit| {
                    coordinate_exceeds_limit(class, axis, min, limit)
                        || coordinate_exceeds_limit(class, axis, max, limit)
                });
                Some(SimulationAxisExtent {
                    axis: axis.to_string(),
                    min_mm: min,
                    max_mm: max,
                    limit_mm: limit,
                    exceeds_limit,
                })
            })
            .collect::<Vec<_>>();
        traces.push(SimulationProgramTrace {
            program_id,
            machine_id: program.machine_id.clone(),
            machine_kind,
            language,
            motion_line_count,
            work_envelope_mm,
            axis_extents,
            safe_clearance_observed,
            spindle_or_heatup_observed,
        });
    }

    let severity = report_severity(&findings, &boundaries);
    SimulationReport {
        ok: severity != "error",
        severity,
        programs: traces,
        findings,
        failure_boundaries: boundaries,
    }
}

fn text_has_any(line: &str, needles: &[&str]) -> bool {
    let lower = line.to_ascii_lowercase();
    needles.iter().any(|needle| lower.contains(needle))
}

fn inspect_text_instruction_line(
    raw_line: &str,
    program_id: &str,
    line_number: usize,
    language: &str,
    findings: &mut Vec<ValidationFinding>,
    boundaries: &mut Vec<FailureBoundary>,
    improvements: &mut Vec<InstructionImprovement>,
) -> TextInstructionSignals {
    let mut signals = TextInstructionSignals::default();
    if text_has_any(
        raw_line,
        &[
            "home",
            "probe",
            "zero",
            "work offset",
            "bed level",
            "level bed",
            "fixture",
            "clamp",
            "vise",
            "stickout",
            "tool length",
        ],
    ) {
        signals.has_setup_reference = true;
        boundaries.push(FailureBoundary {
            kind: "setup-boundary".to_string(),
            severity: "warning".to_string(),
            program_id: Some(program_id.to_string()),
            line: Some(line_number),
            reason: "text instruction depends on setup, fixture, work offset, probing, bed leveling, or tool length state"
                .to_string(),
            requires_human_intervention: true,
            suggested_resolution:
                "capture setup measurements as explicit preflight data before the machine cycle continues"
                    .to_string(),
        });
    }
    if text_has_any(
        raw_line,
        &[
            "pause",
            "operator",
            "manual",
            "confirm",
            "wait for",
            "remove part",
            "change filament",
            "material change",
            "swap",
            "reload",
        ],
    ) {
        boundaries.push(FailureBoundary {
            kind: "human-intervention".to_string(),
            severity: "warning".to_string(),
            program_id: Some(program_id.to_string()),
            line: Some(line_number),
            reason: "text instruction requires an operator action, pause, material change, or manual confirmation"
                .to_string(),
            requires_human_intervention: true,
            suggested_resolution:
                "split this into an operator-approved checkpoint or provide verified automation for the action"
                    .to_string(),
        });
    }
    if text_has_any(
        raw_line,
        &[
            "wash",
            "cure",
            "uv",
            "depowder",
            "sinter",
            "anneal",
            "deburr",
            "remove support",
            "support removal",
            "post process",
            "post-process",
        ],
    ) {
        signals.has_process_preparation = true;
        findings.push(ValidationFinding {
            severity: "warning".to_string(),
            code: "text-post-processing-boundary".to_string(),
            program_id: Some(program_id.to_string()),
            line: Some(line_number),
            message: "text instruction requires post-processing outside the primary machine cycle"
                .to_string(),
        });
        boundaries.push(FailureBoundary {
            kind: "post-processing-boundary".to_string(),
            severity: "warning".to_string(),
            program_id: Some(program_id.to_string()),
            line: Some(line_number),
            reason: "wash/cure/depowder/sinter/deburr/support-removal steps cannot complete as one uninterrupted machine program"
                .to_string(),
            requires_human_intervention: true,
            suggested_resolution:
                "model post-processing as its own process step with readiness and inspection gates"
                    .to_string(),
        });
    }
    if text_has_any(
        raw_line,
        &[
            "assemble",
            "assembly",
            "bond",
            "adhesive",
            "epoxy",
            "fasten",
            "screw",
            "pin",
            "press fit",
            "join",
        ],
    ) {
        findings.push(ValidationFinding {
            severity: "warning".to_string(),
            code: "text-assembly-boundary".to_string(),
            program_id: Some(program_id.to_string()),
            line: Some(line_number),
            message: "text instruction combines fabricated pieces after machine work".to_string(),
        });
        boundaries.push(FailureBoundary {
            kind: "assembly-boundary".to_string(),
            severity: "warning".to_string(),
            program_id: Some(program_id.to_string()),
            line: Some(line_number),
            reason: "instruction requires combining parts, bonding, fastening, or fit-up after fabrication"
                .to_string(),
            requires_human_intervention: true,
            suggested_resolution:
                "preserve alignment datums and add inspection before the assembly step".to_string(),
        });
    }
    if text_has_any(
        raw_line,
        &[
            "split",
            "separate piece",
            "two pieces",
            "multiple pieces",
            "segment",
            "part 1",
            "part 2",
        ],
    ) {
        findings.push(ValidationFinding {
            severity: "info".to_string(),
            code: "text-split-boundary".to_string(),
            program_id: Some(program_id.to_string()),
            line: Some(line_number),
            message: "text instruction separates the object into multiple fabricated pieces"
                .to_string(),
        });
        boundaries.push(FailureBoundary {
            kind: "split-boundary".to_string(),
            severity: "warning".to_string(),
            program_id: Some(program_id.to_string()),
            line: Some(line_number),
            reason:
                "instruction indicates the object must be separated into pieces or recombined later"
                    .to_string(),
            requires_human_intervention: true,
            suggested_resolution:
                "track each piece as a separate operation and make recombination explicit"
                    .to_string(),
        });
    }
    if text_has_any(
        raw_line,
        &["complete", "done", "finish", "finished", "end job"],
    ) {
        signals.has_completion_marker = true;
    }
    if text_has_any(
        raw_line,
        &["ppe", "gloves", "respirator", "ventilation", "enclosure"],
    ) {
        signals.has_process_preparation = true;
    }
    if text_has_any(
        raw_line,
        &[
            "laser",
            "waterjet",
            "water-jet",
            "plasma",
            "kerf",
            "pierce",
            "assist gas",
            "assist-air",
            "fume",
            "fire watch",
            "lens",
            "focus",
        ],
    ) {
        signals.has_process_preparation = true;
        findings.push(ValidationFinding {
            severity: "warning".to_string(),
            code: "text-sheet-cutting-boundary".to_string(),
            program_id: Some(program_id.to_string()),
            line: Some(line_number),
            message:
                "text instruction depends on sheet-cutting kerf, fire/fume, pierce, or focus state"
                    .to_string(),
        });
        boundaries.push(FailureBoundary {
            kind: "sheet-cutting-boundary".to_string(),
            severity: "warning".to_string(),
            program_id: Some(program_id.to_string()),
            line: Some(line_number),
            reason:
                "laser/waterjet/plasma/knife-style sheet work needs kerf, material, focus, fire/fume, and release checks outside an unattended cycle"
                    .to_string(),
            requires_human_intervention: true,
            suggested_resolution:
                "record kerf coupons, material certification, focus/assist-gas settings, ventilation/fire-watch checks, and tab/release inspection"
                    .to_string(),
        });
    }
    if text_has_any(
        raw_line,
        &["proprietary", "vendor only", "unsupported", "unknown"],
    ) {
        findings.push(ValidationFinding {
            severity: "info".to_string(),
            code: "text-controller-specific-step".to_string(),
            program_id: Some(program_id.to_string()),
            line: Some(line_number),
            message: "text instruction references a controller/vendor-specific or unknown step"
                .to_string(),
        });
    }
    if language != "setup-sheet" && text_has_any(raw_line, &["checklist", "setup sheet"]) {
        improvements.push(InstructionImprovement {
            program_id: Some(program_id.to_string()),
            line: Some(line_number),
            action: "normalize-text-setup-sheet".to_string(),
            reason:
                "text program includes setup-sheet concepts but is not labelled as a setup sheet"
                    .to_string(),
        });
    }
    signals
}

fn analyze_instruction_programs(
    programs: &[InstructionProgram],
) -> (
    Vec<AnalyzedProgram>,
    ValidationReport,
    Vec<InstructionImprovement>,
) {
    let mut analyzed = Vec::with_capacity(programs.len());
    let mut findings = Vec::new();
    let mut boundaries = Vec::new();
    let mut improvements = Vec::new();

    for (program_index, program) in programs.iter().enumerate() {
        let program_id = program
            .id
            .clone()
            .unwrap_or_else(|| format!("program-{}", program_index + 1));
        let machine_kind = program
            .machine_kind
            .clone()
            .unwrap_or_else(|| "unknown".to_string());
        let language = program
            .language
            .clone()
            .unwrap_or_else(|| "gcode".to_string());
        let normalized_language = normalize_token(&language);
        let machine_code_language = is_machine_code_language(&language);
        let class = machine_class(&machine_kind);
        let mut has_units_mode = false;
        let mut has_positioning_mode = false;
        let mut has_homing_or_fixture_reference = false;
        let mut has_spindle_or_heatup = false;
        let mut has_program_end = false;
        let mut has_feed_move = false;
        let mut has_extrusion = false;
        let mut has_tool_selection = false;
        let mut has_tool_length_reference = false;
        let mut reported_tool_length_boundary = false;
        let mut has_lathe_spindle_limit = false;
        let mut reported_lathe_css_boundary = false;
        let mut reported_lathe_threading_boundary = false;
        let mut reported_lathe_partoff_boundary = false;
        let findings_at_program_start = findings.len();
        let boundaries_at_program_start = boundaries.len();

        for (line_index, raw_line) in program.instructions.iter().enumerate() {
            let line_number = line_index + 1;
            if !machine_code_language {
                if raw_line.trim().is_empty() {
                    continue;
                }
                let signals = inspect_text_instruction_line(
                    raw_line,
                    &program_id,
                    line_number,
                    &normalized_language,
                    &mut findings,
                    &mut boundaries,
                    &mut improvements,
                );
                has_homing_or_fixture_reference |= signals.has_setup_reference;
                has_spindle_or_heatup |= signals.has_process_preparation;
                has_program_end |= signals.has_completion_marker;
                continue;
            }
            let stripped = strip_comment(raw_line);
            if stripped.is_empty() {
                continue;
            }

            let line_has_feed_move =
                has_any_code(&stripped, &["G1", "G01", "G2", "G02", "G3", "G03"]);

            if has_any_code(&stripped, &["G20", "G21"]) {
                has_units_mode = true;
            }
            if has_any_code(&stripped, &["G90", "G91"]) {
                has_positioning_mode = true;
            }
            if has_any_code(
                &stripped,
                &["G28", "G53", "G54", "G55", "G56", "G57", "G58", "G59"],
            ) {
                has_homing_or_fixture_reference = true;
            }
            if has_any_code(
                &stripped,
                &["M3", "M4", "M03", "M04", "M104", "M109", "M140", "M190"],
            ) {
                has_spindle_or_heatup = true;
            }
            if has_any_code(&stripped, &["M2", "M02", "M30"]) || contains_code(&stripped, "M84") {
                has_program_end = true;
            }
            if line_has_feed_move {
                has_feed_move = true;
            }
            if number_after(&stripped, 'E').is_some() {
                has_extrusion = true;
            }
            if matches!(class, MachineClass::Mill | MachineClass::Router) {
                if has_numeric_tool_select(&stripped) || has_any_code(&stripped, &["M6", "M06"]) {
                    has_tool_selection = true;
                }
                if has_tool_length_compensation(&stripped)
                    || line_mentions(raw_line, "tool length")
                    || line_mentions(raw_line, "probe")
                {
                    has_tool_length_reference = true;
                }
            }
            if class == MachineClass::Lathe {
                if has_any_code(&stripped, &["G50"]) && number_after(&stripped, 'S').is_some() {
                    has_lathe_spindle_limit = true;
                }
                if has_any_code(&stripped, &["G96"])
                    && !has_lathe_spindle_limit
                    && !reported_lathe_css_boundary
                {
                    reported_lathe_css_boundary = true;
                    findings.push(ValidationFinding {
                        severity: "warning".to_string(),
                        code: "lathe-css-without-spindle-limit".to_string(),
                        program_id: Some(program_id.clone()),
                        line: Some(line_number),
                        message:
                            "lathe constant-surface-speed mode appears before an explicit G50 spindle speed limit"
                                .to_string(),
                    });
                    boundaries.push(FailureBoundary {
                        kind: "lathe-spindle-speed-boundary".to_string(),
                        severity: "warning".to_string(),
                        program_id: Some(program_id.clone()),
                        line: Some(line_number),
                        reason:
                            "constant surface speed can overspeed a lathe spindle as diameter changes unless a verified maximum RPM is set"
                                .to_string(),
                        requires_human_intervention: true,
                        suggested_resolution:
                            "insert a reviewed G50 S spindle cap, verify chuck/workholding RPM limits, and rerun turning simulation before release"
                                .to_string(),
                    });
                }
                let normalized_line = normalize_token(raw_line);
                if !reported_lathe_threading_boundary
                    && (has_any_code(&stripped, &["G32", "G33", "G76", "G92"])
                        || normalized_line.contains("thread"))
                {
                    reported_lathe_threading_boundary = true;
                    findings.push(ValidationFinding {
                        severity: "warning".to_string(),
                        code: "lathe-threading-boundary".to_string(),
                        program_id: Some(program_id.clone()),
                        line: Some(line_number),
                        message: "lathe threading move requires synchronized spindle/feed review"
                            .to_string(),
                    });
                    boundaries.push(FailureBoundary {
                        kind: "lathe-threading-boundary".to_string(),
                        severity: "warning".to_string(),
                        program_id: Some(program_id.clone()),
                        line: Some(line_number),
                        reason:
                            "threading cycles depend on pitch, spindle encoder, tool orientation, relief, and verified retry strategy"
                                .to_string(),
                        requires_human_intervention: true,
                        suggested_resolution:
                            "split threading into an operator-approved setup with pitch gauge, spring-pass, relief, and clearance checks"
                                .to_string(),
                    });
                }
                if !reported_lathe_partoff_boundary
                    && (has_any_code(&stripped, &["G75"])
                        || normalized_line.contains("part-off")
                        || normalized_line.contains("cut-off")
                        || normalized_line.contains("cutoff"))
                {
                    reported_lathe_partoff_boundary = true;
                    findings.push(ValidationFinding {
                        severity: "warning".to_string(),
                        code: "lathe-part-off-boundary".to_string(),
                        program_id: Some(program_id.clone()),
                        line: Some(line_number),
                        message: "lathe part-off or cutoff step needs workholding and catch review"
                            .to_string(),
                    });
                    boundaries.push(FailureBoundary {
                        kind: "lathe-part-off-boundary".to_string(),
                        severity: "warning".to_string(),
                        program_id: Some(program_id.clone()),
                        line: Some(line_number),
                        reason:
                            "part-off operations can pinch tools, drop parts, or exceed stick-out/workholding limits without operator-reviewed support"
                                .to_string(),
                        requires_human_intervention: true,
                        suggested_resolution:
                            "add a catch/support plan, verify stick-out and cutoff tool geometry, and split the operation before unattended release"
                                .to_string(),
                    });
                }
            }

            if has_any_code(&stripped, &["M0", "M00", "M1", "M01", "M600"])
                || line_mentions(raw_line, "manual")
                || line_mentions(raw_line, "pause")
                || line_mentions(raw_line, "fixture")
                || line_mentions(raw_line, "flip")
            {
                boundaries.push(FailureBoundary {
                    kind: "human-intervention".to_string(),
                    severity: "warning".to_string(),
                    program_id: Some(program_id.clone()),
                    line: Some(line_number),
                    reason: "program contains a stop, filament/tool intervention, fixture change, or manual-operation marker"
                        .to_string(),
                    requires_human_intervention: true,
                    suggested_resolution:
                        "split the job into operator-approved setup phases or add explicit robotic/fixture automation"
                            .to_string(),
                });
            }

            if class == MachineClass::Additive
                && (has_any_code(&stripped, &["M600", "M701", "M702"])
                    || has_numeric_tool_select(&stripped)
                    || line_mentions(raw_line, "filament change")
                    || line_mentions(raw_line, "color change")
                    || line_mentions(raw_line, "material change")
                    || line_mentions(raw_line, "tool change")
                    || line_mentions(raw_line, "toolchange"))
            {
                findings.push(ValidationFinding {
                    severity: "warning".to_string(),
                    code: "additive-material-change-boundary".to_string(),
                    program_id: Some(program_id.clone()),
                    line: Some(line_number),
                    message:
                        "additive program requires a material, color, filament, or tool-change intervention"
                            .to_string(),
                });
                boundaries.push(FailureBoundary {
                    kind: "additive-material-change-boundary".to_string(),
                    severity: "warning".to_string(),
                    program_id: Some(program_id.clone()),
                    line: Some(line_number),
                    reason:
                        "printer program cannot complete unattended across material/color/tool changes without verified filament handling, purge, and resume state"
                            .to_string(),
                    requires_human_intervention: true,
                    suggested_resolution:
                        "split the print at the material-change boundary or add validated AMS/MMU/robotic filament-change automation with purge and inspection checkpoints"
                    .to_string(),
                });
            }

            if matches!(class, MachineClass::Mill | MachineClass::Router)
                && has_tool_selection
                && !has_tool_length_reference
                && !reported_tool_length_boundary
                && line_has_feed_move
                && number_after(&stripped, 'Z').is_some_and(|z| z < 0.0)
            {
                reported_tool_length_boundary = true;
                findings.push(ValidationFinding {
                    severity: "warning".to_string(),
                    code: "missing-tool-length-compensation".to_string(),
                    program_id: Some(program_id.clone()),
                    line: Some(line_number),
                    message:
                        "cutting move follows a tool selection without explicit tool-length compensation or probe state"
                            .to_string(),
                });
                boundaries.push(FailureBoundary {
                    kind: "tool-length-boundary".to_string(),
                    severity: "warning".to_string(),
                    program_id: Some(program_id.clone()),
                    line: Some(line_number),
                    reason:
                        "mill/router toolpath depends on hidden tool length, probe, or offset state before plunging into stock"
                            .to_string(),
                    requires_human_intervention: true,
                    suggested_resolution:
                        "insert G43/G43.1 with the verified H offset, run a documented tool-length probe, or split the setup for operator signoff before cutting"
                            .to_string(),
                });
            }

            if matches!(
                class,
                MachineClass::Mill
                    | MachineClass::Lathe
                    | MachineClass::Router
                    | MachineClass::SheetCut
            ) && has_feed_move
                && !has_spindle_or_heatup
            {
                findings.push(ValidationFinding {
                    severity: "error".to_string(),
                    code: "cut-before-spindle".to_string(),
                    program_id: Some(program_id.clone()),
                    line: Some(line_number),
                    message:
                        "subtractive feed move appears before spindle, beam, jet, or process start"
                            .to_string(),
                });
                boundaries.push(FailureBoundary {
                    kind: "machine-safety-gate".to_string(),
                    severity: "error".to_string(),
                    program_id: Some(program_id.clone()),
                    line: Some(line_number),
                    reason:
                        "cutting move before spindle, beam, jet, or process start can crash tools, misfire, or scrap stock"
                            .to_string(),
                    requires_human_intervention: true,
                    suggested_resolution:
                        "insert operator-verified tool/beam/jet enable, work offset, assist gas/coolant, and safe approach blocks before cutting"
                            .to_string(),
                });
            }

            if class == MachineClass::Additive && has_extrusion && !has_spindle_or_heatup {
                findings.push(ValidationFinding {
                    severity: "error".to_string(),
                    code: "extrusion-before-heatup".to_string(),
                    program_id: Some(program_id.clone()),
                    line: Some(line_number),
                    message: "extrusion appears before nozzle or bed heat-up commands".to_string(),
                });
                boundaries.push(FailureBoundary {
                    kind: "printer-state-gate".to_string(),
                    severity: "error".to_string(),
                    program_id: Some(program_id.clone()),
                    line: Some(line_number),
                    reason: "cold extrusion will fail or damage the extruder".to_string(),
                    requires_human_intervention: false,
                    suggested_resolution:
                        "add material-specific M104/M109 and bed preparation before extrusion"
                            .to_string(),
                });
            }

            if class == MachineClass::Additive && has_extrusion && !has_homing_or_fixture_reference
            {
                findings.push(ValidationFinding {
                    severity: "warning".to_string(),
                    code: "print-before-homing".to_string(),
                    program_id: Some(program_id.clone()),
                    line: Some(line_number),
                    message: "extrusion appears before homing or coordinate reference".to_string(),
                });
            }

            if matches!(
                class,
                MachineClass::Mill | MachineClass::Lathe | MachineClass::Router
            ) && number_after(&stripped, 'Z').is_some_and(|z| z < -20.0)
            {
                findings.push(ValidationFinding {
                    severity: "warning".to_string(),
                    code: "deep-z-cut".to_string(),
                    program_id: Some(program_id.clone()),
                    line: Some(line_number),
                    message:
                        "deep negative Z move needs stock, fixture, and tool-length verification"
                            .to_string(),
                });
            }

            if has_any_code(&stripped, &["G2", "G02", "G3", "G03"])
                && number_after(&stripped, 'I').is_none()
                && number_after(&stripped, 'J').is_none()
                && number_after(&stripped, 'R').is_none()
            {
                findings.push(ValidationFinding {
                    severity: "error".to_string(),
                    code: "arc-missing-center-or-radius".to_string(),
                    program_id: Some(program_id.clone()),
                    line: Some(line_number),
                    message: "arc move is missing I/J center offsets or R radius".to_string(),
                });
            }

            if matches!(class, MachineClass::Mill | MachineClass::Router)
                && stripped.starts_with('T')
                && !stripped.contains("M6")
                && !stripped.contains("M06")
            {
                improvements.push(InstructionImprovement {
                    program_id: Some(program_id.clone()),
                    line: Some(line_number),
                    action: "make-tool-change-explicit".to_string(),
                    reason: "tool selection without M6 may rely on controller-specific state"
                        .to_string(),
                });
            }

            if has_any_code(&stripped, &["M104", "M109"])
                && number_after(&stripped, 'S').is_some_and(|temperature| temperature > 320.0)
            {
                findings.push(ValidationFinding {
                    severity: "warning".to_string(),
                    code: "high-nozzle-temperature".to_string(),
                    program_id: Some(program_id.clone()),
                    line: Some(line_number),
                    message:
                        "nozzle temperature is unusually high; verify material and hotend rating"
                            .to_string(),
                });
            }
        }

        if !machine_code_language {
            if findings.len() == findings_at_program_start
                && boundaries.len() == boundaries_at_program_start
            {
                findings.push(ValidationFinding {
                    severity: "info".to_string(),
                    code: "text-instruction-needs-structured-gates".to_string(),
                    program_id: Some(program_id.clone()),
                    line: None,
                    message:
                        "text instruction has no explicit setup, post-processing, assembly, split, or operator checkpoint markers"
                            .to_string(),
                });
                improvements.push(InstructionImprovement {
                    program_id: Some(program_id.clone()),
                    line: None,
                    action: "add-structured-text-checkpoints".to_string(),
                    reason: "non-G-code instructions should declare setup, material, post-processing, assembly, and completion gates"
                        .to_string(),
                });
            }
        } else if !has_units_mode {
            improvements.push(InstructionImprovement {
                program_id: Some(program_id.clone()),
                line: None,
                action: "add-units-mode".to_string(),
                reason: "program does not declare G20/G21 units".to_string(),
            });
        }
        if machine_code_language && !has_positioning_mode {
            improvements.push(InstructionImprovement {
                program_id: Some(program_id.clone()),
                line: None,
                action: "add-positioning-mode".to_string(),
                reason: "program does not declare G90/G91 absolute or relative positioning"
                    .to_string(),
            });
        }
        if machine_code_language && !has_homing_or_fixture_reference {
            improvements.push(InstructionImprovement {
                program_id: Some(program_id.clone()),
                line: None,
                action: "add-coordinate-reference".to_string(),
                reason: "program does not home axes or select a work coordinate system".to_string(),
            });
        }
        if machine_code_language && !has_program_end {
            findings.push(ValidationFinding {
                severity: "warning".to_string(),
                code: "missing-program-end".to_string(),
                program_id: Some(program_id.clone()),
                line: None,
                message: "program has no explicit end or motor-off command".to_string(),
            });
        }
        if machine_code_language
            && matches!(
                class,
                MachineClass::Mill
                    | MachineClass::Lathe
                    | MachineClass::Router
                    | MachineClass::SheetCut
            )
            && !has_spindle_or_heatup
        {
            findings.push(ValidationFinding {
                severity: "error".to_string(),
                code: "missing-spindle-start".to_string(),
                program_id: Some(program_id.clone()),
                line: None,
                message: "subtractive program has no spindle, beam, jet, or process start command"
                    .to_string(),
            });
        }
        if machine_code_language
            && class == MachineClass::Additive
            && has_extrusion
            && !has_spindle_or_heatup
        {
            findings.push(ValidationFinding {
                severity: "error".to_string(),
                code: "missing-printer-heatup".to_string(),
                program_id: Some(program_id.clone()),
                line: None,
                message: "additive program extrudes without temperature commands".to_string(),
            });
        }

        analyzed.push(AnalyzedProgram {
            program_id,
            machine_kind,
            language,
            line_count: program.instructions.len(),
            has_units_mode,
            has_positioning_mode,
            has_homing_or_fixture_reference,
            has_spindle_or_heatup,
            has_program_end,
        });
    }

    let severity = report_severity(&findings, &boundaries);
    let ok = severity != "error";
    (
        analyzed,
        ValidationReport {
            ok,
            severity,
            findings,
            failure_boundaries: boundaries,
        },
        improvements,
    )
}

fn improvement_applies(
    improvements: &[InstructionImprovement],
    program_id: &str,
    action: &str,
) -> bool {
    improvements.iter().any(|improvement| {
        improvement.action == action
            && match improvement.program_id.as_deref() {
                Some(value) => value == program_id,
                None => true,
            }
    })
}

fn finding_applies(validation: &ValidationReport, program_id: &str, code: &str) -> bool {
    validation.findings.iter().any(|finding| {
        finding.code == code
            && match finding.program_id.as_deref() {
                Some(value) => value == program_id,
                None => true,
            }
    })
}

fn boundary_applies(boundary: &FailureBoundary, program_id: &str, line: Option<usize>) -> bool {
    match boundary.program_id.as_deref() {
        Some(value) if value != program_id => return false,
        _ => {}
    }
    boundary.line == line
}

fn boundary_gate_instruction(machine_code: bool, boundary: &FailureBoundary) -> String {
    if machine_code {
        if boundary.requires_human_intervention {
            format!("M0 ; boundary {}: {}", boundary.kind, boundary.reason)
        } else {
            format!("; boundary {}: {}", boundary.kind, boundary.reason)
        }
    } else {
        format!(
            "CHECKPOINT [{}]: {} Resolution: {}",
            boundary.kind, boundary.reason, boundary.suggested_resolution
        )
    }
}

fn improve_instruction_programs(
    programs: &[InstructionProgram],
    validation: &ValidationReport,
    improvements: &[InstructionImprovement],
) -> Vec<ImprovedInstructionProgram> {
    programs
        .iter()
        .enumerate()
        .map(|(program_index, program)| {
            let program_id = program
                .id
                .clone()
                .unwrap_or_else(|| format!("program-{}", program_index + 1));
            let machine_kind = program
                .machine_kind
                .clone()
                .unwrap_or_else(|| "unknown".to_string());
            let language = program
                .language
                .clone()
                .unwrap_or_else(|| "gcode".to_string());
            let class = machine_class(&machine_kind);
            let machine_code = is_machine_code_language(&language);
            let mut instructions = Vec::new();
            let mut notes = Vec::new();

            for boundary in validation
                .failure_boundaries
                .iter()
                .filter(|boundary| boundary_applies(boundary, &program_id, None))
            {
                instructions.push(boundary_gate_instruction(machine_code, boundary));
            }

            if machine_code {
                if improvement_applies(improvements, &program_id, "add-units-mode") {
                    instructions.push("G21 ; added review draft: metric units".to_string());
                }
                if improvement_applies(improvements, &program_id, "add-positioning-mode") {
                    instructions.push("G90 ; added review draft: absolute positioning".to_string());
                }
                if improvement_applies(improvements, &program_id, "add-coordinate-reference") {
                    let coordinate_line = match class {
                        MachineClass::Additive => "G28 ; added review draft: home axes before motion",
                        _ => "G54 ; added review draft: select primary work coordinate system",
                    };
                    instructions.push(coordinate_line.to_string());
                }
                if finding_applies(validation, &program_id, "missing-spindle-start") {
                    let start_review = match class {
                        MachineClass::SheetCut => {
                            "M0 ; REVIEW: add verified beam/jet enable, assist gas, pierce, and kerf settings before feed moves"
                        }
                        _ => {
                            "M0 ; REVIEW: add verified spindle speed and direction before feed moves"
                        }
                    };
                    instructions.push(start_review.to_string());
                }
                if finding_applies(validation, &program_id, "missing-printer-heatup") {
                    instructions.push(
                        "M0 ; REVIEW: add verified nozzle/bed heat-up commands before extrusion"
                            .to_string(),
                    );
                }
            } else {
                if improvement_applies(improvements, &program_id, "normalize-text-setup-sheet") {
                    notes.push(
                        "Program reads like a setup sheet; label it as setup-sheet before release"
                            .to_string(),
                    );
                }
                if improvement_applies(improvements, &program_id, "add-structured-text-checkpoints")
                {
                    instructions.push(
                        "CHECKPOINT [setup-boundary]: confirm machine setup, material, PPE, and operator readiness"
                            .to_string(),
                    );
                    instructions.push(
                        "CHECKPOINT [process-boundary]: declare post-processing, assembly, split, and completion gates"
                            .to_string(),
                    );
                }
            }

            for (line_index, line) in program.instructions.iter().enumerate() {
                let line_number = line_index + 1;
                for boundary in validation
                    .failure_boundaries
                    .iter()
                    .filter(|boundary| boundary_applies(boundary, &program_id, Some(line_number)))
                {
                    instructions.push(boundary_gate_instruction(machine_code, boundary));
                }
                instructions.push(line.clone());
            }

            if machine_code && finding_applies(validation, &program_id, "missing-program-end") {
                let end_line = match class {
                    MachineClass::Additive => {
                        "M84 ; added review draft: explicit printer idle/end state"
                    }
                    _ => "M30 ; added review draft: explicit program end",
                };
                instructions.push(end_line.to_string());
            } else if !machine_code
                && improvement_applies(improvements, &program_id, "add-structured-text-checkpoints")
            {
                instructions.push(
                    "CHECKPOINT [completion-boundary]: inspection, cleanup, and sign-off recorded"
                        .to_string(),
                );
            }

            let changed = instructions != program.instructions;
            if changed {
                notes.push(
                    "Improved draft inserts validation gates and conservative defaults; human review and simulation are still required"
                        .to_string(),
                );
            } else {
                notes.push(
                    "No automatic rewrite was needed beyond the validation report".to_string(),
                );
            }

            ImprovedInstructionProgram {
                program_id,
                machine_kind,
                language,
                changed,
                machine_ready: false,
                source_line_count: program.instructions.len(),
                instructions,
                notes,
            }
        })
        .collect()
}

fn artifact_id(prefix: &str, raw: &str) -> String {
    let token = normalize_token(raw);
    if token.is_empty() {
        prefix.to_string()
    } else {
        format!("{}-{}", normalize_token(prefix), token)
    }
}

fn json_artifact(
    artifact_id: String,
    kind: &str,
    content: Value,
    created_at_ms: u128,
) -> FabricationArtifact {
    FabricationArtifact {
        artifact_id,
        kind: kind.to_string(),
        media_type: "application/json".to_string(),
        part_id: None,
        program_id: None,
        machine_kind: None,
        draft: false,
        machine_ready: false,
        line_count: None,
        content,
        notes: Vec::new(),
        created_at_ms,
    }
}

fn design_primitive_for_part(part: &PartPlan) -> Value {
    match machine_class(&part.machine_kind) {
        MachineClass::Additive => json!({
            "primitive": "additive-shell",
            "operation": "slice-print",
            "buildOrientation": "auto-upright",
            "supportStrategy": "generated-review-required",
            "datums": ["build-plate-z", "front-left-origin"],
        }),
        MachineClass::Mill if is_horizontal_mill_kind(&part.machine_kind) => json!({
            "primitive": "horizontal-subtractive-feature",
            "operation": "side-slot-keyway-index-finish",
            "stockAllowanceMm": 1.8,
            "datums": ["G54-tombstone-face", "arbor-clearance-plane", "indexed-side-face"],
        }),
        MachineClass::Mill => json!({
            "primitive": "subtractive-prismatic-body",
            "operation": "face-rough-contour-finish",
            "stockAllowanceMm": 1.5,
            "datums": ["G54-top-face", "primary-vise-stop", "machined-datum-face"],
        }),
        MachineClass::Lathe => json!({
            "primitive": "revolved-turned-body",
            "operation": "face-turn-bore-thread",
            "axis": "Z",
            "datums": ["spindle-axis", "chuck-face-z0"],
        }),
        MachineClass::Router => json!({
            "primitive": "subtractive-sheet-profile",
            "operation": "profile-pocket-tab-cut",
            "stockAllowanceMm": 0.8,
            "datums": ["sheet-origin", "spoilboard-z"],
        }),
        MachineClass::SheetCut => json!({
            "primitive": "kerf-controlled-sheet-profile",
            "operation": "kerf-test-pierce-cut-engrave",
            "stockAllowanceMm": 0.2,
            "datums": ["sheet-origin", "focus-plane", "kerf-coupon"],
        }),
        MachineClass::Other => json!({
            "primitive": "operator-defined-special-process",
            "operation": "manual-review-required",
            "datums": ["operator-defined"],
        }),
    }
}

fn parametric_design_content(response: &FabricationPlanResponse) -> Value {
    let parts = response
        .design
        .parts
        .iter()
        .map(|part| {
            json!({
                "partId": part.id,
                "role": part.role,
                "material": part.material,
                "manufacturingMethod": part.manufacturing_method,
                "machineKind": part.machine_kind,
                "toleranceMm": part.tolerance_mm,
                "interfaces": part.interfaces,
                "primitive": design_primitive_for_part(part),
            })
        })
        .collect::<Vec<_>>();
    let process_links = response
        .process_plan
        .iter()
        .map(|step| {
            json!({
                "step": step.step,
                "partId": step.part_id,
                "machineId": step.machine_id,
                "machineKind": step.machine_kind,
                "operation": step.operation,
                "setup": step.setup,
                "requiresHumanIntervention": step.requires_human_intervention,
            })
        })
        .collect::<Vec<_>>();

    json!({
        "schemaVersion": "dd.fabrication.parametric-design.v1",
        "sourceJobId": response.job_id,
        "objectId": response.design.object_id,
        "units": "mm",
        "representation": response.design.representation,
        "releaseState": {
            "draft": true,
            "machineReady": false,
            "requiresSimulation": true,
            "requiresHumanReview": true
        },
        "parts": parts,
        "processLinks": process_links,
        "assembly": {
            "strategy": response.assembly.strategy,
            "combineCandidates": response.assembly.combine_candidates,
            "splitCandidates": response.assembly.split_candidates,
            "joints": response.assembly.joints,
        }
    })
}

fn plan_artifacts(response: &FabricationPlanResponse) -> Vec<FabricationArtifact> {
    let mut artifacts = vec![
        json_artifact(
            "design-summary".to_string(),
            "design-summary",
            json!(response.design),
            response.generated_at_ms,
        ),
        json_artifact(
            "parametric-design".to_string(),
            "parametric-design",
            parametric_design_content(response),
            response.generated_at_ms,
        ),
        json_artifact(
            "process-plan".to_string(),
            "process-plan",
            json!(response.process_plan),
            response.generated_at_ms,
        ),
        json_artifact(
            "assembly-plan".to_string(),
            "assembly-plan",
            json!(response.assembly),
            response.generated_at_ms,
        ),
        json_artifact(
            "validation-report".to_string(),
            "validation-report",
            json!(response.validation),
            response.generated_at_ms,
        ),
        json_artifact(
            "simulation-report".to_string(),
            "simulation-report",
            json!(response.simulation),
            response.generated_at_ms,
        ),
        json_artifact(
            "learning-plan".to_string(),
            "learning-plan",
            json!(response.learning),
            response.generated_at_ms,
        ),
        json_artifact(
            "mdp-request".to_string(),
            "mdp-request",
            fabrication_mdp_request(response),
            response.generated_at_ms,
        ),
    ];

    artifacts.extend(
        response
            .generated_programs
            .iter()
            .map(|program| FabricationArtifact {
                artifact_id: artifact_id("program", &program.program_id),
                kind: "generated-machine-program".to_string(),
                media_type: "application/json".to_string(),
                part_id: Some(program.part_id.clone()),
                program_id: Some(program.program_id.clone()),
                machine_kind: Some(program.machine_kind.clone()),
                draft: program.draft,
                machine_ready: program.machine_ready,
                line_count: Some(program.instructions.len()),
                content: json!({
                    "language": program.language,
                    "instructions": program.instructions,
                }),
                notes: program.safety_notes.clone(),
                created_at_ms: response.generated_at_ms,
            }),
    );
    artifacts
}

fn analysis_artifacts(response: &InstructionAnalysisResponse) -> Vec<FabricationArtifact> {
    let mut artifacts = vec![
        json_artifact(
            "analysis-validation-report".to_string(),
            "analysis-validation-report",
            json!(response.validation),
            response.generated_at_ms,
        ),
        json_artifact(
            "analysis-simulation-report".to_string(),
            "analysis-simulation-report",
            json!(response.simulation),
            response.generated_at_ms,
        ),
        json_artifact(
            "analysis-improvements".to_string(),
            "analysis-improvements",
            json!(response.improvements),
            response.generated_at_ms,
        ),
    ];

    artifacts.extend(
        response
            .improved_programs
            .iter()
            .map(|program| FabricationArtifact {
                artifact_id: artifact_id("improved-program", &program.program_id),
                kind: "improved-instruction-program".to_string(),
                media_type: "application/json".to_string(),
                part_id: None,
                program_id: Some(program.program_id.clone()),
                machine_kind: Some(program.machine_kind.clone()),
                draft: true,
                machine_ready: program.machine_ready,
                line_count: Some(program.instructions.len()),
                content: json!({
                    "language": program.language,
                    "changed": program.changed,
                    "sourceLineCount": program.source_line_count,
                    "instructions": program.instructions,
                }),
                notes: program.notes.clone(),
                created_at_ms: response.generated_at_ms,
            }),
    );
    artifacts
}

fn stored_plan_job(response: &FabricationPlanResponse) -> StoredFabricationJob {
    let artifacts = plan_artifacts(response)
        .into_iter()
        .map(|artifact| (artifact.artifact_id.clone(), artifact))
        .collect::<BTreeMap<_, _>>();
    let artifact_ids = artifacts.keys().cloned().collect::<Vec<_>>();
    StoredFabricationJob {
        record: FabricationJobRecord {
            job_id: response.job_id.clone(),
            request_id: response.request_id.clone(),
            kind: "fabrication-plan".to_string(),
            status: "complete".to_string(),
            ok: response.ok,
            severity: response.validation.severity.clone(),
            summary: summary_text(&response.objective),
            artifact_count: artifact_ids.len(),
            artifact_ids,
            created_at_ms: response.generated_at_ms,
            updated_at_ms: response.generated_at_ms,
        },
        plan: Some(response.clone()),
        analysis: None,
        learning: None,
        artifacts,
    }
}

fn stored_analysis_job(response: &InstructionAnalysisResponse) -> StoredFabricationJob {
    let artifacts = analysis_artifacts(response)
        .into_iter()
        .map(|artifact| (artifact.artifact_id.clone(), artifact))
        .collect::<BTreeMap<_, _>>();
    let artifact_ids = artifacts.keys().cloned().collect::<Vec<_>>();
    StoredFabricationJob {
        record: FabricationJobRecord {
            job_id: response.job_id.clone(),
            request_id: response.request_id.clone(),
            kind: "instruction-analysis".to_string(),
            status: "complete".to_string(),
            ok: response.ok,
            severity: response.validation.severity.clone(),
            summary: format!("{} program(s) analyzed", response.programs.len()),
            artifact_count: artifact_ids.len(),
            artifact_ids,
            created_at_ms: response.generated_at_ms,
            updated_at_ms: response.generated_at_ms,
        },
        plan: None,
        analysis: Some(response.clone()),
        learning: None,
        artifacts,
    }
}

fn report_severity(findings: &[ValidationFinding], boundaries: &[FailureBoundary]) -> String {
    if findings.iter().any(|finding| finding.severity == "error")
        || boundaries
            .iter()
            .any(|boundary| boundary.severity == "error")
    {
        "error".to_string()
    } else if findings.iter().any(|finding| finding.severity == "warning")
        || boundaries
            .iter()
            .any(|boundary| boundary.severity == "warning")
    {
        "warning".to_string()
    } else {
        "ok".to_string()
    }
}

fn canonical_policy_method(value: &str) -> Option<String> {
    let token = normalize_token(value);
    if wants_horizontal_milling(&token) {
        Some("horizontal-milling".to_string())
    } else if token.contains("router") || token.contains("routing") || token.contains("rout") {
        Some("routing".to_string())
    } else if wants_sheet_cutting(&token) || token.contains("sheet-cutting") {
        Some("sheet-cutting".to_string())
    } else if token.contains("turn") || token.contains("lathe") {
        Some("turning".to_string())
    } else if token.contains("mill") || token.contains("machin") {
        Some("milling".to_string())
    } else if token.contains("print") || token.contains("additive") || token.contains("fdm") {
        Some("additive-print".to_string())
    } else {
        None
    }
}

fn method_rank(method: &str) -> u8 {
    match method {
        "additive-print" => 0,
        "milling" => 1,
        "horizontal-milling" => 2,
        "routing" => 3,
        "sheet-cutting" => 4,
        "turning" => 5,
        _ => 100,
    }
}

fn canonical_policy_methods(values: &[String]) -> Vec<String> {
    let mut methods = values
        .iter()
        .filter_map(|value| canonical_policy_method(value))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    methods.sort_by(|left, right| {
        method_rank(left)
            .cmp(&method_rank(right))
            .then_with(|| left.cmp(right))
    });
    methods
}

fn method_combination_key(values: &[String]) -> Option<String> {
    let methods = canonical_policy_methods(values);
    if methods.len() > 1 {
        Some(methods.join("+"))
    } else {
        None
    }
}

fn learned_preferred_methods(policy: Option<&LearningPolicySnapshot>) -> Vec<String> {
    let mut methods = Vec::new();
    let Some(policy) = policy else {
        return methods;
    };
    for preference in &policy.method_preferences {
        if preference.recommendation != "prefer"
            || preference.samples < 2
            || preference.average_reward < 0.0
        {
            continue;
        }
        if let Some(method) = canonical_policy_method(&preference.key) {
            if !methods.contains(&method) {
                methods.push(method);
            }
        }
    }
    methods
}

fn learned_preferred_method_combination(policy: Option<&LearningPolicySnapshot>) -> Vec<String> {
    let Some(policy) = policy else {
        return Vec::new();
    };
    policy
        .method_combination_preferences
        .iter()
        .find(|preference| {
            preference.recommendation == "prefer"
                && preference.samples >= 2
                && preference.average_reward >= 0.0
        })
        .map(|preference| {
            preference
                .key
                .split('+')
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

fn learned_preferred_assembly_strategy(policy: Option<&LearningPolicySnapshot>) -> Option<String> {
    let policy = policy?;
    policy
        .assembly_preferences
        .iter()
        .find(|preference| {
            preference.recommendation == "prefer"
                && preference.samples >= 2
                && preference.average_reward >= 0.0
        })
        .map(|preference| preference.key.clone())
}

fn learned_part_description(method: &str) -> &'static str {
    match method {
        "additive-print" => "learned additive component inferred from successful hybrid outcomes",
        "milling" => "learned milled datum, pocket, or precision face inferred from successful hybrid outcomes",
        "horizontal-milling" => {
            "learned horizontal-milled side slot or keyway inferred from successful hybrid outcomes"
        }
        "routing" => "learned routed sheet/profile component inferred from successful hybrid outcomes",
        "sheet-cutting" => {
            "learned kerf-controlled sheet-cut component inferred from successful hybrid outcomes"
        }
        "turning" => {
            "learned turned shaft, bushing, threaded insert, or cylindrical component inferred from successful hybrid outcomes"
        }
        _ => "learned special-process component inferred from successful hybrid outcomes",
    }
}

fn learned_parts_for_method_combination(
    request: &FabricationPlanRequest,
    methods: &[String],
) -> Option<Vec<RequestedPart>> {
    if methods.len() < 2 {
        return None;
    }
    let parts = methods
        .iter()
        .take(MAX_PARTS)
        .map(|method| RequestedPart {
            id: format!("learned-{}-part", normalize_token(method)),
            description: learned_part_description(method).to_string(),
            material: request.material.clone(),
            preferred_method: Some(method.clone()),
            tolerance_mm: request.tolerance_mm,
        })
        .collect::<Vec<_>>();
    if parts.len() > 1 {
        Some(parts)
    } else {
        None
    }
}

fn apply_learning_policy_to_request(
    mut request: FabricationPlanRequest,
    policy: Option<&LearningPolicySnapshot>,
) -> FabricationPlanRequest {
    let learned_method_combination = learned_preferred_method_combination(policy);
    let learned_methods = if learned_method_combination.is_empty() {
        learned_preferred_methods(policy)
    } else {
        learned_method_combination.clone()
    };
    let learned_assembly_strategy = learned_preferred_assembly_strategy(policy);
    if learned_methods.is_empty() && learned_assembly_strategy.is_none() {
        return request;
    }

    let has_request_preferences = request
        .constraints
        .as_ref()
        .and_then(|constraints| constraints.preferred_methods.as_ref())
        .is_some_and(|methods| !methods.is_empty());
    if request.parts.is_none() && !has_request_preferences && learned_method_combination.len() > 1 {
        request.parts = learned_parts_for_method_combination(&request, &learned_method_combination);
    }
    let constraints = request
        .constraints
        .get_or_insert_with(|| FabricationConstraints {
            max_setups: None,
            allow_human_intervention: None,
            allow_multi_part_assembly: None,
            require_dry_run: None,
            preferred_methods: None,
            preferred_assembly_strategy: None,
        });
    if !has_request_preferences && !learned_methods.is_empty() {
        constraints.preferred_methods = Some(learned_methods.clone());
    }
    if constraints.preferred_assembly_strategy.is_none() {
        constraints.preferred_assembly_strategy = learned_assembly_strategy.clone();
    }
    if learned_assembly_strategy.is_some() && constraints.allow_multi_part_assembly.is_none() {
        constraints.allow_multi_part_assembly = Some(true);
    }

    let learning = request.learning.get_or_insert_with(|| LearningHints {
        policy_hint: None,
        model_family: None,
        reward_weights: None,
        observations: None,
        prior_successes: None,
    });
    if learning
        .policy_hint
        .as_ref()
        .map(|hint| hint.trim().is_empty())
        .unwrap_or(true)
    {
        let mut hint_parts = Vec::new();
        if !learned_methods.is_empty() {
            hint_parts.push(format!("methods={}", learned_methods.join("+")));
        }
        if let Some(strategy) = learned_assembly_strategy.as_ref() {
            hint_parts.push(format!("assembly={strategy}"));
        }
        learning.policy_hint = Some(format!("learned-policy-prefer:{}", hint_parts.join(";")));
    }
    if learning
        .prior_successes
        .as_ref()
        .map(|examples| examples.is_empty())
        .unwrap_or(true)
    {
        if let Some(policy) = policy {
            let examples = policy
                .neural_training_examples
                .iter()
                .take(8)
                .cloned()
                .collect::<Vec<_>>();
            if !examples.is_empty() {
                learning.prior_successes = Some(examples);
            }
        }
    }
    request
}

fn plan_fabrication_with_policy(
    request: FabricationPlanRequest,
    policy: Option<&LearningPolicySnapshot>,
) -> Result<FabricationPlanResponse, String> {
    plan_fabrication(apply_learning_policy_to_request(request, policy))
}

fn plan_fabrication(request: FabricationPlanRequest) -> Result<FabricationPlanResponse, String> {
    let request_id = request_id(request.request_id.as_ref(), "fabrication-plan");
    let objective = validate_text(&request.objective, "objective", MAX_TEXT_LEN)?;
    let material = material_or_default(request.material)?;
    let tolerance_mm = request
        .tolerance_mm
        .map(|value| finite_positive(value, "toleranceMm"))
        .transpose()?
        .unwrap_or(DEFAULT_TOLERANCE_MM);
    let quantity = request.quantity.unwrap_or(1);
    if quantity == 0 || quantity > 10_000 {
        return Err("quantity must be in [1, 10000]".to_string());
    }
    if let Some(stock) = request.stock.as_ref() {
        validate_text(&stock.form, "stock.form", MAX_LABEL_LEN)?;
        if let Some(dimensions) = stock.dimensions_mm.as_ref() {
            if dimensions.is_empty() || dimensions.len() > 4 {
                return Err("stock.dimensionsMm must have 1 to 4 values".to_string());
            }
            for dimension in dimensions {
                finite_positive(*dimension, "stock.dimensionsMm")?;
            }
        }
    }

    let machines = validate_machines(request.machines)?;
    let requested_parts =
        validate_request_parts(request.parts, &objective, &material, tolerance_mm)?;
    let existing_programs =
        validate_programs(request.existing_instructions.as_deref().unwrap_or_default())?;
    let constraints = request.constraints.as_ref();
    if let Some(strategy) =
        constraints.and_then(|constraints| constraints.preferred_assembly_strategy.as_ref())
    {
        validate_text(
            strategy,
            "constraints.preferredAssemblyStrategy",
            MAX_TEXT_LEN,
        )?;
    }

    let mut part_plans = Vec::with_capacity(requested_parts.len());
    let mut process_plan = Vec::with_capacity(requested_parts.len());
    let mut generated_programs = Vec::with_capacity(requested_parts.len());
    let mut warnings = Vec::new();
    let mut plan_boundaries = Vec::new();
    let mut machine_by_part = BTreeMap::new();

    for (index, part) in requested_parts.iter().enumerate() {
        let part_material = part.material.clone().unwrap_or_else(|| material.clone());
        let part_tolerance = part.tolerance_mm.unwrap_or(tolerance_mm);
        let machine = choose_machine(part, &machines, &part_material, constraints);
        let class = machine_class(&machine.kind);
        let method = part_method(class).to_string();

        if !material_supported(machine, &part_material) {
            warnings.push(format!(
                "machine {} does not explicitly list support for material {}",
                machine.id, part_material.name
            ));
        }
        if part_tolerance <= 0.05 {
            plan_boundaries.push(FailureBoundary {
                kind: "inspection-gate".to_string(),
                severity: "warning".to_string(),
                program_id: None,
                line: None,
                reason: format!(
                    "part {} requests tolerance {:.3} mm, which needs metrology feedback",
                    part.id, part_tolerance
                ),
                requires_human_intervention: true,
                suggested_resolution:
                    "add in-process probing, first-article inspection, or an explicit human measurement checkpoint"
                        .to_string(),
            });
        }
        if constraints.and_then(|constraints| constraints.allow_human_intervention) == Some(false)
            && matches!(
                class,
                MachineClass::Mill
                    | MachineClass::Lathe
                    | MachineClass::Router
                    | MachineClass::SheetCut
            )
        {
            plan_boundaries.push(FailureBoundary {
                kind: "automation-boundary".to_string(),
                severity: "warning".to_string(),
                program_id: None,
                line: None,
                reason: format!(
                    "part {} is assigned to {}, which often requires tool, workholding, or inspection intervention",
                    part.id, machine.kind
                ),
                requires_human_intervention: true,
                suggested_resolution:
                    "split the process into certified automated cells or relax the no-human-intervention constraint"
                        .to_string(),
            });
        }

        let part_plan = PartPlan {
            id: part.id.clone(),
            role: part.description.clone(),
            material: part_material,
            manufacturing_method: method,
            machine_kind: machine.kind.clone(),
            tolerance_mm: part_tolerance,
            interfaces: if requested_parts.len() > 1 {
                vec![format!("{}-assembly-interface", part.id)]
            } else {
                Vec::new()
            },
        };
        let generated = generate_program(&part_plan, machine);
        process_plan.push(ProcessStep {
            step: index as u32 + 1,
            part_id: part_plan.id.clone(),
            machine_id: machine.id.clone(),
            machine_kind: machine.kind.clone(),
            operation: operation_for_part(&part_plan).to_string(),
            setup: if matches!(class, MachineClass::Additive) {
                "single additive setup with material-specific slicing".to_string()
            } else {
                "operator-verified stock, tool, work offset, and dry-run setup".to_string()
            },
            expected_minutes: expected_minutes(class, part_tolerance),
            requires_human_intervention: !matches!(class, MachineClass::Additive),
            notes: generated.safety_notes.clone(),
        });
        machine_by_part.insert(part_plan.id.clone(), machine.clone());
        part_plans.push(part_plan);
        generated_programs.push(generated);
    }

    if let Some(max_setups) = constraints.and_then(|constraints| constraints.max_setups) {
        if max_setups == 0 {
            return Err("constraints.maxSetups must be positive when provided".to_string());
        }
        if process_plan.len() as u32 > max_setups {
            plan_boundaries.push(FailureBoundary {
                kind: "setup-limit".to_string(),
                severity: "warning".to_string(),
                program_id: None,
                line: None,
                reason: format!(
                    "plan uses {} setups, exceeding requested maxSetups={max_setups}",
                    process_plan.len()
                ),
                requires_human_intervention: true,
                suggested_resolution:
                    "combine compatible operations, split the request into separate jobs, or relax maxSetups"
                        .to_string(),
            });
        }
    }
    if constraints
        .and_then(|constraints| constraints.require_dry_run)
        .unwrap_or(false)
    {
        warnings.push(
            "requireDryRun=true: generated programs remain draft-only until simulation or dry-run evidence is attached"
                .to_string(),
        );
    }
    if let Some(stock_dimensions) = request
        .stock
        .as_ref()
        .and_then(|stock| stock.dimensions_mm.as_ref())
    {
        for step in &process_plan {
            let Some(machine) = machine_by_part.get(&step.part_id) else {
                continue;
            };
            let Some(work_envelope) = machine.work_envelope_mm.as_ref() else {
                continue;
            };
            for (axis_index, stock, limit) in
                stock_envelope_excesses(stock_dimensions, work_envelope)
            {
                plan_boundaries.push(FailureBoundary {
                    kind: "machine-envelope".to_string(),
                    severity: "error".to_string(),
                    program_id: None,
                    line: None,
                    reason: format!(
                        "part {} stock dimension axis {} is {:.3} mm, exceeding machine {} envelope {:.3} mm",
                        step.part_id, axis_index, stock, machine.id, limit
                    ),
                    requires_human_intervention: true,
                    suggested_resolution:
                        "split the part, choose a larger machine, revise stock prep, or add an explicit fixture/assembly plan"
                            .to_string(),
                });
            }
        }
    }

    let generated_as_input = generated_programs
        .iter()
        .map(|program| InstructionProgram {
            id: Some(program.program_id.clone()),
            machine_id: Some(program.machine_id.clone()),
            machine_kind: Some(program.machine_kind.clone()),
            language: Some(program.language.clone()),
            instructions: program.instructions.clone(),
        })
        .chain(existing_programs.clone())
        .collect::<Vec<_>>();
    let (_, mut validation, improvements) = analyze_instruction_programs(&generated_as_input);
    let simulation = simulate_instruction_programs(&generated_as_input, &machines);
    validation.findings.extend(simulation.findings.clone());
    validation.failure_boundaries.extend(plan_boundaries);
    validation
        .failure_boundaries
        .extend(simulation.failure_boundaries.clone());
    validation.severity = report_severity(&validation.findings, &validation.failure_boundaries);
    validation.ok = validation.severity != "error";

    let assembly = assembly_plan(&part_plans, constraints);
    let learning = learning_plan(
        request.learning.as_ref(),
        constraints,
        &part_plans,
        &process_plan,
        &validation,
        &improvements,
    )?;
    let generated_at_ms = now_ms();
    let job_id = safe_job_id("plan", &request_id, generated_at_ms);

    Ok(FabricationPlanResponse {
        ok: validation.ok,
        job_id,
        request_id,
        schema_version: SCHEMA_VERSION,
        objective,
        material,
        quantity,
        design: DesignSummary {
            representation: "parametric-csg-plus-process-features-v1".to_string(),
            object_id: "generated-fabrication-object".to_string(),
            parts: part_plans,
            join_strategy: assembly.strategy.clone(),
            manufacturability_notes: vec![
                "Design is a planning envelope, not a final mesh or certified CAM output"
                    .to_string(),
                "Generated machine programs are draft review artifacts and are not machine-ready"
                    .to_string(),
            ],
        },
        process_plan,
        generated_programs,
        validation,
        simulation,
        assembly,
        learning,
        warnings,
        generated_at_ms,
    })
}

fn assembly_plan(parts: &[PartPlan], constraints: Option<&FabricationConstraints>) -> AssemblyPlan {
    let allow_multi_part = constraints
        .and_then(|constraints| constraints.allow_multi_part_assembly)
        .unwrap_or(true);
    let preferred_assembly_strategy = constraints
        .and_then(|constraints| constraints.preferred_assembly_strategy.as_ref())
        .filter(|strategy| !strategy.trim().is_empty());
    let methods = parts
        .iter()
        .map(|part| part.manufacturing_method.as_str())
        .collect::<BTreeSet<_>>();

    let mut combine_candidates = if methods.len() > 1 {
        vec![
            "combine printed shells with machined datum inserts when tolerance stack allows"
                .to_string(),
            "merge low-load printed covers into one print job to avoid unnecessary fasteners"
                .to_string(),
        ]
    } else {
        Vec::new()
    };
    if allow_multi_part && preferred_assembly_strategy.is_some() {
        let strategy = preferred_assembly_strategy.unwrap();
        combine_candidates.push(format!(
            "reuse learned assembly strategy when interfaces permit: {strategy}"
        ));
    }
    let split_candidates = parts
        .iter()
        .filter(|part| {
            part.tolerance_mm <= 0.08
                || matches!(machine_class(&part.machine_kind), MachineClass::Mill | MachineClass::Lathe)
        })
        .map(|part| {
            format!(
                "split {} from cosmetic/additive geometry so tight features can be machined and inspected",
                part.id
            )
        })
        .collect::<Vec<_>>();
    let joints = parts
        .iter()
        .flat_map(|part| part.interfaces.iter().cloned())
        .collect::<Vec<_>>();

    let mut notes = vec![
        "Assembly choices should be promoted into CAD constraints before final CAM generation"
            .to_string(),
        "Every join interface needs tolerance stack-up and access-path validation".to_string(),
    ];
    if let Some(strategy) = preferred_assembly_strategy {
        notes.push(format!(
            "Learned policy prefers assembly strategy: {strategy}"
        ));
    }

    AssemblyPlan {
        strategy: if parts.len() == 1 {
            "single-part fabrication".to_string()
        } else if !allow_multi_part {
            "single-piece preference; review split candidates before approving".to_string()
        } else if let Some(strategy) = preferred_assembly_strategy {
            format!("learned hybrid assembly strategy: {strategy}")
        } else {
            "multi-part hybrid fabrication with explicit assembly interfaces".to_string()
        },
        combine_candidates,
        split_candidates,
        joints,
        notes,
    }
}

fn clamp_unit(value: f64) -> f64 {
    value.clamp(0.0, 1.0)
}

fn sigmoid(value: f64) -> f64 {
    1.0 / (1.0 + (-value).exp())
}

fn neural_policy_sketch(
    model_family: &str,
    actions: &[String],
    parts: &[PartPlan],
    process_plan: &[ProcessStep],
    validation: &ValidationReport,
    improvements: &[InstructionImprovement],
) -> NeuralPolicySketch {
    let method_count = parts
        .iter()
        .map(|part| part.manufacturing_method.as_str())
        .collect::<BTreeSet<_>>()
        .len();
    let human_intervention_steps = process_plan
        .iter()
        .filter(|step| step.requires_human_intervention)
        .count();
    let min_tolerance = parts
        .iter()
        .map(|part| part.tolerance_mm)
        .fold(DEFAULT_TOLERANCE_MM, f64::min);
    let error_count = validation
        .findings
        .iter()
        .filter(|finding| finding.severity == "error")
        .count()
        + validation
            .failure_boundaries
            .iter()
            .filter(|boundary| boundary.severity == "error")
            .count();
    let human_boundary_count = validation
        .failure_boundaries
        .iter()
        .filter(|boundary| boundary.requires_human_intervention)
        .count();

    let feature_vector = vec![
        clamp_unit(parts.len() as f64 / MAX_PARTS as f64),
        clamp_unit(method_count as f64 / 6.0),
        clamp_unit(human_intervention_steps as f64 / process_plan.len().max(1) as f64),
        clamp_unit(validation.findings.len() as f64 / 12.0),
        clamp_unit(human_boundary_count as f64 / 12.0),
        clamp_unit(improvements.len() as f64 / 12.0),
        clamp_unit((DEFAULT_TOLERANCE_MM / min_tolerance.max(0.01)) / 10.0),
    ];
    let hidden_activations = vec![
        sigmoid(feature_vector[0] * 1.2 + feature_vector[1] * 0.9 + feature_vector[6] * 0.6 - 0.8),
        sigmoid(feature_vector[2] * 1.4 + feature_vector[4] * 1.1 + feature_vector[5] * 0.7 - 0.6),
        sigmoid(feature_vector[3] * 1.6 + error_count as f64 * 0.35 - 0.5),
    ];
    let action_scores = actions
        .iter()
        .map(|action| {
            let score = if action.contains("reject") {
                hidden_activations[2]
            } else if action.contains("human") || action.contains("inspection") {
                hidden_activations[1]
            } else if action.contains("split") || action.contains("combine") {
                hidden_activations[0]
            } else if action.contains("learned") {
                (hidden_activations[0] + hidden_activations[1]) / 2.0
            } else if action.contains("assign") {
                1.0 - hidden_activations[2] * 0.5
            } else {
                0.5 + hidden_activations[0] * 0.25 - hidden_activations[2] * 0.25
            };
            NeuralActionScore {
                action: action.clone(),
                score: (clamp_unit(score) * 1000.0).round() / 1000.0,
            }
        })
        .collect::<Vec<_>>();

    NeuralPolicySketch {
        schema_version: "dd.fabrication.neural-policy-sketch.v1".to_string(),
        model_family: model_family.to_string(),
        feature_vector,
        hidden_activations,
        action_scores,
        notes: vec![
            "Deterministic neural-network sketch for downstream training or replacement by an external model"
                .to_string(),
            "Inputs are normalized plan, validation, intervention, tolerance, and improvement features"
                .to_string(),
        ],
    }
}

fn learning_plan(
    hints: Option<&LearningHints>,
    constraints: Option<&FabricationConstraints>,
    parts: &[PartPlan],
    process_plan: &[ProcessStep],
    validation: &ValidationReport,
    improvements: &[InstructionImprovement],
) -> Result<LearningPlan, String> {
    let model_family = hints
        .and_then(|hints| hints.model_family.as_ref())
        .map(|value| validate_text(value, "learning.modelFamily", MAX_LABEL_LEN))
        .transpose()?
        .unwrap_or_else(|| "hybrid-mdp-pomdp-neural-policy".to_string());
    if let Some(policy_hint) = hints.and_then(|hints| hints.policy_hint.as_ref()) {
        validate_text(policy_hint, "learning.policyHint", MAX_TEXT_LEN)?;
    }
    if let Some(reward_weights) = hints.and_then(|hints| hints.reward_weights.as_ref()) {
        for (name, value) in reward_weights {
            validate_text(name, "learning.rewardWeights key", MAX_LABEL_LEN)?;
            if !value.is_finite() {
                return Err("learning.rewardWeights values must be finite".to_string());
            }
        }
    }

    let mut actions = vec![
        "choose-additive-process".to_string(),
        "choose-milling-process".to_string(),
        "choose-routing-process".to_string(),
        "choose-sheet-cutting-process".to_string(),
        "choose-turning-process".to_string(),
        "split-part".to_string(),
        "combine-parts".to_string(),
        "insert-human-inspection".to_string(),
        "reject-or-repostprocess-program".to_string(),
    ];
    for part in parts {
        actions.push(format!(
            "assign-{}-to-{}",
            part.id,
            normalize_token(&part.machine_kind)
        ));
    }
    if let Some(methods) =
        constraints.and_then(|constraints| constraints.preferred_methods.as_ref())
    {
        let canonical_methods = canonical_policy_methods(methods);
        if canonical_methods.len() > 1 {
            actions.push(format!(
                "prefer-learned-method-combination-{}",
                normalize_token(&canonical_methods.join("-"))
            ));
        }
    }
    if let Some(strategy) =
        constraints.and_then(|constraints| constraints.preferred_assembly_strategy.as_ref())
    {
        actions.push(format!(
            "prefer-learned-assembly-{}",
            normalize_token(strategy)
        ));
    }
    actions.sort();
    actions.dedup();

    let observations = hints
        .and_then(|hints| hints.observations.clone())
        .unwrap_or_else(|| {
            vec![
                "machine-state".to_string(),
                "tool-wear".to_string(),
                "thermal-state".to_string(),
                "material-batch".to_string(),
                "first-article-measurement".to_string(),
                "operator-intervention".to_string(),
            ]
        });

    let training_examples = hints
        .and_then(|hints| hints.prior_successes.clone())
        .unwrap_or_else(|| {
            process_plan
                .iter()
                .map(|step| {
                    format!(
                        "{}:{}:{}:{}",
                        step.part_id, step.machine_kind, step.operation, step.expected_minutes
                    )
                })
                .collect()
        });
    let neural_policy = neural_policy_sketch(
        &model_family,
        &actions,
        parts,
        process_plan,
        validation,
        improvements,
    );

    Ok(LearningPlan {
        model_family,
        mdp_states: vec![
            "design-proposed".to_string(),
            "process-selected".to_string(),
            "program-generated".to_string(),
            "simulation-passed".to_string(),
            "inspection-required".to_string(),
            "assembly-required".to_string(),
            "complete".to_string(),
            "failed".to_string(),
        ],
        pomdp_observations: observations,
        actions,
        reward_terms: vec![
            "successful-completion".to_string(),
            "surface-finish".to_string(),
            "dimensional-accuracy".to_string(),
            "setup-count".to_string(),
            "human-intervention-cost".to_string(),
            "scrap-risk".to_string(),
            "machine-time".to_string(),
            format!("validation-findings-{}", validation.findings.len()),
            format!("improvement-opportunities-{}", improvements.len()),
        ],
        neural_features: vec![
            "objective-embedding".to_string(),
            "material-family".to_string(),
            "stock-envelope".to_string(),
            "machine-envelope".to_string(),
            "toolpath-token-sequence".to_string(),
            "simulated-force-temperature-vibration".to_string(),
            "inspection-error-vector".to_string(),
        ],
        neural_policy,
        training_examples,
    })
}

fn validate_reward_weights(weights: Option<&BTreeMap<String, f64>>) -> Result<(), String> {
    if let Some(weights) = weights {
        if weights.len() > MAX_LEARNING_SIGNALS {
            return Err(format!(
                "rewardWeights must contain at most {MAX_LEARNING_SIGNALS} entries"
            ));
        }
        for (name, value) in weights {
            validate_label(name, "rewardWeights key")?;
            if !value.is_finite() {
                return Err("rewardWeights values must be finite".to_string());
            }
        }
    }
    Ok(())
}

fn outcome_reward_weight(
    weights: Option<&BTreeMap<String, f64>>,
    name: &str,
    fallback: f64,
) -> Result<f64, String> {
    match weights.and_then(|weights| weights.get(name)) {
        Some(weight) if weight.is_finite() => Ok(*weight),
        Some(_) => Err(format!("rewardWeights.{name} must be finite")),
        None => Ok(fallback),
    }
}

fn outcome_reward_term(name: &str, value: f64, weight: f64) -> LearningRewardTerm {
    LearningRewardTerm {
        name: name.to_string(),
        value,
        weight,
        contribution: value * weight,
    }
}

fn process_method_for_machine(machine_kind: Option<&String>) -> String {
    machine_kind
        .map(|kind| match machine_class(kind) {
            MachineClass::Additive => "additive-print",
            MachineClass::Mill => "milling",
            MachineClass::Lathe => "turning",
            MachineClass::Router => "routing",
            MachineClass::SheetCut => "sheet-cutting",
            MachineClass::Other => "unknown-process",
        })
        .unwrap_or("unknown-process")
        .to_string()
}

fn learn_from_outcome(
    request: FabricationOutcomeRequest,
) -> Result<(FabricationLearningResponse, LearningOutcomeRecord), String> {
    let request_id = request_id(request.request_id.as_ref(), "fabrication-outcome");
    let outcome = validate_text(&request.outcome, "outcome", MAX_TEXT_LEN)?;
    let source_job_id = validate_optional_label(request.source_job_id, "sourceJobId")?;
    let source_artifact_id =
        validate_optional_label(request.source_artifact_id, "sourceArtifactId")?;
    let part_id = validate_optional_label(request.part_id, "partId")?;
    let program_id = validate_optional_label(request.program_id, "programId")?;
    let machine_id = validate_optional_label(request.machine_id, "machineId")?;
    let machine_kind = validate_optional_label(request.machine_kind, "machineKind")?;
    let material = request
        .material
        .map(|material| material_or_default(Some(material)))
        .transpose()?;
    let intervention_minutes = request
        .intervention_minutes
        .map(|value| finite_non_negative(value, "interventionMinutes"))
        .transpose()?;
    let duration_minutes = request
        .duration_minutes
        .map(|value| finite_non_negative(value, "durationMinutes"))
        .transpose()?;
    let dimensional_error_mm = request
        .dimensional_error_mm
        .map(|value| finite_non_negative(value, "dimensionalErrorMm"))
        .transpose()?;
    let surface_quality = request
        .surface_quality
        .map(|value| finite_ratio(value, "surfaceQuality"))
        .transpose()?;
    let reward_weights = request.reward_weights;
    validate_reward_weights(reward_weights.as_ref())?;
    let notes = validate_signal_list(request.notes, "notes", MAX_TEXT_LEN)?;
    let mut observations =
        validate_signal_list(request.observations, "observations", MAX_TEXT_LEN)?;
    let observed_text = normalize_token(&format!("{} {}", outcome, observations.join(" ")));
    let completed = request.completed.unwrap_or_else(|| {
        observed_text.contains("complete")
            || observed_text.contains("success")
            || observed_text.contains("pass")
    });
    let machine_failure = request.machine_failure.unwrap_or_else(|| {
        observed_text.contains("fail")
            || observed_text.contains("alarm")
            || observed_text.contains("crash")
    });
    let scrap = request
        .scrap
        .unwrap_or_else(|| observed_text.contains("scrap") || observed_text.contains("reject"));
    let human_intervention_required = request.human_intervention_required.unwrap_or_else(|| {
        intervention_minutes.unwrap_or(0.0) > 0.0
            || observed_text.contains("manual")
            || observed_text.contains("operator")
            || observed_text.contains("intervention")
    });
    if observations.is_empty() {
        observations.push(format!("outcome:{}", normalize_token(&outcome)));
    }
    if completed {
        observations.push("completed".to_string());
    }
    if machine_failure {
        observations.push("machine-failure".to_string());
    }
    if scrap {
        observations.push("scrap".to_string());
    }
    if human_intervention_required {
        observations.push("human-intervention-required".to_string());
    }
    if let Some(error_mm) = dimensional_error_mm {
        observations.push(format!("dimensional-error-mm:{error_mm:.4}"));
    }
    if let Some(quality) = surface_quality {
        observations.push(format!("surface-quality:{quality:.3}"));
    }
    observations.sort();
    observations.dedup();

    let completion_value = if completed { 1.0 } else { -0.5 };
    let machine_failure_value = if machine_failure { -1.0 } else { 0.0 };
    let scrap_value = if scrap { -1.0 } else { 0.0 };
    let intervention_value = if human_intervention_required {
        -bounded(intervention_minutes.unwrap_or(15.0) / 120.0, 0.0, 1.0)
    } else {
        0.0
    };
    let dimensional_value = dimensional_error_mm
        .map(|error| 1.0 - bounded(error / DEFAULT_TOLERANCE_MM.max(0.001), 0.0, 2.0))
        .unwrap_or(0.0);
    let surface_value = surface_quality.map(|quality| quality - 0.5).unwrap_or(0.0);
    let duration_value = duration_minutes
        .map(|minutes| -bounded(minutes / 480.0, 0.0, 1.0))
        .unwrap_or(0.0);
    let reward_terms = vec![
        outcome_reward_term(
            "successfulCompletion",
            completion_value,
            outcome_reward_weight(reward_weights.as_ref(), "successfulCompletion", 2.0)?,
        ),
        outcome_reward_term(
            "machineFailure",
            machine_failure_value,
            outcome_reward_weight(reward_weights.as_ref(), "machineFailure", 3.0)?,
        ),
        outcome_reward_term(
            "scrapRisk",
            scrap_value,
            outcome_reward_weight(reward_weights.as_ref(), "scrapRisk", 2.0)?,
        ),
        outcome_reward_term(
            "humanInterventionCost",
            intervention_value,
            outcome_reward_weight(reward_weights.as_ref(), "humanInterventionCost", 1.0)?,
        ),
        outcome_reward_term(
            "dimensionalAccuracy",
            dimensional_value,
            outcome_reward_weight(reward_weights.as_ref(), "dimensionalAccuracy", 1.5)?,
        ),
        outcome_reward_term(
            "surfaceQuality",
            surface_value,
            outcome_reward_weight(reward_weights.as_ref(), "surfaceQuality", 1.0)?,
        ),
        outcome_reward_term(
            "machineTime",
            duration_value,
            outcome_reward_weight(reward_weights.as_ref(), "machineTime", 0.5)?,
        ),
    ];
    let reward = reward_terms
        .iter()
        .map(|term| term.contribution)
        .sum::<f64>();
    let state = if machine_failure || scrap {
        "failed"
    } else if human_intervention_required {
        "inspection-required"
    } else if completed {
        "complete"
    } else {
        "program-generated"
    }
    .to_string();
    let recommended_action = if machine_failure || scrap {
        "reject-or-repostprocess-program"
    } else if human_intervention_required {
        "insert-human-inspection"
    } else if dimensional_error_mm.is_some_and(|error| error > DEFAULT_TOLERANCE_MM) {
        "split-part"
    } else if completed {
        "reuse-successful-policy"
    } else {
        "continue-fabrication-or-simulation"
    }
    .to_string();
    let ok = completed && !machine_failure && !scrap;
    let generated_at_ms = now_ms();
    let job_id = safe_job_id("learning", &request_id, generated_at_ms);
    let method = process_method_for_machine(machine_kind.as_ref());
    let material_name = material.as_ref().map(|material| material.name.clone());
    let material_family = material
        .as_ref()
        .and_then(|material| material.family.clone());
    let mdp_update = json!({
        "schemaVersion": "dd.fabrication.learning-experience.v1",
        "requestId": request_id,
        "jobId": job_id,
        "sourceJobId": source_job_id,
        "sourceArtifactId": source_artifact_id,
        "partId": part_id,
        "programId": program_id,
        "machineId": machine_id,
        "machineKind": machine_kind,
        "state": "program-generated",
        "action": recommended_action,
        "reward": reward,
        "nextState": state,
        "terminal": completed || machine_failure || scrap,
        "observations": observations,
        "rewardTerms": reward_terms,
    });
    let neural_example = json!({
        "schemaVersion": "dd.fabrication.neural-example.v1",
        "features": {
            "machineKind": machine_kind,
            "manufacturingMethod": method,
            "materialName": material_name,
            "materialFamily": material_family,
            "completed": completed,
            "machineFailure": machine_failure,
            "scrap": scrap,
            "humanInterventionRequired": human_intervention_required,
            "interventionMinutes": intervention_minutes,
            "durationMinutes": duration_minutes,
            "dimensionalErrorMm": dimensional_error_mm,
            "surfaceQuality": surface_quality,
            "observations": observations,
            "notes": notes,
        },
        "labels": {
            "reward": reward,
            "state": state,
            "recommendedAction": recommended_action,
            "ok": ok,
        }
    });
    let mut warnings = Vec::new();
    if machine_failure || scrap {
        warnings.push(
            "Outcome marks a failed fabrication attempt; generated reward is intentionally negative"
                .to_string(),
        );
    }
    if !completed && !machine_failure && !scrap {
        warnings.push(
            "Outcome is non-terminal; policy update should be treated as partial evidence"
                .to_string(),
        );
    }
    let response = FabricationLearningResponse {
        ok,
        job_id: job_id.clone(),
        request_id: request_id.clone(),
        source_job_id: mdp_update
            .get("sourceJobId")
            .and_then(Value::as_str)
            .map(str::to_string),
        source_artifact_id: mdp_update
            .get("sourceArtifactId")
            .and_then(Value::as_str)
            .map(str::to_string),
        outcome: outcome.clone(),
        state: state.clone(),
        recommended_action: recommended_action.clone(),
        reward,
        reward_terms,
        observations: mdp_update
            .get("observations")
            .and_then(Value::as_array)
            .map(|values| {
                values
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default(),
        mdp_update,
        neural_example,
        warnings,
        generated_at_ms,
    };
    let record = LearningOutcomeRecord {
        outcome_id: job_id,
        request_id,
        job_id: response.source_job_id.clone(),
        objective: Some(summary_text(&outcome)),
        material,
        manufacturing_methods: vec![method],
        assembly_strategy: Some(recommended_action),
        success: response.ok,
        reward,
        observations: response.observations.clone(),
        notes,
        created_at_ms: generated_at_ms,
    };
    Ok((response, record))
}

fn learning_outcome_record(
    request: LearningOutcomeRequest,
) -> Result<LearningOutcomeRecord, String> {
    let request_id = request_id(request.request_id.as_ref(), "learning-outcome");
    let job_id = validate_optional_label(request.job_id, "jobId")?;
    let objective = validate_optional_text(request.objective, "objective", MAX_TEXT_LEN)?;
    let material = request
        .material
        .map(|material| material_or_default(Some(material)))
        .transpose()?;
    let manufacturing_methods = validate_signal_list(
        request.manufacturing_methods,
        "manufacturingMethods",
        MAX_LABEL_LEN,
    )?;
    let manufacturing_methods = if manufacturing_methods.is_empty() {
        vec!["unknown-process".to_string()]
    } else {
        manufacturing_methods
            .into_iter()
            .map(|method| normalize_token(&method))
            .collect()
    };
    let assembly_strategy =
        validate_optional_text(request.assembly_strategy, "assemblyStrategy", MAX_TEXT_LEN)?;
    let reward = request
        .reward
        .map(|value| {
            if value.is_finite() {
                Ok(value)
            } else {
                Err("reward must be finite".to_string())
            }
        })
        .transpose()?
        .unwrap_or(if request.success { 1.0 } else { -1.0 });
    let observations = validate_signal_list(request.observations, "observations", MAX_TEXT_LEN)?;
    let notes = validate_signal_list(request.notes, "notes", MAX_TEXT_LEN)?;
    let created_at_ms = now_ms();
    Ok(LearningOutcomeRecord {
        outcome_id: safe_job_id("outcome", &request_id, created_at_ms),
        request_id,
        job_id,
        objective,
        material,
        manufacturing_methods,
        assembly_strategy,
        success: request.success,
        reward,
        observations,
        notes,
        created_at_ms,
    })
}

fn learning_artifacts(response: &FabricationLearningResponse) -> Vec<FabricationArtifact> {
    vec![
        json_artifact(
            "outcome-learning-event".to_string(),
            "outcome-learning-event",
            json!(response),
            response.generated_at_ms,
        ),
        json_artifact(
            "reward-signal".to_string(),
            "reward-signal",
            json!({
                "reward": response.reward,
                "terms": &response.reward_terms,
                "state": response.state,
                "recommendedAction": response.recommended_action,
            }),
            response.generated_at_ms,
        ),
        json_artifact(
            "mdp-experience".to_string(),
            "mdp-experience",
            response.mdp_update.clone(),
            response.generated_at_ms,
        ),
        json_artifact(
            "pomdp-observations".to_string(),
            "pomdp-observations",
            json!({
                "observations": &response.observations,
                "state": response.state,
                "sourceJobId": response.source_job_id,
                "sourceArtifactId": response.source_artifact_id,
            }),
            response.generated_at_ms,
        ),
        json_artifact(
            "neural-example".to_string(),
            "neural-example",
            response.neural_example.clone(),
            response.generated_at_ms,
        ),
    ]
}

fn stored_learning_job(response: &FabricationLearningResponse) -> StoredFabricationJob {
    let artifacts = learning_artifacts(response)
        .into_iter()
        .map(|artifact| (artifact.artifact_id.clone(), artifact))
        .collect::<BTreeMap<_, _>>();
    let artifact_ids = artifacts.keys().cloned().collect::<Vec<_>>();
    StoredFabricationJob {
        record: FabricationJobRecord {
            job_id: response.job_id.clone(),
            request_id: response.request_id.clone(),
            kind: "fabrication-learning-outcome".to_string(),
            status: response.state.clone(),
            ok: response.ok,
            severity: if response.ok { "ok" } else { "warning" }.to_string(),
            summary: summary_text(&response.outcome),
            artifact_count: artifact_ids.len(),
            artifact_ids,
            created_at_ms: response.generated_at_ms,
            updated_at_ms: response.generated_at_ms,
        },
        plan: None,
        analysis: None,
        learning: Some(response.clone()),
        artifacts,
    }
}

fn store_job(state: &AppState, job: StoredFabricationJob) {
    let artifact_count = job.artifacts.len() as u64;
    match state.jobs.write() {
        Ok(mut jobs) => {
            jobs.insert(job);
            state
                .metrics
                .jobs_stored_total
                .fetch_add(1, Ordering::Relaxed);
            state
                .metrics
                .artifacts_stored_total
                .fetch_add(artifact_count, Ordering::Relaxed);
        }
        Err(error) => {
            state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
            eprintln!("{SERVICE_NAME} job store lock failed: {error}");
        }
    }
}

fn store_plan_response(state: &AppState, response: &FabricationPlanResponse) {
    store_job(state, stored_plan_job(response));
}

fn store_analysis_response(state: &AppState, response: &InstructionAnalysisResponse) {
    store_job(state, stored_analysis_job(response));
}

fn store_learning_record(
    state: &AppState,
    record: LearningOutcomeRecord,
) -> Result<LearningPolicySnapshot, String> {
    match state.learning.write() {
        Ok(mut learning) => {
            learning.insert(record);
            state
                .metrics
                .learning_events_stored_total
                .fetch_add(1, Ordering::Relaxed);
            Ok(learning.snapshot())
        }
        Err(error) => {
            state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
            Err(format!("learning memory lock failed: {error}"))
        }
    }
}

fn store_learning_response(
    state: &AppState,
    response: &FabricationLearningResponse,
    record: LearningOutcomeRecord,
) -> Result<LearningPolicySnapshot, String> {
    store_job(state, stored_learning_job(response));
    store_learning_record(state, record)
}

fn learning_policy_snapshot(state: &AppState) -> Result<LearningPolicySnapshot, String> {
    match state.learning.read() {
        Ok(learning) => Ok(learning.snapshot()),
        Err(error) => {
            state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
            Err(format!("learning memory lock failed: {error}"))
        }
    }
}

async fn publish_event(state: &AppState, event_type: &str, request_id: &str, ok: bool) {
    let Some(nats) = state.nats.as_ref() else {
        return;
    };
    let payload = json!({
        "schema": "dd.log.v1",
        "source": SERVICE_NAME,
        "type": event_type,
        "requestId": request_id,
        "ok": ok,
        "generatedAtMs": now_ms(),
    });
    match nats
        .publish(state.event_subject.clone(), payload.to_string().into())
        .await
    {
        Ok(()) => {
            state
                .metrics
                .nats_published_total
                .fetch_add(1, Ordering::Relaxed);
        }
        Err(error) => {
            state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
            eprintln!("{SERVICE_NAME} failed to publish runtime event: {error}");
        }
    }
}

async fn publish_json_to_nats(state: &AppState, subject: &str, payload: Value) -> bool {
    let Some(nats) = state.nats.as_ref() else {
        return false;
    };
    match nats
        .publish(subject.to_string(), payload.to_string().into())
        .await
    {
        Ok(()) => {
            state
                .metrics
                .nats_published_total
                .fetch_add(1, Ordering::Relaxed);
            true
        }
        Err(error) => {
            state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
            eprintln!("{SERVICE_NAME} failed to publish to {subject}: {error}");
            false
        }
    }
}

fn fabrication_mdp_request(response: &FabricationPlanResponse) -> Value {
    let states = response.learning.mdp_states.clone();
    let actions = response.learning.actions.clone();
    let final_states = ["complete", "failed"];
    let mut transitions = Vec::new();
    let mut rewards = Vec::new();

    for state in &states {
        for action in &actions {
            let target = if final_states.contains(&state.as_str()) {
                state.as_str()
            } else if action.contains("reject") {
                "failed"
            } else if action.contains("insert-human-inspection") {
                "inspection-required"
            } else if action.contains("combine") || action.contains("split") {
                "assembly-required"
            } else if action.contains("assign-") || action.contains("choose-") {
                "process-selected"
            } else {
                "program-generated"
            };
            transitions.push(json!({
                "state": state,
                "action": action,
                "nextState": target,
                "probability": 0.86
            }));
            if target != "failed" {
                transitions.push(json!({
                    "state": state,
                    "action": action,
                    "nextState": "failed",
                    "probability": 0.14
                }));
            }

            let reward = match target {
                "complete" => 4.0,
                "failed" => -5.0,
                "inspection-required" => 0.7,
                "assembly-required" => 1.0,
                "process-selected" => 1.5,
                "program-generated" => 2.0,
                _ => 0.2,
            } - if action.contains("human") { 0.7 } else { 0.0 };
            rewards.push(json!({
                "state": state,
                "action": action,
                "value": reward
            }));
        }
    }

    json!({
        "requestId": format!("{}-fabrication-policy", response.request_id),
        "kind": "fabrication.mdp.process-policy",
        "states": states,
        "actions": actions,
        "transitions": transitions,
        "rewards": rewards,
        "observations": response.learning.pomdp_observations,
        "gamma": 0.82,
        "tolerance": 0.000001,
        "maxIterations": 1000
    })
}

async fn publish_plan_outputs(state: &AppState, response: &FabricationPlanResponse) {
    let result = json!({
        "schemaVersion": SCHEMA_VERSION,
        "type": "fabrication.plan.result",
        "response": response,
    });
    if publish_json_to_nats(state, &state.result_subject, result).await {
        state
            .metrics
            .nats_results_published_total
            .fetch_add(1, Ordering::Relaxed);
    }
    if state.mdp_autopublish {
        let mdp_request = fabrication_mdp_request(response);
        if publish_json_to_nats(state, &state.mdp_subject, mdp_request).await {
            state
                .metrics
                .mdp_published_total
                .fetch_add(1, Ordering::Relaxed);
        }
    }
}

async fn publish_learning_outputs(state: &AppState, response: &FabricationLearningResponse) {
    let result = json!({
        "schemaVersion": "fabrication.learning.v1",
        "type": "fabrication.learning.result",
        "response": response,
    });
    if publish_json_to_nats(state, &state.result_subject, result).await {
        state
            .metrics
            .nats_results_published_total
            .fetch_add(1, Ordering::Relaxed);
    }
    if state.mdp_autopublish {
        if publish_json_to_nats(state, &state.mdp_subject, response.mdp_update.clone()).await {
            state
                .metrics
                .mdp_published_total
                .fetch_add(1, Ordering::Relaxed);
        }
    }
}

fn record_plan_metrics(state: &AppState, response: &FabricationPlanResponse) {
    state
        .metrics
        .generated_programs_total
        .fetch_add(response.generated_programs.len() as u64, Ordering::Relaxed);
    state
        .metrics
        .validation_findings_total
        .fetch_add(response.validation.findings.len() as u64, Ordering::Relaxed);
    state.metrics.failure_boundaries_total.fetch_add(
        response.validation.failure_boundaries.len() as u64,
        Ordering::Relaxed,
    );
}

async fn run_nats_loop(state: AppState) {
    let Some(nats) = state.nats.clone() else {
        println!("{SERVICE_NAME} nats loop disabled: NATS_URL is not configured");
        return;
    };
    println!(
        "{SERVICE_NAME} nats loop starting: subject={} queueGroup={} resultSubject={}",
        state.request_subject, state.queue_group, state.result_subject
    );
    let mut subscription = match nats
        .queue_subscribe(state.request_subject.clone(), state.queue_group.clone())
        .await
    {
        Ok(subscription) => subscription,
        Err(error) => {
            eprintln!("{SERVICE_NAME} nats subscribe failed: {error}");
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
                "{SERVICE_NAME} rejected oversize nats request: bytes={} max={MAX_NATS_PAYLOAD_BYTES}",
                payload.len()
            );
            continue;
        }
        let task_state = state.clone();
        tokio::spawn(async move {
            match serde_json::from_slice::<FabricationPlanRequest>(&payload) {
                Ok(request) => {
                    let policy_snapshot = learning_policy_snapshot(&task_state).ok();
                    match plan_fabrication_with_policy(request, policy_snapshot.as_ref()) {
                        Ok(response) => {
                            task_state
                                .metrics
                                .plan_requests_total
                                .fetch_add(1, Ordering::Relaxed);
                            record_plan_metrics(&task_state, &response);
                            store_plan_response(&task_state, &response);
                            publish_plan_outputs(&task_state, &response).await;
                            publish_event(
                                &task_state,
                                "fabrication.plan.completed",
                                &response.request_id,
                                response.ok,
                            )
                            .await;
                        }
                        Err(error) => {
                            task_state
                                .metrics
                                .errors_total
                                .fetch_add(1, Ordering::Relaxed);
                            eprintln!("{SERVICE_NAME} failed nats fabrication plan: {error}");
                        }
                    }
                }
                Err(error) => {
                    task_state
                        .metrics
                        .errors_total
                        .fetch_add(1, Ordering::Relaxed);
                    eprintln!("{SERVICE_NAME} invalid nats fabrication request: {error}");
                }
            }
        });
    }
}

async fn root() -> impl IntoResponse {
    Json(json!({
        "service": SERVICE_NAME,
        "schemaVersion": SCHEMA_VERSION,
        "routes": [
            "GET /healthz",
            "GET /metrics",
            "GET /docs/api",
            "GET /api/docs",
            "GET /api/docs.json",
            "GET /jobs",
            "GET /jobs/:job_id",
            "GET /jobs/:job_id/artifacts/:artifact_id",
            "GET /learning/policy",
            "GET /fabrication/learning/policy",
            "POST /plan",
            "POST /fabrication/plan",
            "POST /instructions/analyze",
            "POST /fabrication/instructions/analyze",
            "POST /learning/observe",
            "POST /fabrication/learning/observe",
            "POST /learning/outcomes",
            "POST /fabrication/learning/outcomes"
        ],
        "capabilities": [
            "hybrid additive/subtractive/turning process planning",
            "draft G-code and operator instruction generation",
            "existing instruction validation and improvement hints",
            "bounded job and artifact inspection",
            "fabrication outcome reward ingestion and policy snapshots",
            "machine-failure and human-intervention boundary detection",
            "MDP/POMDP/neural policy feature contract"
        ]
    }))
}

async fn healthz() -> impl IntoResponse {
    Json(json!({ "ok": true, "service": SERVICE_NAME }))
}

async fn list_jobs(State(state): State<AppState>) -> Response {
    match state.jobs.read() {
        Ok(jobs) => {
            let records = jobs.list();
            Json(json!({
                "ok": true,
                "count": records.len(),
                "jobs": records,
            }))
            .into_response()
        }
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "ok": false, "error": format!("job store lock failed: {error}") })),
        )
            .into_response(),
    }
}

async fn get_job(State(state): State<AppState>, Path(job_id): Path<String>) -> Response {
    match state.jobs.read() {
        Ok(jobs) => match jobs.detail(&job_id) {
            Some(detail) => Json(json!({ "ok": true, "job": detail })).into_response(),
            None => (
                StatusCode::NOT_FOUND,
                Json(json!({ "ok": false, "error": "fabrication job not found" })),
            )
                .into_response(),
        },
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "ok": false, "error": format!("job store lock failed: {error}") })),
        )
            .into_response(),
    }
}

async fn get_artifact(
    State(state): State<AppState>,
    Path((job_id, artifact_id)): Path<(String, String)>,
) -> Response {
    state
        .metrics
        .artifact_requests_total
        .fetch_add(1, Ordering::Relaxed);
    match state.jobs.read() {
        Ok(jobs) => match jobs.artifact(&job_id, &artifact_id) {
            Some(artifact) => Json(json!({ "ok": true, "artifact": artifact })).into_response(),
            None => (
                StatusCode::NOT_FOUND,
                Json(json!({ "ok": false, "error": "fabrication artifact not found" })),
            )
                .into_response(),
        },
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "ok": false, "error": format!("job store lock failed: {error}") })),
        )
            .into_response(),
    }
}

async fn metrics(State(state): State<AppState>) -> Response {
    let (current_jobs, current_artifacts) = state
        .jobs
        .read()
        .map(|jobs| jobs.counts())
        .unwrap_or((0, 0));
    let current_learning_outcomes = state
        .learning
        .read()
        .map(|learning| learning.count())
        .unwrap_or(0);
    let body = format!(
        "# HELP dd_fabrication_server_plan_requests_total Fabrication plan requests received.\n\
         # TYPE dd_fabrication_server_plan_requests_total counter\n\
         dd_fabrication_server_plan_requests_total {}\n\
         # HELP dd_fabrication_server_analysis_requests_total Instruction analysis requests received.\n\
         # TYPE dd_fabrication_server_analysis_requests_total counter\n\
         dd_fabrication_server_analysis_requests_total {}\n\
         # HELP dd_fabrication_server_learning_requests_total Learning outcome requests received.\n\
         # TYPE dd_fabrication_server_learning_requests_total counter\n\
         dd_fabrication_server_learning_requests_total {}\n\
         # HELP dd_fabrication_server_generated_programs_total Draft machine programs generated.\n\
         # TYPE dd_fabrication_server_generated_programs_total counter\n\
         dd_fabrication_server_generated_programs_total {}\n\
         # HELP dd_fabrication_server_validation_findings_total Validation findings emitted.\n\
         # TYPE dd_fabrication_server_validation_findings_total counter\n\
         dd_fabrication_server_validation_findings_total {}\n\
         # HELP dd_fabrication_server_failure_boundaries_total Failure boundaries emitted.\n\
         # TYPE dd_fabrication_server_failure_boundaries_total counter\n\
         dd_fabrication_server_failure_boundaries_total {}\n\
         # HELP dd_fabrication_server_errors_total Requests or background events that failed.\n\
         # TYPE dd_fabrication_server_errors_total counter\n\
         dd_fabrication_server_errors_total {}\n\
         # HELP dd_fabrication_server_nats_messages_total Fabrication requests received from NATS.\n\
         # TYPE dd_fabrication_server_nats_messages_total counter\n\
         dd_fabrication_server_nats_messages_total {}\n\
         # HELP dd_fabrication_server_nats_published_total NATS messages published by the fabrication server.\n\
         # TYPE dd_fabrication_server_nats_published_total counter\n\
         dd_fabrication_server_nats_published_total {}\n\
         # HELP dd_fabrication_server_nats_results_published_total Fabrication result messages published to NATS.\n\
         # TYPE dd_fabrication_server_nats_results_published_total counter\n\
         dd_fabrication_server_nats_results_published_total {}\n\
         # HELP dd_fabrication_server_mdp_published_total MDP optimization requests published for fabrication policy learning.\n\
         # TYPE dd_fabrication_server_mdp_published_total counter\n\
         dd_fabrication_server_mdp_published_total {}\n\
         # HELP dd_fabrication_server_jobs_stored_total Fabrication jobs recorded in the in-process artifact ledger.\n\
         # TYPE dd_fabrication_server_jobs_stored_total counter\n\
         dd_fabrication_server_jobs_stored_total {}\n\
         # HELP dd_fabrication_server_artifacts_stored_total Fabrication artifacts recorded in the in-process artifact ledger.\n\
         # TYPE dd_fabrication_server_artifacts_stored_total counter\n\
         dd_fabrication_server_artifacts_stored_total {}\n\
         # HELP dd_fabrication_server_artifact_requests_total Artifact detail requests served by the fabrication server.\n\
         # TYPE dd_fabrication_server_artifact_requests_total counter\n\
         dd_fabrication_server_artifact_requests_total {}\n\
         # HELP dd_fabrication_server_learning_events_stored_total Learning events recorded in the in-process policy memory.\n\
         # TYPE dd_fabrication_server_learning_events_stored_total counter\n\
         dd_fabrication_server_learning_events_stored_total {}\n\
         # HELP dd_fabrication_server_current_jobs Current jobs retained in the bounded in-process artifact ledger.\n\
         # TYPE dd_fabrication_server_current_jobs gauge\n\
         dd_fabrication_server_current_jobs {}\n\
         # HELP dd_fabrication_server_current_artifacts Current artifacts retained in the bounded in-process artifact ledger.\n\
         # TYPE dd_fabrication_server_current_artifacts gauge\n\
         dd_fabrication_server_current_artifacts {}\n\
         # HELP dd_fabrication_server_current_learning_outcomes Current outcomes retained in bounded policy memory.\n\
         # TYPE dd_fabrication_server_current_learning_outcomes gauge\n\
         dd_fabrication_server_current_learning_outcomes {}\n",
        state.metrics.plan_requests_total.load(Ordering::Relaxed),
        state.metrics.analysis_requests_total.load(Ordering::Relaxed),
        state.metrics.learning_requests_total.load(Ordering::Relaxed),
        state.metrics.generated_programs_total.load(Ordering::Relaxed),
        state
            .metrics
            .validation_findings_total
            .load(Ordering::Relaxed),
        state
            .metrics
            .failure_boundaries_total
            .load(Ordering::Relaxed),
        state.metrics.errors_total.load(Ordering::Relaxed),
        state.metrics.nats_messages_total.load(Ordering::Relaxed),
        state.metrics.nats_published_total.load(Ordering::Relaxed),
        state
            .metrics
            .nats_results_published_total
            .load(Ordering::Relaxed),
        state.metrics.mdp_published_total.load(Ordering::Relaxed),
        state.metrics.jobs_stored_total.load(Ordering::Relaxed),
        state.metrics.artifacts_stored_total.load(Ordering::Relaxed),
        state.metrics.artifact_requests_total.load(Ordering::Relaxed),
        state
            .metrics
            .learning_events_stored_total
            .load(Ordering::Relaxed),
        current_jobs,
        current_artifacts,
        current_learning_outcomes,
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

async fn plan_http(
    State(state): State<AppState>,
    Json(request): Json<FabricationPlanRequest>,
) -> Response {
    state
        .metrics
        .plan_requests_total
        .fetch_add(1, Ordering::Relaxed);
    let policy_snapshot = match learning_policy_snapshot(&state) {
        Ok(snapshot) => snapshot,
        Err(error) => {
            state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "ok": false, "error": error })),
            )
                .into_response();
        }
    };
    match plan_fabrication_with_policy(request, Some(&policy_snapshot)) {
        Ok(response) => {
            record_plan_metrics(&state, &response);
            store_plan_response(&state, &response);
            publish_plan_outputs(&state, &response).await;
            publish_event(
                &state,
                "fabrication.plan.completed",
                &response.request_id,
                response.ok,
            )
            .await;
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

async fn analyze_http(
    State(state): State<AppState>,
    Json(request): Json<InstructionAnalysisRequest>,
) -> Response {
    state
        .metrics
        .analysis_requests_total
        .fetch_add(1, Ordering::Relaxed);
    let request_id = request_id(request.request_id.as_ref(), "instruction-analysis");
    let programs = match validate_programs(&request.programs) {
        Ok(programs) if !programs.is_empty() => programs,
        Ok(_) => {
            state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "ok": false, "error": "programs must not be empty" })),
            )
                .into_response();
        }
        Err(error) => {
            state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "ok": false, "error": error })),
            )
                .into_response();
        }
    };
    let machines = match validate_machines(request.machines) {
        Ok(machines) => machines,
        Err(error) => {
            state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "ok": false, "error": error })),
            )
                .into_response();
        }
    };
    if let Some(material) = request.material {
        if let Err(error) = material_or_default(Some(material)) {
            state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "ok": false, "error": error })),
            )
                .into_response();
        }
    }

    let (analyzed, mut validation, improvements) = analyze_instruction_programs(&programs);
    let simulation = simulate_instruction_programs(&programs, &machines);
    validation.findings.extend(simulation.findings.clone());
    validation
        .failure_boundaries
        .extend(simulation.failure_boundaries.clone());
    validation.severity = report_severity(&validation.findings, &validation.failure_boundaries);
    validation.ok = validation.severity != "error";
    let improved_programs = improve_instruction_programs(&programs, &validation, &improvements);
    state
        .metrics
        .validation_findings_total
        .fetch_add(validation.findings.len() as u64, Ordering::Relaxed);
    state.metrics.failure_boundaries_total.fetch_add(
        validation.failure_boundaries.len() as u64,
        Ordering::Relaxed,
    );
    let generated_at_ms = now_ms();
    let response = InstructionAnalysisResponse {
        ok: validation.ok,
        job_id: safe_job_id("analysis", &request_id, generated_at_ms),
        request_id,
        programs: analyzed,
        validation,
        simulation,
        improvements,
        improved_programs,
        generated_at_ms,
    };
    store_analysis_response(&state, &response);
    publish_event(
        &state,
        "fabrication.instructions.analyzed",
        &response.request_id,
        response.ok,
    )
    .await;
    Json(response).into_response()
}

async fn learning_observe_http(
    State(state): State<AppState>,
    Json(request): Json<FabricationOutcomeRequest>,
) -> Response {
    state
        .metrics
        .learning_requests_total
        .fetch_add(1, Ordering::Relaxed);
    match learn_from_outcome(request) {
        Ok((response, record)) => {
            let snapshot = match store_learning_response(&state, &response, record) {
                Ok(snapshot) => snapshot,
                Err(error) => {
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(json!({ "ok": false, "error": error })),
                    )
                        .into_response();
                }
            };
            publish_learning_outputs(&state, &response).await;
            publish_event(
                &state,
                "fabrication.learning.observed",
                &response.request_id,
                response.ok,
            )
            .await;
            Json(json!({
                "ok": true,
                "learning": response,
                "policy": snapshot,
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

async fn learning_outcome_http(
    State(state): State<AppState>,
    Json(request): Json<LearningOutcomeRequest>,
) -> Response {
    state
        .metrics
        .learning_requests_total
        .fetch_add(1, Ordering::Relaxed);
    match learning_outcome_record(request) {
        Ok(record) => {
            let outcome_id = record.outcome_id.clone();
            let snapshot = match store_learning_record(&state, record) {
                Ok(snapshot) => snapshot,
                Err(error) => {
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(json!({ "ok": false, "error": error })),
                    )
                        .into_response();
                }
            };
            publish_event(&state, "fabrication.learning.outcome", &outcome_id, true).await;
            Json(json!({
                "ok": true,
                "outcomeId": outcome_id,
                "policy": snapshot,
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

async fn learning_policy_http(State(state): State<AppState>) -> Response {
    match learning_policy_snapshot(&state) {
        Ok(snapshot) => Json(json!({
            "ok": true,
            "policy": snapshot,
        }))
        .into_response(),
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "ok": false, "error": error })),
        )
            .into_response(),
    }
}

async fn api_docs_html() -> axum::response::Html<&'static str> {
    axum::response::Html(include_str!("../generated/api-docs.html"))
}

async fn api_docs_json() -> impl IntoResponse {
    (
        [("content-type", "application/json; charset=utf-8")],
        include_str!("../generated/api-docs.json"),
    )
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
        request_subject: env_value("FABRICATION_REQUEST_SUBJECT", FABRICATION_REQUESTS_SUBJECT),
        queue_group: env_value("FABRICATION_QUEUE_GROUP", FABRICATION_REQUESTS_QUEUE_GROUP),
        result_subject: env_value("FABRICATION_RESULT_SUBJECT", FABRICATION_RESULTS_SUBJECT),
        event_subject: env_value("FABRICATION_EVENT_SUBJECT", RUNTIME_EVENTS_SUBJECT),
        mdp_subject: env_value("FABRICATION_MDP_OPTIMIZE_SUBJECT", MDP_OPTIMIZE_SUBJECT),
        mdp_autopublish: env_bool("FABRICATION_MDP_AUTOPUBLISH", false),
        metrics: Arc::new(Metrics::default()),
        jobs: Arc::new(RwLock::new(FabricationJobStore::new(MAX_STORED_JOBS))),
        learning: Arc::new(RwLock::new(LearningMemory::new(MAX_LEARNING_OUTCOMES))),
    };
    tokio::spawn(run_nats_loop(state.clone()));

    let app = Router::new()
        .route("/", get(root))
        .route("/healthz", get(healthz))
        .route("/readyz", get(healthz))
        .route("/docs/api", get(api_docs_html))
        .route("/api/docs", get(api_docs_html))
        .route("/api/docs.json", get(api_docs_json))
        .route("/metrics", get(metrics))
        .route("/jobs", get(list_jobs))
        .route("/jobs/:job_id", get(get_job))
        .route("/jobs/:job_id/artifacts/:artifact_id", get(get_artifact))
        .route("/learning/policy", get(learning_policy_http))
        .route("/fabrication/learning/policy", get(learning_policy_http))
        .route("/plan", post(plan_http))
        .route("/fabrication/plan", post(plan_http))
        .route("/instructions/analyze", post(analyze_http))
        .route("/fabrication/instructions/analyze", post(analyze_http))
        .route("/learning/observe", post(learning_observe_http))
        .route("/fabrication/learning/observe", post(learning_observe_http))
        .route("/learning/outcomes", post(learning_outcome_http))
        .route(
            "/fabrication/learning/outcomes",
            post(learning_outcome_http),
        )
        .layer(DefaultBodyLimit::max(MAX_HTTP_BODY_BYTES))
        .with_state(state)
        .merge(dd_runtime_config_client::router());

    tokio::spawn(dd_runtime_config_client::register_with_control_plane());

    let addr: SocketAddr = format!("{host}:{port}").parse()?;
    println!("{SERVICE_NAME} listening on http://{addr}");
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

    fn material(name: &str, family: &str) -> MaterialSpec {
        MaterialSpec {
            name: name.to_string(),
            family: Some(family.to_string()),
            hardness: None,
        }
    }

    fn program(id: &str, machine_kind: &str, instructions: &[&str]) -> InstructionProgram {
        InstructionProgram {
            id: Some(id.to_string()),
            machine_id: Some(format!("{machine_kind}-1")),
            machine_kind: Some(machine_kind.to_string()),
            language: Some("gcode".to_string()),
            instructions: instructions.iter().map(|line| line.to_string()).collect(),
        }
    }

    #[test]
    fn hybrid_plan_splits_tight_metal_object_across_mill_and_lathe() {
        let response = plan_fabrication(FabricationPlanRequest {
            request_id: Some("unit-hybrid".to_string()),
            objective:
                "aluminum gearbox housing with a round bearing shaft insert and tight datum bore"
                    .to_string(),
            material: Some(material("aluminum", "metal")),
            stock: Some(StockSpec {
                form: "bar-and-plate".to_string(),
                dimensions_mm: Some(vec![150.0, 80.0, 40.0]),
            }),
            tolerance_mm: Some(0.03),
            quantity: Some(1),
            machines: None,
            constraints: Some(FabricationConstraints {
                max_setups: Some(4),
                allow_human_intervention: Some(true),
                allow_multi_part_assembly: Some(true),
                require_dry_run: Some(true),
                preferred_methods: None,
                preferred_assembly_strategy: None,
            }),
            parts: None,
            existing_instructions: None,
            learning: None,
        })
        .expect("hybrid plan should be generated");

        assert_eq!(response.request_id, "unit-hybrid");
        assert!(response
            .design
            .parts
            .iter()
            .any(|part| part.machine_kind.contains("mill")));
        assert!(response
            .design
            .parts
            .iter()
            .any(|part| part.machine_kind.contains("lathe")));
        assert!(response
            .assembly
            .split_candidates
            .iter()
            .any(|candidate| candidate.contains("tight features")));
        assert!(response
            .validation
            .failure_boundaries
            .iter()
            .any(|boundary| {
                boundary.kind == "inspection-gate" && boundary.requires_human_intervention
            }));
        assert!(response
            .learning
            .actions
            .iter()
            .any(|action| action == "combine-parts"));
        assert!(!response.generated_programs.is_empty());
        assert!(response
            .generated_programs
            .iter()
            .all(|program| program.draft && !program.machine_ready));
    }

    #[test]
    fn horizontal_mill_plan_generates_side_slot_program_and_artifact_metadata() {
        let response = plan_fabrication(FabricationPlanRequest {
            request_id: Some("unit-horizontal-mill".to_string()),
            objective: "steel fixture rail with horizontal keyway and deep side slot".to_string(),
            material: Some(material("steel", "metal")),
            stock: Some(StockSpec {
                form: "bar".to_string(),
                dimensions_mm: Some(vec![220.0, 70.0, 45.0]),
            }),
            tolerance_mm: Some(0.04),
            quantity: Some(1),
            machines: None,
            constraints: Some(FabricationConstraints {
                max_setups: Some(3),
                allow_human_intervention: Some(true),
                allow_multi_part_assembly: Some(true),
                require_dry_run: Some(true),
                preferred_methods: Some(vec!["horizontal-milling".to_string()]),
                preferred_assembly_strategy: None,
            }),
            parts: None,
            existing_instructions: None,
            learning: None,
        })
        .expect("horizontal mill plan should be generated");

        assert!(response.design.parts.iter().any(|part| {
            part.id == "horizontal-slotted-feature"
                && part.machine_kind == "horizontal-mill"
                && part.manufacturing_method == "subtractive-milling"
        }));
        assert!(response
            .process_plan
            .iter()
            .any(|step| step.operation.contains("side-mill slots")));
        let horizontal_program = response
            .generated_programs
            .iter()
            .find(|program| program.machine_kind == "horizontal-mill")
            .expect("horizontal mill program should be generated");
        assert_eq!(horizontal_program.language, "iso-gcode");
        assert!(horizontal_program
            .instructions
            .iter()
            .any(|line| line.contains("draft horizontal milling program")));
        assert!(horizontal_program
            .instructions
            .iter()
            .any(|line| line.contains("index fixture")));
        assert!(horizontal_program
            .safety_notes
            .iter()
            .any(|note| note.contains("arbor support")));

        let job = stored_plan_job(&response);
        let parametric_design = job
            .artifacts
            .get("parametric-design")
            .expect("parametric design artifact should be retained");
        assert!(parametric_design
            .content
            .get("parts")
            .and_then(Value::as_array)
            .is_some_and(|parts| parts.iter().any(|part| {
                part.get("primitive")
                    .and_then(|primitive| primitive.get("primitive"))
                    .and_then(Value::as_str)
                    == Some("horizontal-subtractive-feature")
            })));
    }

    #[test]
    fn router_plan_uses_default_cnc_router_and_tabbed_profile_program() {
        let response = plan_fabrication(FabricationPlanRequest {
            request_id: Some("unit-router".to_string()),
            objective: "plywood sign with engraved lettering and tabbed outside profile"
                .to_string(),
            material: Some(material("plywood", "wood")),
            stock: Some(StockSpec {
                form: "sheet".to_string(),
                dimensions_mm: Some(vec![400.0, 200.0, 12.0]),
            }),
            tolerance_mm: Some(0.25),
            quantity: Some(1),
            machines: None,
            constraints: Some(FabricationConstraints {
                max_setups: Some(2),
                allow_human_intervention: Some(true),
                allow_multi_part_assembly: Some(true),
                require_dry_run: Some(true),
                preferred_methods: Some(vec!["routing".to_string()]),
                preferred_assembly_strategy: None,
            }),
            parts: None,
            existing_instructions: None,
            learning: None,
        })
        .expect("router plan should be generated");

        assert!(response
            .design
            .parts
            .iter()
            .any(|part| part.manufacturing_method == "subtractive-routing"
                && part.machine_kind == "cnc-router"));
        let router_program = response
            .generated_programs
            .iter()
            .find(|program| program.machine_kind == "cnc-router")
            .expect("router program should be generated");
        assert_eq!(router_program.language, "grbl-gcode");
        assert!(router_program
            .instructions
            .iter()
            .any(|line| line.contains("draft router profile program")));
        assert!(router_program
            .instructions
            .iter()
            .any(|line| line.contains("lift over tab boundary")));
        assert!(router_program
            .safety_notes
            .iter()
            .any(|note| note.contains("hold-down")));
        assert!(response
            .learning
            .actions
            .iter()
            .any(|action| action == "choose-routing-process"));
        assert!(!response
            .validation
            .findings
            .iter()
            .any(|finding| finding.code == "missing-spindle-start"));
    }

    #[test]
    fn default_sheet_cut_fleet_generates_laser_job_and_kerf_boundary() {
        let response = plan_fabrication(FabricationPlanRequest {
            request_id: Some("unit-laser-cutter".to_string()),
            objective: "acrylic laser-cut stencil with kerf test and engraved alignment marks"
                .to_string(),
            material: Some(material("acrylic", "plastic")),
            stock: Some(StockSpec {
                form: "sheet".to_string(),
                dimensions_mm: Some(vec![300.0, 200.0, 3.0]),
            }),
            tolerance_mm: Some(0.18),
            quantity: Some(1),
            machines: None,
            constraints: None,
            parts: None,
            existing_instructions: None,
            learning: None,
        })
        .expect("laser cutter plan should be generated");

        assert!(response.design.parts.iter().any(|part| {
            part.id == "sheet-cut-profile"
                && part.machine_kind == "laser-cutter"
                && part.manufacturing_method == "sheet-cutting"
        }));
        assert!(response
            .process_plan
            .iter()
            .any(|step| step.operation.contains("kerf-test")));
        assert!(response
            .learning
            .actions
            .iter()
            .any(|action| action == "choose-sheet-cutting-process"));
        let laser_program = response
            .generated_programs
            .iter()
            .find(|program| program.machine_kind == "laser-cutter")
            .expect("laser program should be generated");
        assert_eq!(laser_program.language, "laser-job");
        assert!(laser_program
            .instructions
            .iter()
            .any(|line| line.contains("draft laser sheet-cutting job")));
        assert!(laser_program
            .instructions
            .iter()
            .any(|line| line.contains("KERF_TEST")));
        assert!(response.validation.findings.iter().any(|finding| {
            finding.code == "text-sheet-cutting-boundary"
                && finding
                    .program_id
                    .as_deref()
                    .is_some_and(|id| id.contains("laser-cutter"))
        }));
        assert!(response
            .validation
            .failure_boundaries
            .iter()
            .any(|boundary| boundary.kind == "sheet-cutting-boundary"));
    }

    #[test]
    fn default_sheet_cut_fleet_generates_waterjet_job_for_metal_profile() {
        let response = plan_fabrication(FabricationPlanRequest {
            request_id: Some("unit-waterjet-cutter".to_string()),
            objective: "steel waterjet-cut bracket with abrasive pierce and kerf compensation"
                .to_string(),
            material: Some(material("steel", "metal")),
            stock: Some(StockSpec {
                form: "sheet".to_string(),
                dimensions_mm: Some(vec![500.0, 250.0, 12.0]),
            }),
            tolerance_mm: Some(0.20),
            quantity: Some(1),
            machines: None,
            constraints: None,
            parts: None,
            existing_instructions: None,
            learning: None,
        })
        .expect("waterjet cutter plan should be generated");

        assert!(response.design.parts.iter().any(|part| {
            part.id == "sheet-cut-profile"
                && part.machine_kind == "waterjet-cutter"
                && part.manufacturing_method == "sheet-cutting"
        }));
        let waterjet_program = response
            .generated_programs
            .iter()
            .find(|program| program.machine_kind == "waterjet-cutter")
            .expect("waterjet program should be generated");
        assert_eq!(waterjet_program.language, "waterjet-job");
        assert!(waterjet_program
            .instructions
            .iter()
            .any(|line| line.contains("draft waterjet sheet-cutting job")));
        assert!(waterjet_program
            .instructions
            .iter()
            .any(|line| line.contains("ABRASIVE_FLOW_TEST")));
        assert!(waterjet_program
            .safety_notes
            .iter()
            .any(|note| note.contains("high-pressure water")));
        assert!(response.validation.findings.iter().any(|finding| {
            finding.code == "text-sheet-cutting-boundary"
                && finding
                    .program_id
                    .as_deref()
                    .is_some_and(|id| id.contains("waterjet-cutter"))
        }));
    }

    #[test]
    fn default_sheet_cut_fleet_generates_plasma_job_for_conductive_sheet() {
        let response = plan_fabrication(FabricationPlanRequest {
            request_id: Some("unit-plasma-cutter".to_string()),
            objective: "plasma-cut steel guard plate with pierce height and dross allowance"
                .to_string(),
            material: Some(material("steel", "metal")),
            stock: Some(StockSpec {
                form: "sheet".to_string(),
                dimensions_mm: Some(vec![420.0, 280.0, 6.0]),
            }),
            tolerance_mm: Some(0.35),
            quantity: Some(1),
            machines: None,
            constraints: None,
            parts: None,
            existing_instructions: None,
            learning: None,
        })
        .expect("plasma cutter plan should be generated");

        assert!(response.design.parts.iter().any(|part| {
            part.id == "sheet-cut-profile"
                && part.machine_kind == "plasma-cutter"
                && part.manufacturing_method == "sheet-cutting"
        }));
        let plasma_program = response
            .generated_programs
            .iter()
            .find(|program| program.machine_kind == "plasma-cutter")
            .expect("plasma program should be generated");
        assert_eq!(plasma_program.language, "plasma-job");
        assert!(plasma_program
            .instructions
            .iter()
            .any(|line| line.contains("draft plasma sheet-cutting job")));
        assert!(plasma_program
            .instructions
            .iter()
            .any(|line| line.contains("ARC_OK")));
        assert!(plasma_program
            .instructions
            .iter()
            .any(|line| line.contains("PLASMA_CUT")));
        assert!(plasma_program
            .safety_notes
            .iter()
            .any(|note| note.contains("conductive workholding")));
    }

    #[test]
    fn default_additive_fleet_generates_resin_printer_job() {
        let response = plan_fabrication(FabricationPlanRequest {
            request_id: Some("unit-resin-printer".to_string()),
            objective: "resin SLA dental guide with fine organic channels".to_string(),
            material: Some(material("resin", "polymer")),
            stock: None,
            tolerance_mm: Some(0.08),
            quantity: Some(1),
            machines: None,
            constraints: None,
            parts: None,
            existing_instructions: None,
            learning: None,
        })
        .expect("resin printer plan should be generated");

        assert!(response.design.parts.iter().any(|part| {
            part.machine_kind == "sla-printer" && part.manufacturing_method == "additive-print"
        }));
        assert!(response
            .process_plan
            .iter()
            .any(|step| step.operation.contains("UV cure")));
        let resin_program = response
            .generated_programs
            .iter()
            .find(|program| program.machine_kind == "sla-printer")
            .expect("SLA program should be generated");
        assert_eq!(resin_program.language, "sla-job");
        assert!(resin_program
            .instructions
            .iter()
            .any(|line| line.contains("draft resin SLA/MSLA job")));
        assert!(resin_program
            .instructions
            .iter()
            .any(|line| line.contains("process-split-boundary")));
        assert!(resin_program
            .safety_notes
            .iter()
            .any(|note| note.contains("wash/cure timing")));
        assert_eq!(response.validation.severity, "warning");
        assert!(response.validation.findings.iter().any(|finding| {
            finding.code == "text-post-processing-boundary"
                && finding
                    .program_id
                    .as_deref()
                    .is_some_and(|id| id.contains("sla-printer"))
        }));
        assert!(response
            .validation
            .failure_boundaries
            .iter()
            .any(|boundary| boundary.kind == "post-processing-boundary"));
    }

    #[test]
    fn default_additive_fleet_generates_powder_bed_printer_job() {
        let response = plan_fabrication(FabricationPlanRequest {
            request_id: Some("unit-sls-printer".to_string()),
            objective: "SLS nylon manifold with nested powder bed clips".to_string(),
            material: Some(material("pa12", "polymer")),
            stock: None,
            tolerance_mm: Some(0.18),
            quantity: Some(1),
            machines: None,
            constraints: None,
            parts: None,
            existing_instructions: None,
            learning: None,
        })
        .expect("powder-bed printer plan should be generated");

        assert!(response
            .design
            .parts
            .iter()
            .any(|part| part.machine_kind == "sls-printer"));
        assert!(response
            .process_plan
            .iter()
            .any(|step| step.operation.contains("depowder")));
        let powder_program = response
            .generated_programs
            .iter()
            .find(|program| program.machine_kind == "sls-printer")
            .expect("SLS program should be generated");
        assert_eq!(powder_program.language, "sls-job");
        assert!(powder_program
            .instructions
            .iter()
            .any(|line| line.contains("draft powder-bed additive job")));
        assert!(powder_program
            .instructions
            .iter()
            .any(|line| line.contains("cooldown-boundary")));
        assert!(powder_program
            .safety_notes
            .iter()
            .any(|note| note.contains("depowdering")));
        assert_eq!(response.validation.severity, "warning");
        assert!(response.validation.findings.iter().any(|finding| {
            finding.code == "text-post-processing-boundary"
                && finding
                    .program_id
                    .as_deref()
                    .is_some_and(|id| id.contains("sls-printer"))
        }));
        assert!(response
            .validation
            .failure_boundaries
            .iter()
            .any(|boundary| boundary.kind == "post-processing-boundary"));
    }

    #[test]
    fn router_analysis_flags_profile_before_spindle_and_tab_stop_boundary() {
        let programs = vec![program(
            "unsafe-router",
            "cnc-router",
            &[
                "G21 G90 G54",
                "G1 X120 Y0 F900",
                "M0 inspect retained tabs and hold-down",
                "M30",
            ],
        )];

        let (analyzed, validation, improvements) = analyze_instruction_programs(&programs);

        assert_eq!(analyzed[0].machine_kind, "cnc-router");
        assert_eq!(validation.severity, "error");
        assert!(validation
            .findings
            .iter()
            .any(|finding| finding.code == "cut-before-spindle"));
        assert!(validation
            .findings
            .iter()
            .any(|finding| finding.code == "missing-spindle-start"));
        assert!(validation
            .failure_boundaries
            .iter()
            .any(|boundary| boundary.kind == "machine-safety-gate"));
        assert!(validation.failure_boundaries.iter().any(|boundary| {
            boundary.kind == "human-intervention" && boundary.requires_human_intervention
        }));
        assert!(!improvements
            .iter()
            .any(|improvement| improvement.action == "add-coordinate-reference"));

        let improved = improve_instruction_programs(&programs, &validation, &improvements);
        assert!(improved[0].changed);
        assert!(!improved[0].machine_ready);
        assert!(improved[0]
            .instructions
            .iter()
            .any(|line| line.contains("REVIEW: add verified spindle speed")));
        assert!(improved[0]
            .instructions
            .iter()
            .any(|line| line.contains("boundary machine-safety-gate")));
    }

    #[test]
    fn mill_analysis_flags_uncompensated_tool_length_before_plunge() {
        let programs = vec![program(
            "uncompensated-mill",
            "vertical-mill",
            &[
                "G21 G90 G54",
                "T1 M6",
                "S8000 M3",
                "G0 X0 Y0 Z15",
                "G1 Z-2.0 F120",
                "M30",
            ],
        )];

        let (_, validation, improvements) = analyze_instruction_programs(&programs);

        assert_eq!(validation.severity, "warning");
        assert!(validation.findings.iter().any(|finding| {
            finding.code == "missing-tool-length-compensation"
                && finding.program_id.as_deref() == Some("uncompensated-mill")
                && finding.line == Some(5)
        }));
        assert!(validation.failure_boundaries.iter().any(|boundary| {
            boundary.kind == "tool-length-boundary"
                && boundary.requires_human_intervention
                && boundary.suggested_resolution.contains("G43")
        }));
        assert!(improvements.is_empty());

        let improved = improve_instruction_programs(&programs, &validation, &improvements);
        assert!(improved[0].changed);
        assert!(!improved[0].machine_ready);
        assert!(improved[0]
            .instructions
            .iter()
            .any(|line| line.contains("boundary tool-length-boundary")));
    }

    #[test]
    fn simulation_flags_submitted_toolpath_outside_machine_envelope() {
        let programs = vec![InstructionProgram {
            id: Some("oversize-router".to_string()),
            machine_id: Some("tiny-router".to_string()),
            machine_kind: Some("cnc-router".to_string()),
            language: Some("grbl-gcode".to_string()),
            instructions: vec![
                "G21 G90 G54".to_string(),
                "S18000 M3".to_string(),
                "G0 X0 Y0 Z8".to_string(),
                "G1 X150 Y20 Z-2 F800".to_string(),
                "M30".to_string(),
            ],
        }];
        let machines = vec![MachineProfile {
            id: "tiny-router".to_string(),
            kind: "cnc-router".to_string(),
            controller: Some("grbl-gcode".to_string()),
            materials: Some(vec!["wood".to_string()]),
            work_envelope_mm: Some(vec![100.0, 80.0, 50.0]),
            axes: Some(3),
            operations: Some(vec!["profile".to_string()]),
        }];

        let simulation = simulate_instruction_programs(&programs, &machines);

        assert!(!simulation.ok);
        assert_eq!(simulation.severity, "error");
        assert!(simulation
            .findings
            .iter()
            .any(|finding| finding.code == "simulated-axis-envelope-exceeded"));
        assert!(simulation
            .failure_boundaries
            .iter()
            .any(|boundary| boundary.kind == "simulated-machine-envelope"));
        let trace = simulation
            .programs
            .first()
            .expect("simulation should keep a program trace");
        assert_eq!(trace.machine_id.as_deref(), Some("tiny-router"));
        assert!(trace.safe_clearance_observed);
        assert!(trace.spindle_or_heatup_observed);
        assert!(trace.axis_extents.iter().any(|axis| {
            axis.axis == "X" && axis.limit_mm == Some(100.0) && axis.exceeds_limit
        }));
    }

    #[test]
    fn additive_analysis_flags_extrusion_before_heatup_and_homing() {
        let programs = vec![program(
            "bad-print",
            "fdm-printer",
            &["G21", "G90", "G1 X10 Y10 E2.0 F900", "M84"],
        )];

        let (_, validation, improvements) = analyze_instruction_programs(&programs);

        assert_eq!(validation.severity, "error");
        assert!(validation
            .findings
            .iter()
            .any(|finding| finding.code == "extrusion-before-heatup"));
        assert!(validation
            .findings
            .iter()
            .any(|finding| finding.code == "print-before-homing"));
        assert!(validation
            .failure_boundaries
            .iter()
            .any(|boundary| boundary.kind == "printer-state-gate"));
        assert!(improvements
            .iter()
            .any(|improvement| improvement.action == "add-coordinate-reference"));
        let improved = improve_instruction_programs(&programs, &validation, &improvements);
        assert!(improved[0].changed);
        assert!(!improved[0].machine_ready);
        assert!(improved[0]
            .instructions
            .iter()
            .any(|line| line.starts_with("G28 ; added review draft")));
        assert!(improved[0]
            .instructions
            .iter()
            .any(|line| { line.contains("REVIEW: add verified nozzle/bed heat-up commands") }));
    }

    #[test]
    fn additive_analysis_flags_material_change_as_operator_boundary() {
        let programs = vec![program(
            "multi-material-print",
            "fdm-printer",
            &[
                "G21 G90",
                "G28",
                "M104 S215",
                "M109 S215",
                "G1 X10 Y10 E1.0 F900",
                "M600 ; color change before raised logo",
                "T1 ; switch to support extruder",
                "G1 X20 Y10 E1.5 F900",
                "M84",
            ],
        )];

        let (_, validation, improvements) = analyze_instruction_programs(&programs);

        assert_eq!(validation.severity, "warning");
        assert!(validation.findings.iter().any(|finding| {
            finding.code == "additive-material-change-boundary"
                && finding.program_id.as_deref() == Some("multi-material-print")
                && finding.line == Some(6)
        }));
        assert!(validation.findings.iter().any(|finding| {
            finding.code == "additive-material-change-boundary" && finding.line == Some(7)
        }));
        assert!(validation.failure_boundaries.iter().any(|boundary| {
            boundary.kind == "additive-material-change-boundary"
                && boundary.requires_human_intervention
                && boundary.suggested_resolution.contains("AMS/MMU")
        }));
        assert!(validation
            .failure_boundaries
            .iter()
            .any(|boundary| { boundary.kind == "human-intervention" && boundary.line == Some(6) }));
        assert!(improvements.is_empty());

        let improved = improve_instruction_programs(&programs, &validation, &improvements);
        assert!(improved[0].changed);
        assert!(!improved[0].machine_ready);
        assert!(improved[0]
            .instructions
            .iter()
            .any(|line| line.contains("boundary additive-material-change-boundary")));
    }

    #[test]
    fn mill_analysis_finds_cut_before_spindle_and_manual_stop_boundary() {
        let programs = vec![program(
            "bad-mill",
            "vertical-mill",
            &["G21 G90 G54", "G1 Z-1.0 F100", "M0 flip fixture", "M30"],
        )];

        let (analyzed, validation, improvements) = analyze_instruction_programs(&programs);

        assert_eq!(analyzed[0].program_id, "bad-mill");
        assert_eq!(validation.severity, "error");
        assert!(validation
            .findings
            .iter()
            .any(|finding| finding.code == "cut-before-spindle"));
        assert!(validation.failure_boundaries.iter().any(|boundary| {
            boundary.kind == "human-intervention" && boundary.requires_human_intervention
        }));
        assert!(validation
            .failure_boundaries
            .iter()
            .any(|boundary| boundary.kind == "machine-safety-gate"));
        assert!(improvements.is_empty());
    }

    #[test]
    fn lathe_analysis_flags_css_threading_and_partoff_boundaries() {
        let programs = vec![program(
            "risky-lathe",
            "lathe",
            &[
                "G21 G90 G54",
                "T0303",
                "G96 S220 M3",
                "G1 X18 Z-20 F0.18",
                "G76 X16 Z-30 P010060 Q100 F1.5",
                "G1 X2 Z-35 F0.04 ; part-off cutoff",
                "M30",
            ],
        )];

        let (analyzed, validation, improvements) = analyze_instruction_programs(&programs);

        assert_eq!(analyzed[0].machine_kind, "lathe");
        assert_eq!(validation.severity, "warning");
        assert!(validation
            .findings
            .iter()
            .any(|finding| finding.code == "lathe-css-without-spindle-limit"));
        assert!(validation
            .findings
            .iter()
            .any(|finding| finding.code == "lathe-threading-boundary"));
        assert!(validation
            .findings
            .iter()
            .any(|finding| finding.code == "lathe-part-off-boundary"));
        assert!(validation.failure_boundaries.iter().any(|boundary| {
            boundary.kind == "lathe-spindle-speed-boundary"
                && boundary.requires_human_intervention
        }));
        assert!(validation
            .failure_boundaries
            .iter()
            .any(|boundary| boundary.kind == "lathe-threading-boundary"));
        assert!(validation
            .failure_boundaries
            .iter()
            .any(|boundary| boundary.kind == "lathe-part-off-boundary"));
        assert!(improvements.is_empty());

        let improved = improve_instruction_programs(&programs, &validation, &improvements);
        assert!(improved[0].changed);
        assert!(improved[0]
            .instructions
            .iter()
            .any(|line| line.contains("boundary lathe-spindle-speed-boundary")));
        assert!(improved[0]
            .instructions
            .iter()
            .any(|line| line.contains("boundary lathe-threading-boundary")));
        assert!(improved[0]
            .instructions
            .iter()
            .any(|line| line.contains("boundary lathe-part-off-boundary")));
    }

    #[test]
    fn text_printer_job_finds_post_process_split_and_assembly_boundaries() {
        let programs = vec![InstructionProgram {
            id: Some("resin-assembly".to_string()),
            machine_id: Some("sla-1".to_string()),
            machine_kind: Some("sla-printer".to_string()),
            language: Some("printer-job".to_string()),
            instructions: vec![
                "Slice resin bracket part 1 and part 2 with dense supports".to_string(),
                "Pause for operator to remove part from the build plate".to_string(),
                "Wash, UV cure, and remove supports before fit check".to_string(),
                "Assemble two pieces with pins and epoxy; finish when alignment passes".to_string(),
            ],
        }];

        let (analyzed, validation, improvements) = analyze_instruction_programs(&programs);

        assert_eq!(analyzed[0].language, "printer-job");
        assert_eq!(validation.severity, "warning");
        assert!(validation
            .findings
            .iter()
            .any(|finding| finding.code == "text-post-processing-boundary"));
        assert!(validation
            .findings
            .iter()
            .any(|finding| finding.code == "text-assembly-boundary"));
        assert!(validation
            .failure_boundaries
            .iter()
            .any(|boundary| boundary.kind == "split-boundary"));
        assert!(validation.failure_boundaries.iter().any(|boundary| {
            boundary.kind == "human-intervention" && boundary.requires_human_intervention
        }));
        assert!(!improvements
            .iter()
            .any(|improvement| improvement.action == "add-units-mode"));
        let improved = improve_instruction_programs(&programs, &validation, &improvements);
        assert!(improved[0].changed);
        assert!(improved[0]
            .instructions
            .iter()
            .any(|line| line.starts_with("CHECKPOINT [post-processing-boundary]")));
        assert!(improved[0]
            .instructions
            .iter()
            .any(|line| line.starts_with("CHECKPOINT [assembly-boundary]")));
    }

    #[test]
    fn custom_learning_hints_feed_policy_contract() {
        let response = plan_fabrication(FabricationPlanRequest {
            request_id: Some("unit-learning".to_string()),
            objective: "printed ergonomic handle with machined threaded brass insert".to_string(),
            material: Some(material("petg", "polymer")),
            stock: None,
            tolerance_mm: Some(0.12),
            quantity: Some(2),
            machines: None,
            constraints: None,
            parts: None,
            existing_instructions: Some(vec![program(
                "legacy-insert",
                "lathe",
                &[
                    "G21 G90",
                    "G54",
                    "T0101",
                    "G96 S160 M3",
                    "G1 Z-12 F0.1",
                    "M0 measure thread",
                    "M30",
                ],
            )]),
            learning: Some(LearningHints {
                policy_hint: Some(
                    "prefer printed body plus turned insert after success".to_string(),
                ),
                model_family: Some("mdp-pomdp-neural-cam-policy".to_string()),
                reward_weights: Some(BTreeMap::from([
                    ("accuracy".to_string(), 2.0),
                    ("interventionCost".to_string(), -1.0),
                ])),
                observations: Some(vec![
                    "thread-gauge-pass".to_string(),
                    "insert-fit".to_string(),
                ]),
                prior_successes: Some(vec!["handle-v1-plus-insert-v2".to_string()]),
            }),
        })
        .expect("learning plan should be generated");

        assert_eq!(
            response.learning.model_family,
            "mdp-pomdp-neural-cam-policy"
        );
        assert!(response
            .learning
            .pomdp_observations
            .iter()
            .any(|observation| observation == "insert-fit"));
        assert_eq!(
            response.learning.neural_policy.schema_version,
            "dd.fabrication.neural-policy-sketch.v1"
        );
        assert_eq!(
            response.learning.neural_policy.model_family,
            "mdp-pomdp-neural-cam-policy"
        );
        assert_eq!(response.learning.neural_policy.feature_vector.len(), 7);
        assert_eq!(response.learning.neural_policy.hidden_activations.len(), 3);
        assert!(response
            .learning
            .neural_policy
            .action_scores
            .iter()
            .any(|score| score.action == "reject-or-repostprocess-program"
                && score.score >= 0.0
                && score.score <= 1.0));
        assert!(response
            .validation
            .failure_boundaries
            .iter()
            .any(|boundary| boundary.kind == "human-intervention"));
    }

    #[test]
    fn fabrication_outcome_creates_reward_and_training_artifacts() {
        let (response, record) = learn_from_outcome(FabricationOutcomeRequest {
            request_id: Some("unit-outcome".to_string()),
            source_job_id: Some("plan-unit-artifact-plan-123".to_string()),
            source_artifact_id: Some("program-main".to_string()),
            part_id: Some("bracket".to_string()),
            program_id: Some("mill-bracket".to_string()),
            machine_id: Some("vertical-mill-1".to_string()),
            machine_kind: Some("vertical-mill".to_string()),
            material: Some(material("aluminum", "metal")),
            outcome: "machine alarm; part scrapped after manual intervention".to_string(),
            completed: Some(false),
            machine_failure: Some(true),
            scrap: Some(true),
            human_intervention_required: Some(true),
            intervention_minutes: Some(18.0),
            duration_minutes: Some(42.0),
            dimensional_error_mm: Some(0.42),
            surface_quality: Some(0.25),
            observations: Some(vec!["spindle load spike".to_string()]),
            notes: Some(vec!["revise feeds and split the finish pass".to_string()]),
            reward_weights: None,
        })
        .expect("fabrication outcome should produce learning evidence");

        assert!(!response.ok);
        assert_eq!(response.state, "failed");
        assert_eq!(
            response.recommended_action,
            "reject-or-repostprocess-program"
        );
        assert!(response.reward < 0.0);
        assert!(response
            .observations
            .iter()
            .any(|observation| observation == "machine-failure"));
        assert_eq!(
            response
                .mdp_update
                .get("schemaVersion")
                .and_then(Value::as_str),
            Some("dd.fabrication.learning-experience.v1")
        );
        assert_eq!(
            response
                .neural_example
                .get("schemaVersion")
                .and_then(Value::as_str),
            Some("dd.fabrication.neural-example.v1")
        );

        let job = stored_learning_job(&response);
        assert_eq!(job.record.kind, "fabrication-learning-outcome");
        assert!(job.artifacts.contains_key("outcome-learning-event"));
        assert!(job.artifacts.contains_key("reward-signal"));
        assert!(job.artifacts.contains_key("mdp-experience"));
        assert!(job.artifacts.contains_key("pomdp-observations"));
        assert!(job.artifacts.contains_key("neural-example"));

        let mut memory = LearningMemory::new(8);
        memory.insert(record);
        let snapshot = memory.snapshot();
        assert_eq!(snapshot.outcome_count, 1);
        assert_eq!(snapshot.failures, 1);
        assert!(snapshot.average_reward < 0.0);
        assert!(snapshot
            .method_preferences
            .iter()
            .any(|preference| preference.key == "milling"));
    }

    #[test]
    fn compact_learning_outcomes_prefer_successful_hybrid_strategy() {
        let first_success = learning_outcome_record(LearningOutcomeRequest {
            request_id: Some("hybrid-success-1".to_string()),
            job_id: Some("plan-hybrid-1".to_string()),
            objective: Some("printed housing with turned bearing insert".to_string()),
            material: Some(material("petg", "polymer")),
            manufacturing_methods: Some(vec!["additive-print".to_string(), "turning".to_string()]),
            assembly_strategy: Some("printed body plus turned insert".to_string()),
            success: true,
            reward: Some(3.2),
            observations: Some(vec!["press-fit passed".to_string()]),
            notes: Some(vec!["reuse insert allowance".to_string()]),
        })
        .expect("first compact learning outcome should be valid");
        let second_success = learning_outcome_record(LearningOutcomeRequest {
            request_id: Some("hybrid-success-2".to_string()),
            job_id: Some("plan-hybrid-2".to_string()),
            objective: Some("printed knob with turned brass threaded core".to_string()),
            material: Some(material("petg", "polymer")),
            manufacturing_methods: Some(vec!["additive-print".to_string(), "turning".to_string()]),
            assembly_strategy: Some("printed body plus turned insert".to_string()),
            success: true,
            reward: Some(2.4),
            observations: Some(vec!["thread gauge passed".to_string()]),
            notes: Some(vec!["recorded for reuse".to_string()]),
        })
        .expect("second compact learning outcome should be valid");
        let failed_milling = learning_outcome_record(LearningOutcomeRequest {
            request_id: Some("hybrid-failure-1".to_string()),
            job_id: Some("plan-hybrid-3".to_string()),
            objective: Some("single-piece milled plastic housing".to_string()),
            material: Some(material("petg", "polymer")),
            manufacturing_methods: Some(vec!["milling".to_string()]),
            assembly_strategy: Some("single-piece machining".to_string()),
            success: false,
            reward: Some(-1.0),
            observations: Some(vec!["thin wall chatter".to_string()]),
            notes: Some(vec!["split into printed body and turned insert".to_string()]),
        })
        .expect("failed compact learning outcome should still be valid evidence");

        let mut memory = LearningMemory::new(8);
        memory.insert(first_success);
        memory.insert(second_success);
        memory.insert(failed_milling);
        let snapshot = memory.snapshot();

        assert_eq!(snapshot.outcome_count, 3);
        assert_eq!(snapshot.successes, 2);
        assert_eq!(snapshot.failures, 1);
        assert!(snapshot.method_preferences.iter().any(|preference| {
            preference.key == "additive-print"
                && preference.samples == 2
                && preference.recommendation == "prefer"
        }));
        assert!(snapshot.method_preferences.iter().any(|preference| {
            preference.key == "turning"
                && preference.samples == 2
                && preference.recommendation == "prefer"
        }));
        assert!(snapshot
            .method_combination_preferences
            .iter()
            .any(|preference| {
                preference.key == "additive-print+turning"
                    && preference.samples == 2
                    && preference.recommendation == "prefer"
            }));
        assert!(snapshot.assembly_preferences.iter().any(|preference| {
            preference.key == "printed body plus turned insert"
                && preference.samples == 2
                && preference.recommendation == "prefer"
        }));
        assert!(snapshot
            .neural_training_examples
            .iter()
            .any(|example| example.contains("methods=additive-print+turning")));
    }

    #[test]
    fn learned_policy_preferences_steer_future_plans_when_request_is_open() {
        let machines = vec![
            MachineProfile {
                id: "polymer-printer".to_string(),
                kind: "fdm-printer".to_string(),
                controller: Some("marlin".to_string()),
                materials: Some(vec!["pla".to_string()]),
                work_envelope_mm: Some(vec![220.0, 220.0, 220.0]),
                axes: Some(3),
                operations: Some(vec!["additive-print".to_string()]),
            },
            MachineProfile {
                id: "polymer-router".to_string(),
                kind: "cnc-router".to_string(),
                controller: Some("grbl-gcode".to_string()),
                materials: Some(vec!["pla".to_string()]),
                work_envelope_mm: Some(vec![600.0, 400.0, 80.0]),
                axes: Some(3),
                operations: Some(vec!["profile".to_string(), "pocket".to_string()]),
            },
        ];
        let request = FabricationPlanRequest {
            request_id: Some("unit-learned-policy-routing".to_string()),
            objective: "PLA clamp blank that can be fabricated by several cells".to_string(),
            material: Some(material("pla", "polymer")),
            stock: None,
            tolerance_mm: Some(0.25),
            quantity: Some(1),
            machines: Some(machines),
            constraints: None,
            parts: Some(vec![RequestedPart {
                id: "open-clamp-blank".to_string(),
                description: "open process blank with no caller-specified preferred method"
                    .to_string(),
                material: Some(material("pla", "polymer")),
                preferred_method: None,
                tolerance_mm: Some(0.25),
            }]),
            existing_instructions: None,
            learning: None,
        };
        let baseline = plan_fabrication(request.clone()).expect("baseline plan should succeed");
        assert!(baseline
            .design
            .parts
            .iter()
            .any(|part| part.id == "open-clamp-blank" && part.machine_kind == "fdm-printer"));

        let policy = LearningPolicySnapshot {
            outcome_count: 2,
            successes: 2,
            failures: 0,
            average_reward: 1.7,
            method_preferences: vec![LearningPreference {
                key: "routing".to_string(),
                samples: 2,
                successes: 2,
                failures: 0,
                average_reward: 1.7,
                recommendation: "prefer".to_string(),
            }],
            method_combination_preferences: Vec::new(),
            assembly_preferences: vec![LearningPreference {
                key: "printed body plus routed clamp insert".to_string(),
                samples: 2,
                successes: 2,
                failures: 0,
                average_reward: 1.5,
                recommendation: "prefer".to_string(),
            }],
            neural_training_examples: vec![
                "job=router-success-1 success=true reward=1.600 methods=routing assembly=single-piece observations=clean-tabs".to_string(),
                "job=router-success-2 success=true reward=1.800 methods=routing assembly=single-piece observations=low-intervention".to_string(),
            ],
        };
        let learned =
            plan_fabrication_with_policy(request, Some(&policy)).expect("learned plan should work");

        assert!(learned.design.parts.iter().any(|part| {
            part.id == "open-clamp-blank"
                && part.machine_kind == "cnc-router"
                && part.manufacturing_method == "subtractive-routing"
        }));
        assert!(learned.generated_programs.iter().any(|program| {
            program.part_id == "open-clamp-blank" && program.language == "grbl-gcode"
        }));
        assert!(learned
            .learning
            .training_examples
            .iter()
            .any(|example| example.contains("router-success-1")));
        assert!(learned.assembly.combine_candidates.iter().any(|candidate| {
            candidate.contains("reuse learned assembly strategy")
                && candidate.contains("printed body plus routed clamp insert")
        }));
        assert!(learned.learning.actions.iter().any(|action| {
            action == "prefer-learned-assembly-printed-body-plus-routed-clamp-insert"
        }));
    }

    #[test]
    fn learned_method_combinations_decompose_future_open_requests() {
        let first_success = learning_outcome_record(LearningOutcomeRequest {
            request_id: Some("hybrid-methods-1".to_string()),
            job_id: Some("plan-methods-1".to_string()),
            objective: Some("printed fixture body with milled datum pads".to_string()),
            material: Some(material("pla", "polymer")),
            manufacturing_methods: Some(vec!["milling".to_string(), "additive-print".to_string()]),
            assembly_strategy: Some("printed body plus milled datum pads".to_string()),
            success: true,
            reward: Some(2.2),
            observations: Some(vec!["datum inspection passed".to_string()]),
            notes: Some(vec!["reuse hybrid process".to_string()]),
        })
        .expect("first learned combination outcome should be valid");
        let second_success = learning_outcome_record(LearningOutcomeRequest {
            request_id: Some("hybrid-methods-2".to_string()),
            job_id: Some("plan-methods-2".to_string()),
            objective: Some("printed jig with milled reference ledges".to_string()),
            material: Some(material("pla", "polymer")),
            manufacturing_methods: Some(vec!["additive-print".to_string(), "milling".to_string()]),
            assembly_strategy: Some("printed body plus milled datum pads".to_string()),
            success: true,
            reward: Some(2.6),
            observations: Some(vec!["low intervention".to_string()]),
            notes: Some(vec!["same process worked again".to_string()]),
        })
        .expect("second learned combination outcome should be valid");
        let mut memory = LearningMemory::new(8);
        memory.insert(first_success);
        memory.insert(second_success);
        let snapshot = memory.snapshot();
        assert!(snapshot
            .method_combination_preferences
            .iter()
            .any(|preference| {
                preference.key == "additive-print+milling"
                    && preference.samples == 2
                    && preference.recommendation == "prefer"
            }));

        let request = FabricationPlanRequest {
            request_id: Some("unit-learned-combination".to_string()),
            objective: "PLA production aid that can be fabricated by learned shop cells"
                .to_string(),
            material: Some(material("pla", "polymer")),
            stock: None,
            tolerance_mm: Some(0.18),
            quantity: Some(1),
            machines: Some(vec![
                MachineProfile {
                    id: "polymer-printer".to_string(),
                    kind: "fdm-printer".to_string(),
                    controller: Some("marlin".to_string()),
                    materials: Some(vec!["pla".to_string()]),
                    work_envelope_mm: Some(vec![220.0, 220.0, 220.0]),
                    axes: Some(3),
                    operations: Some(vec!["additive-print".to_string()]),
                },
                MachineProfile {
                    id: "polymer-mill".to_string(),
                    kind: "vertical-mill".to_string(),
                    controller: Some("haas-gcode".to_string()),
                    materials: Some(vec!["pla".to_string()]),
                    work_envelope_mm: Some(vec![300.0, 180.0, 120.0]),
                    axes: Some(3),
                    operations: Some(vec!["face".to_string(), "contour".to_string()]),
                },
            ]),
            constraints: None,
            parts: None,
            existing_instructions: None,
            learning: None,
        };
        let learned = plan_fabrication_with_policy(request, Some(&snapshot))
            .expect("learned plan should work");

        assert!(learned.design.parts.iter().any(|part| {
            part.id == "learned-additive-print-part"
                && part.machine_kind == "fdm-printer"
                && part.manufacturing_method == "additive-print"
        }));
        assert!(learned.design.parts.iter().any(|part| {
            part.id == "learned-milling-part"
                && part.machine_kind == "vertical-mill"
                && part.manufacturing_method == "subtractive-milling"
        }));
        assert!(learned.generated_programs.iter().any(|program| {
            program.part_id == "learned-additive-print-part"
                && program.machine_id == "polymer-printer"
        }));
        assert!(learned.generated_programs.iter().any(|program| {
            program.part_id == "learned-milling-part" && program.machine_id == "polymer-mill"
        }));
        assert_eq!(
            learned.assembly.strategy,
            "learned hybrid assembly strategy: printed body plus milled datum pads"
        );
        assert!(learned.learning.actions.iter().any(|action| {
            action == "prefer-learned-method-combination-additive-print-milling"
        }));
    }

    #[test]
    fn learned_assembly_preferences_shape_future_hybrid_join_strategy() {
        let request = FabricationPlanRequest {
            request_id: Some("unit-learned-assembly".to_string()),
            objective: "PETG housing with a turned brass threaded insert".to_string(),
            material: Some(material("petg", "polymer")),
            stock: None,
            tolerance_mm: Some(0.12),
            quantity: Some(1),
            machines: None,
            constraints: None,
            parts: Some(vec![
                RequestedPart {
                    id: "printed-body".to_string(),
                    description: "printed ergonomic shell".to_string(),
                    material: Some(material("petg", "polymer")),
                    preferred_method: Some("additive-print".to_string()),
                    tolerance_mm: Some(0.18),
                },
                RequestedPart {
                    id: "turned-insert".to_string(),
                    description: "turned brass threaded insert".to_string(),
                    material: Some(material("brass", "metal")),
                    preferred_method: Some("turning".to_string()),
                    tolerance_mm: Some(0.04),
                },
            ]),
            existing_instructions: None,
            learning: None,
        };
        let baseline = plan_fabrication(request.clone()).expect("baseline plan should succeed");
        assert_eq!(
            baseline.assembly.strategy,
            "multi-part hybrid fabrication with explicit assembly interfaces"
        );

        let policy = LearningPolicySnapshot {
            outcome_count: 2,
            successes: 2,
            failures: 0,
            average_reward: 2.8,
            method_preferences: Vec::new(),
            method_combination_preferences: Vec::new(),
            assembly_preferences: vec![LearningPreference {
                key: "printed body plus turned insert".to_string(),
                samples: 2,
                successes: 2,
                failures: 0,
                average_reward: 2.8,
                recommendation: "prefer".to_string(),
            }],
            neural_training_examples: vec![
                "job=hybrid-success-1 success=true reward=3.200 methods=additive-print+turning assembly=printed body plus turned insert observations=press-fit-pass".to_string(),
            ],
        };
        let mut restricted_request = request.clone();
        restricted_request.constraints = Some(FabricationConstraints {
            max_setups: None,
            allow_human_intervention: None,
            allow_multi_part_assembly: Some(false),
            require_dry_run: None,
            preferred_methods: None,
            preferred_assembly_strategy: None,
        });
        let restricted = plan_fabrication_with_policy(restricted_request, Some(&policy))
            .expect("restricted learned plan should still work");
        assert_eq!(
            restricted.assembly.strategy,
            "single-piece preference; review split candidates before approving"
        );

        let learned =
            plan_fabrication_with_policy(request, Some(&policy)).expect("learned plan should work");

        assert_eq!(
            learned.assembly.strategy,
            "learned hybrid assembly strategy: printed body plus turned insert"
        );
        assert!(learned.assembly.combine_candidates.iter().any(|candidate| {
            candidate.contains("reuse learned assembly strategy")
                && candidate.contains("printed body plus turned insert")
        }));
        assert!(learned
            .assembly
            .notes
            .iter()
            .any(|note| note.contains("Learned policy prefers assembly strategy")));
        assert!(learned
            .learning
            .training_examples
            .iter()
            .any(|example| example.contains("hybrid-success-1")));
    }

    #[test]
    fn plan_job_store_records_design_program_and_learning_artifacts() {
        let response = plan_fabrication(FabricationPlanRequest {
            request_id: Some("unit-artifact-plan".to_string()),
            objective: "PLA prototype cover with a machined datum face".to_string(),
            material: Some(material("pla", "polymer")),
            stock: None,
            tolerance_mm: Some(0.1),
            quantity: Some(1),
            machines: None,
            constraints: None,
            parts: None,
            existing_instructions: None,
            learning: None,
        })
        .expect("plan should succeed");

        assert!(response.job_id.starts_with("plan-unit-artifact-plan-"));
        let job = stored_plan_job(&response);
        assert_eq!(job.record.job_id, response.job_id);
        assert!(job.artifacts.contains_key("design-summary"));
        assert!(job.artifacts.contains_key("parametric-design"));
        assert!(job.artifacts.contains_key("mdp-request"));
        assert!(job.artifacts.contains_key("simulation-report"));
        assert!(response.simulation.ok);
        assert!(job
            .artifacts
            .keys()
            .any(|artifact_id| artifact_id.starts_with("program-")));
        let parametric_design = job
            .artifacts
            .get("parametric-design")
            .expect("parametric design artifact should be retained");
        assert_eq!(
            parametric_design
                .content
                .get("schemaVersion")
                .and_then(Value::as_str),
            Some("dd.fabrication.parametric-design.v1")
        );
        assert!(parametric_design
            .content
            .get("parts")
            .and_then(Value::as_array)
            .is_some_and(|parts| !parts.is_empty()));
        assert_eq!(parametric_design.machine_ready, false);

        let mut store = FabricationJobStore::new(2);
        store.insert(job);
        let (job_count, artifact_count) = store.counts();
        assert_eq!(job_count, 1);
        assert!(artifact_count >= 3);
        let detail = store
            .detail(&response.job_id)
            .expect("stored plan should be retrievable");
        assert_eq!(detail.record.kind, "fabrication-plan");
        assert!(detail.plan.is_some());
        assert!(detail
            .artifacts
            .iter()
            .any(|artifact| artifact.artifact_id == "learning-plan"));
    }

    #[test]
    fn oversized_stock_creates_machine_envelope_failure_boundary() {
        let response = plan_fabrication(FabricationPlanRequest {
            request_id: Some("unit-envelope".to_string()),
            objective: "large PLA printer cover".to_string(),
            material: Some(material("pla", "polymer")),
            stock: Some(StockSpec {
                form: "sheet".to_string(),
                dimensions_mm: Some(vec![300.0, 300.0, 80.0]),
            }),
            tolerance_mm: Some(0.2),
            quantity: Some(1),
            machines: Some(vec![MachineProfile {
                id: "small-printer".to_string(),
                kind: "fdm-printer".to_string(),
                controller: Some("marlin".to_string()),
                materials: Some(vec!["pla".to_string()]),
                work_envelope_mm: Some(vec![120.0, 120.0, 120.0]),
                axes: Some(3),
                operations: Some(vec!["additive-print".to_string()]),
            }]),
            constraints: None,
            parts: None,
            existing_instructions: None,
            learning: None,
        })
        .expect("oversize plan should still return a validation report");

        assert!(!response.ok);
        assert_eq!(response.validation.severity, "error");
        assert!(response
            .validation
            .failure_boundaries
            .iter()
            .any(|boundary| {
                boundary.kind == "machine-envelope"
                    && boundary.requires_human_intervention
                    && boundary.suggested_resolution.contains("split the part")
            }));
    }

    #[test]
    fn analysis_job_store_records_improved_instruction_artifacts() {
        let programs = vec![program(
            "legacy-print",
            "fdm-printer",
            &["G21", "G90", "G1 X10 Y10 E2.0 F900"],
        )];
        let (analyzed, validation, improvements) = analyze_instruction_programs(&programs);
        let simulation = simulate_instruction_programs(&programs, &default_machines());
        let improved_programs = improve_instruction_programs(&programs, &validation, &improvements);
        let generated_at_ms = now_ms();
        let response = InstructionAnalysisResponse {
            ok: validation.ok,
            job_id: safe_job_id("analysis", "unit-analysis-artifacts", generated_at_ms),
            request_id: "unit-analysis-artifacts".to_string(),
            programs: analyzed,
            validation,
            simulation,
            improvements,
            improved_programs,
            generated_at_ms,
        };

        let job = stored_analysis_job(&response);
        assert_eq!(job.record.kind, "instruction-analysis");
        assert!(job.artifacts.contains_key("analysis-validation-report"));
        assert!(job.artifacts.contains_key("analysis-simulation-report"));
        assert!(job
            .artifacts
            .keys()
            .any(|artifact_id| artifact_id.starts_with("improved-program-")));
        let improved_artifact = job
            .artifacts
            .values()
            .find(|artifact| artifact.kind == "improved-instruction-program")
            .expect("improved program artifact should exist");
        assert!(improved_artifact.draft);
        assert!(!improved_artifact.machine_ready);
        assert!(improved_artifact.line_count.unwrap_or_default() >= 3);
    }
}
