use std::{
    collections::{HashMap, HashSet},
    env,
    error::Error,
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
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::json;

const MAX_HTTP_BODY_BYTES: usize = 256 * 1024;
const MAX_NATS_PAYLOAD_BYTES: usize = 256 * 1024;
const MAX_TELEMETRY_SIGNALS: usize = 128;
const MAX_TELEMETRY_ACTIONS: usize = 32;
const MAX_TELEMETRY_IMPACTS_PER_SIGNAL: usize = 16;
const MAX_TELEMETRY_TOKEN_LEN: usize = 96;
const MAX_TELEMETRY_REQUEST_ID_LEN: usize = 128;
const MAX_TELEMETRY_WINDOW_MS: u64 = 24 * 60 * 60 * 1000;
const MAX_TELEMETRY_SIGNAL_WEIGHT: f64 = 100.0;

#[derive(Clone)]
struct AppState {
    nats: Option<async_nats::Client>,
    result_subject: String,
    event_subject: String,
    metrics: Arc<Metrics>,
}

#[derive(Default)]
struct Metrics {
    requests_total: AtomicU64,
    telemetry_requests_total: AtomicU64,
    optimizations_total: AtomicU64,
    errors_total: AtomicU64,
    nats_messages_total: AtomicU64,
    nats_telemetry_messages_total: AtomicU64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OptimizationRequest {
    request_id: Option<String>,
    kind: Option<String>,
    states: Vec<String>,
    actions: Vec<String>,
    transitions: Vec<TransitionInput>,
    rewards: Vec<RewardInput>,
    observations: Option<Vec<String>>,
    observation_model: Option<Vec<ObservationInput>>,
    belief: Option<Vec<BeliefInput>>,
    belief_action: Option<String>,
    observed: Option<String>,
    gamma: Option<f64>,
    tolerance: Option<f64>,
    max_iterations: Option<usize>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TransitionInput {
    state: String,
    action: String,
    next_state: String,
    probability: f64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RewardInput {
    state: String,
    action: String,
    value: f64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ObservationInput {
    action: String,
    next_state: String,
    observation: String,
    probability: f64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BeliefInput {
    state: String,
    probability: f64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TelemetryLearningRequest {
    request_id: Option<String>,
    scope: Option<String>,
    window_ms: Option<u64>,
    signals: Vec<TelemetrySignalInput>,
    actions: Option<Vec<String>>,
    gamma: Option<f64>,
    tolerance: Option<f64>,
    max_iterations: Option<usize>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TelemetrySignalInput {
    name: String,
    service: Option<String>,
    layer: Option<String>,
    value: f64,
    baseline: Option<f64>,
    target: Option<f64>,
    warning: Option<f64>,
    critical: Option<f64>,
    weight: Option<f64>,
    higher_is_better: Option<bool>,
    action_impacts: Option<Vec<TelemetryActionImpactInput>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TelemetryActionImpactInput {
    action: String,
    delta: f64,
    confidence: Option<f64>,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct OptimizationResponse {
    ok: bool,
    request_id: String,
    kind: String,
    gamma: f64,
    tolerance: f64,
    iterations: usize,
    converged: bool,
    residual: f64,
    policy: Vec<PolicyEntry>,
    values: Vec<StateValue>,
    q_values: Vec<ActionValue>,
    belief: Option<BeliefSummary>,
    warnings: Vec<String>,
    generated_at_ms: u128,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct PolicyEntry {
    state: String,
    action: String,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct StateValue {
    state: String,
    value: f64,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct ActionValue {
    state: String,
    action: String,
    value: f64,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct BeliefSummary {
    input: Vec<StateValue>,
    action_values: Vec<ActionBeliefValue>,
    selected_action: Option<String>,
    posterior: Option<Vec<StateValue>>,
    observed: Option<String>,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct ActionBeliefValue {
    action: String,
    value: f64,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct TelemetryLearningResponse {
    ok: bool,
    request_id: String,
    kind: String,
    scope: String,
    window_ms: Option<u64>,
    risk: f64,
    state: String,
    recommended_action: String,
    insights: Vec<TelemetryInsight>,
    optimization: OptimizationResponse,
    warnings: Vec<String>,
    generated_at_ms: u128,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct TelemetryInsight {
    layer: String,
    service: String,
    signal: String,
    value: f64,
    risk: f64,
    state: String,
    recommended_action: String,
    reason: String,
}

struct ScoredTelemetrySignal {
    name: String,
    service: String,
    layer: String,
    value: f64,
    risk: f64,
    weight: f64,
    action_impacts: Vec<(String, f64)>,
}

struct ObservationModelTable {
    observation_index: HashMap<String, usize>,
    probabilities: Vec<Vec<Vec<f64>>>,
}

fn env_value(key: &str, fallback: &str) -> String {
    env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| fallback.to_string())
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn finite_probability(value: f64, label: &str) -> Result<f64, String> {
    if !value.is_finite() || value < 0.0 {
        return Err(format!("{label} must be finite and non-negative"));
    }
    Ok(value)
}

fn finite_value(value: f64, label: &str) -> Result<f64, String> {
    if !value.is_finite() {
        return Err(format!("{label} must be finite"));
    }
    Ok(value)
}

fn index_of(values: &[String]) -> HashMap<String, usize> {
    values
        .iter()
        .enumerate()
        .map(|(index, value)| (value.clone(), index))
        .collect()
}

fn validate_unique_labels(values: &[String], label: &str) -> Result<(), String> {
    let mut seen = HashSet::with_capacity(values.len());
    for value in values {
        if !seen.insert(value.as_str()) {
            return Err(format!("{label} must be unique; duplicate {value}"));
        }
    }
    Ok(())
}

fn validate_telemetry_token(value: &str, label: &str) -> Result<String, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(format!("{label} must not be empty"));
    }
    if trimmed.len() > MAX_TELEMETRY_TOKEN_LEN {
        return Err(format!(
            "{label} must be at most {MAX_TELEMETRY_TOKEN_LEN} bytes"
        ));
    }
    if trimmed.chars().any(|character| character.is_control()) {
        return Err(format!("{label} must not contain control characters"));
    }
    Ok(trimmed.to_string())
}

fn validate_telemetry_request_id(value: Option<String>) -> Result<Option<String>, String> {
    value
        .map(|request_id| {
            let trimmed = request_id.trim();
            if trimmed.is_empty() {
                return Err("telemetry requestId must not be empty when provided".to_string());
            }
            if trimmed.len() > MAX_TELEMETRY_REQUEST_ID_LEN {
                return Err(format!(
                    "telemetry requestId must be at most {MAX_TELEMETRY_REQUEST_ID_LEN} bytes"
                ));
            }
            if trimmed.chars().any(|character| character.is_control()) {
                return Err("telemetry requestId must not contain control characters".to_string());
            }
            Ok(trimmed.to_string())
        })
        .transpose()
}

fn normalize_telemetry_action(action: &str) -> Result<String, String> {
    let token = validate_telemetry_token(action, "telemetry action")?.to_ascii_lowercase();
    if token.chars().any(|character| {
        !(character.is_ascii_alphanumeric()
            || character == '-'
            || character == '_'
            || character == '.'
            || character == ':')
    }) {
        return Err(
            "telemetry action may only contain letters, numbers, '-', '_', '.', or ':'".to_string(),
        );
    }
    Ok(token)
}

fn normalize_telemetry_dimension(value: Option<&str>, fallback: &str) -> Result<String, String> {
    let raw = value.unwrap_or(fallback);
    let token = validate_telemetry_token(raw, "telemetry scope or layer")?.to_ascii_lowercase();
    Ok(match token.as_str() {
        "application" => "app".to_string(),
        "infrastructure" | "platform" | "observability" | "messaging" | "database" | "data" => {
            "infra".to_string()
        }
        "app" | "infra" | "mixed" => token,
        _ => {
            return Err(format!(
                "unsupported telemetry scope or layer {token}; expected app, infra, or mixed"
            ));
        }
    })
}

fn validate_telemetry_window(window_ms: Option<u64>) -> Result<Option<u64>, String> {
    if let Some(window_ms) = window_ms {
        if window_ms == 0 {
            return Err("telemetry windowMs must be positive when provided".to_string());
        }
        if window_ms > MAX_TELEMETRY_WINDOW_MS {
            return Err(format!(
                "telemetry windowMs must be at most {MAX_TELEMETRY_WINDOW_MS}"
            ));
        }
    }
    Ok(window_ms)
}

fn finite_ratio(value: f64, label: &str) -> Result<f64, String> {
    if !value.is_finite() || !(0.0..=1.0).contains(&value) {
        return Err(format!("{label} must be finite and in [0, 1]"));
    }
    Ok(value)
}

fn bounded_impact_delta(value: f64) -> Result<f64, String> {
    if !value.is_finite() || !(-1.0..=1.0).contains(&value) {
        return Err("telemetry action impact delta must be finite and in [-1, 1]".to_string());
    }
    Ok(value)
}

fn normalize(values: &mut [f64]) -> f64 {
    let sum = values.iter().copied().sum::<f64>();
    if sum > 0.0 {
        for value in values {
            *value /= sum;
        }
    }
    sum
}

fn clamp01(value: f64) -> f64 {
    if value <= 0.0 {
        0.0
    } else if value >= 1.0 {
        1.0
    } else {
        value
    }
}

fn state_for_risk(risk: f64) -> &'static str {
    if risk >= 0.75 {
        "critical"
    } else if risk >= 0.5 {
        "degraded"
    } else if risk >= 0.25 {
        "watch"
    } else {
        "nominal"
    }
}

fn state_risk_center(state: &str) -> f64 {
    match state {
        "critical" => 0.9,
        "degraded" => 0.65,
        "watch" => 0.35,
        _ => 0.1,
    }
}

fn default_telemetry_actions(scope: &str, signals: &[ScoredTelemetrySignal]) -> Vec<String> {
    let mut actions = vec!["hold".to_string(), "observe".to_string()];
    let has_infra = signals.iter().any(|signal| signal.layer == "infra") || scope == "infra";
    let has_app = signals.iter().any(|signal| signal.layer == "app") || scope == "app";

    if has_infra {
        actions.extend(
            ["scale-up", "restart", "shed-load"]
                .iter()
                .map(|value| value.to_string()),
        );
    }
    if has_app {
        actions.extend(
            ["enable-fallback", "throttle-feature", "disable-experiment"]
                .iter()
                .map(|value| value.to_string()),
        );
    }
    actions.push("page-human".to_string());

    for signal in signals {
        for (action, _) in &signal.action_impacts {
            actions.push(action.clone());
        }
    }
    dedupe_strings(actions)
}

fn normalize_telemetry_actions(actions: Vec<String>) -> Result<Vec<String>, String> {
    if actions.is_empty() {
        return Err("telemetry actions must not be empty when provided".to_string());
    }
    if actions.len() > MAX_TELEMETRY_ACTIONS {
        return Err(format!(
            "telemetry actions must contain at most {MAX_TELEMETRY_ACTIONS} entries"
        ));
    }
    let normalized = actions
        .iter()
        .map(|action| normalize_telemetry_action(action))
        .collect::<Result<Vec<_>, _>>()?;
    validate_unique_labels(&normalized, "telemetry actions")?;
    Ok(normalized)
}

fn dedupe_strings(values: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::with_capacity(values.len());
    let mut deduped = Vec::with_capacity(values.len());
    for value in values {
        if seen.insert(value.clone()) {
            deduped.push(value);
        }
    }
    deduped
}

fn telemetry_action_cost(action: &str) -> f64 {
    match action {
        "hold" => 0.0,
        "observe" => 0.02,
        "scale-up" => 0.1,
        "throttle-feature" => 0.12,
        "enable-fallback" => 0.16,
        "disable-experiment" => 0.18,
        "shed-load" => 0.2,
        "restart" => 0.22,
        "page-human" => 0.42,
        _ => 0.15,
    }
}

fn default_telemetry_action_efficacy(action: &str, scope: &str) -> f64 {
    match action {
        "hold" => 0.0,
        "observe" => 0.06,
        "scale-up" => {
            if scope == "app" {
                0.12
            } else {
                0.26
            }
        }
        "restart" => 0.2,
        "shed-load" => 0.24,
        "enable-fallback" => 0.27,
        "throttle-feature" => 0.22,
        "disable-experiment" => 0.18,
        "page-human" => 0.34,
        _ => 0.12,
    }
}

fn risk_from_thresholds(value: f64, warning: f64, critical: f64, higher_is_better: bool) -> f64 {
    if (warning - critical).abs() <= f64::EPSILON {
        return if higher_is_better {
            if value <= critical {
                1.0
            } else {
                0.0
            }
        } else if value >= critical {
            1.0
        } else {
            0.0
        };
    }

    if higher_is_better {
        let safe = warning.max(critical);
        let bad = warning.min(critical);
        clamp01((safe - value) / (safe - bad))
    } else {
        let safe = warning.min(critical);
        let bad = warning.max(critical);
        clamp01((value - safe) / (bad - safe))
    }
}

fn risk_from_reference(value: f64, reference: f64, higher_is_better: bool) -> f64 {
    let denominator = reference.abs().max(1.0);
    if higher_is_better {
        clamp01((reference - value) / denominator)
    } else {
        clamp01((value - reference) / denominator)
    }
}

fn score_telemetry_signal(signal: &TelemetrySignalInput) -> Result<ScoredTelemetrySignal, String> {
    let name = validate_telemetry_token(&signal.name, "telemetry signal name")?;
    let service = validate_telemetry_token(
        signal.service.as_deref().unwrap_or("unknown-service"),
        "telemetry signal service",
    )?;
    let layer = normalize_telemetry_dimension(signal.layer.as_deref(), "app")?;
    if let Some(impacts) = signal.action_impacts.as_ref() {
        if impacts.len() > MAX_TELEMETRY_IMPACTS_PER_SIGNAL {
            return Err(format!(
                "telemetry signal {name} must contain at most {MAX_TELEMETRY_IMPACTS_PER_SIGNAL} action impacts"
            ));
        }
    }
    let value = finite_value(signal.value, "telemetry signal value")?;
    let weight = finite_probability(signal.weight.unwrap_or(1.0), "telemetry signal weight")?;
    if weight > MAX_TELEMETRY_SIGNAL_WEIGHT {
        return Err(format!(
            "telemetry signal weight must be at most {MAX_TELEMETRY_SIGNAL_WEIGHT}"
        ));
    }
    let higher_is_better = signal.higher_is_better.unwrap_or(false);
    let risk = match (
        signal.warning,
        signal.critical,
        signal.target,
        signal.baseline,
    ) {
        (Some(warning), Some(critical), _, _) => risk_from_thresholds(
            value,
            finite_value(warning, "telemetry warning")?,
            finite_value(critical, "telemetry critical")?,
            higher_is_better,
        ),
        (_, _, Some(target), _) => risk_from_reference(
            value,
            finite_value(target, "telemetry target")?,
            higher_is_better,
        ),
        (_, _, _, Some(baseline)) => risk_from_reference(
            value,
            finite_value(baseline, "telemetry baseline")?,
            higher_is_better,
        ),
        _ if (0.0..=1.0).contains(&value) => {
            if higher_is_better {
                1.0 - value
            } else {
                value
            }
        }
        _ => 0.0,
    };

    let mut action_impacts = Vec::new();
    for impact in signal.action_impacts.as_deref().unwrap_or(&[]) {
        let action = normalize_telemetry_action(&impact.action)?;
        let delta = bounded_impact_delta(impact.delta)?;
        let confidence = finite_ratio(
            impact.confidence.unwrap_or(1.0),
            "telemetry action impact confidence",
        )?;
        action_impacts.push((action, delta * confidence));
    }

    Ok(ScoredTelemetrySignal {
        name,
        service,
        layer,
        value,
        risk: clamp01(risk),
        weight,
        action_impacts,
    })
}

fn aggregate_telemetry_risk(signals: &[ScoredTelemetrySignal]) -> Result<f64, String> {
    let total_weight = signals.iter().map(|signal| signal.weight).sum::<f64>();
    if total_weight <= 0.0 {
        return Err("telemetry signal weights sum to zero".to_string());
    }
    Ok(signals
        .iter()
        .map(|signal| signal.risk * signal.weight)
        .sum::<f64>()
        / total_weight)
}

fn telemetry_action_efficacy(
    action: &str,
    scope: &str,
    signals: &[ScoredTelemetrySignal],
    warnings: &mut Vec<String>,
) -> f64 {
    let mut weighted_delta = 0.0;
    let mut total_weight = 0.0;
    for signal in signals {
        for (impact_action, delta) in &signal.action_impacts {
            if impact_action == action {
                weighted_delta += delta * signal.weight * signal.risk.max(0.1);
                total_weight += signal.weight * signal.risk.max(0.1);
            }
        }
    }
    if total_weight > 0.0 {
        let custom = weighted_delta / total_weight;
        if custom < 0.0 {
            warnings.push(format!(
                "telemetry action impact for action={action} lowers expected recovery"
            ));
        }
        return clamp01(custom);
    }
    clamp01(default_telemetry_action_efficacy(action, scope))
}

fn telemetry_optimization_request(
    request_id: &str,
    scope: &str,
    risk: f64,
    actions: Vec<String>,
    signals: &[ScoredTelemetrySignal],
    request: &TelemetryLearningRequest,
    warnings: &mut Vec<String>,
) -> OptimizationRequest {
    let states = vec![
        "nominal".to_string(),
        "watch".to_string(),
        "degraded".to_string(),
        "critical".to_string(),
    ];
    let mut transitions = Vec::new();
    let mut rewards = Vec::new();

    for state in &states {
        let center = state_risk_center(state);
        for action in &actions {
            let efficacy = telemetry_action_efficacy(action, scope, signals, warnings);
            let next_risk = clamp01(center + risk * 0.22 - efficacy);
            let next_state = state_for_risk(next_risk).to_string();
            if next_state == *state {
                transitions.push(TransitionInput {
                    state: state.clone(),
                    action: action.clone(),
                    next_state,
                    probability: 1.0,
                });
            } else {
                transitions.push(TransitionInput {
                    state: state.clone(),
                    action: action.clone(),
                    next_state,
                    probability: 0.78,
                });
                transitions.push(TransitionInput {
                    state: state.clone(),
                    action: action.clone(),
                    next_state: state.clone(),
                    probability: 0.22,
                });
            }
            rewards.push(RewardInput {
                state: state.clone(),
                action: action.clone(),
                value: efficacy * (0.9 + risk) - telemetry_action_cost(action) - center * 1.8,
            });
        }
    }

    OptimizationRequest {
        request_id: Some(format!("{request_id}:telemetry-policy")),
        kind: Some("telemetry.mdp.value-iteration".to_string()),
        states,
        actions,
        transitions,
        rewards,
        observations: None,
        observation_model: None,
        belief: None,
        belief_action: None,
        observed: None,
        gamma: request.gamma.or(Some(0.82)),
        tolerance: request.tolerance.or(Some(1e-8)),
        max_iterations: request.max_iterations.or(Some(2_000)),
    }
}

fn policy_action_for_state(response: &OptimizationResponse, state: &str) -> Option<String> {
    response
        .policy
        .iter()
        .find(|entry| entry.state == state)
        .map(|entry| entry.action.clone())
}

fn optimize_telemetry(
    request: TelemetryLearningRequest,
) -> Result<TelemetryLearningResponse, String> {
    if request.signals.is_empty() {
        return Err("telemetry signals must not be empty".to_string());
    }
    if request.signals.len() > MAX_TELEMETRY_SIGNALS {
        return Err(format!(
            "telemetry signals must contain at most {MAX_TELEMETRY_SIGNALS} entries"
        ));
    }
    let request_id = validate_telemetry_request_id(request.request_id.clone())?
        .unwrap_or_else(|| format!("telemetry-mdp-{}", now_ms()));
    let window_ms = validate_telemetry_window(request.window_ms)?;
    let mut warnings = Vec::new();
    let scored = request
        .signals
        .iter()
        .map(score_telemetry_signal)
        .collect::<Result<Vec<_>, _>>()?;

    let scope = normalize_telemetry_dimension(request.scope.as_deref(), "mixed")?;
    let risk = aggregate_telemetry_risk(&scored)?;
    let state = state_for_risk(risk).to_string();
    let actions = match request.actions.clone() {
        Some(actions) => normalize_telemetry_actions(actions)?,
        None => normalize_telemetry_actions(default_telemetry_actions(&scope, &scored))?,
    };

    let optimization_request = telemetry_optimization_request(
        &request_id,
        &scope,
        risk,
        actions,
        &scored,
        &request,
        &mut warnings,
    );
    let optimization = optimize(optimization_request)?;
    let recommended_action = policy_action_for_state(&optimization, &state)
        .or_else(|| {
            optimization
                .policy
                .first()
                .map(|entry| entry.action.clone())
        })
        .unwrap_or_else(|| "observe".to_string());

    let mut insights = scored
        .iter()
        .map(|signal| {
            let signal_state = state_for_risk(signal.risk).to_string();
            TelemetryInsight {
                layer: signal.layer.clone(),
                service: signal.service.clone(),
                signal: signal.name.clone(),
                value: signal.value,
                risk: signal.risk,
                state: signal_state.clone(),
                recommended_action: recommended_action.clone(),
                reason: format!(
                    "{} telemetry from {} maps to {} risk at {:.3}",
                    signal.layer, signal.service, signal_state, signal.risk
                ),
            }
        })
        .collect::<Vec<_>>();
    insights.sort_by(|left, right| right.risk.total_cmp(&left.risk));

    Ok(TelemetryLearningResponse {
        ok: true,
        request_id,
        kind: "telemetry.mdp.insight".to_string(),
        scope,
        window_ms,
        risk,
        state,
        recommended_action,
        insights,
        optimization,
        warnings,
        generated_at_ms: now_ms(),
    })
}

fn optimize(request: OptimizationRequest) -> Result<OptimizationResponse, String> {
    if request.states.is_empty() {
        return Err("states must not be empty".to_string());
    }
    if request.actions.is_empty() {
        return Err("actions must not be empty".to_string());
    }
    validate_unique_labels(&request.states, "states")?;
    validate_unique_labels(&request.actions, "actions")?;

    let gamma = request.gamma.unwrap_or(0.95);
    if !gamma.is_finite() || !(0.0..1.0).contains(&gamma) {
        return Err("gamma must be finite and in [0, 1)".to_string());
    }
    let tolerance = request.tolerance.unwrap_or(1e-8);
    if !tolerance.is_finite() || tolerance <= 0.0 {
        return Err("tolerance must be finite and positive".to_string());
    }
    let max_iterations = request.max_iterations.unwrap_or(10_000).clamp(1, 1_000_000);
    if let Some(observations) = &request.observations {
        if observations.is_empty() {
            return Err("observations must not be empty when provided".to_string());
        }
        validate_unique_labels(observations, "observations")?;
        if let Some(observed) = &request.observed {
            if !observations.iter().any(|candidate| candidate == observed) {
                return Err(format!(
                    "observed value {observed} is not listed in observations"
                ));
            }
        }
    }

    let state_index = index_of(&request.states);
    let action_index = index_of(&request.actions);
    let n_states = request.states.len();
    let n_actions = request.actions.len();
    if let Some(action) = &request.belief_action {
        if !action_index.contains_key(action) {
            return Err(format!("unknown beliefAction {action}"));
        }
    }
    let mut warnings = Vec::new();
    let mut transition = vec![vec![vec![0.0; n_states]; n_actions]; n_states];
    let mut reward = vec![vec![0.0; n_actions]; n_states];

    for item in &request.transitions {
        let s = *state_index
            .get(&item.state)
            .ok_or_else(|| format!("unknown transition state {}", item.state))?;
        let a = *action_index
            .get(&item.action)
            .ok_or_else(|| format!("unknown transition action {}", item.action))?;
        let next = *state_index
            .get(&item.next_state)
            .ok_or_else(|| format!("unknown transition nextState {}", item.next_state))?;
        transition[s][a][next] += finite_probability(item.probability, "transition probability")?;
    }

    for s in 0..n_states {
        for a in 0..n_actions {
            let sum = normalize(&mut transition[s][a]);
            if sum == 0.0 {
                transition[s][a][s] = 1.0;
                warnings.push(format!(
                    "missing transition for state={} action={}; using self-loop",
                    request.states[s], request.actions[a]
                ));
            } else if (sum - 1.0).abs() > 1e-9 {
                warnings.push(format!(
                    "normalized transition probabilities for state={} action={} from sum={sum}",
                    request.states[s], request.actions[a]
                ));
            }
        }
    }

    for item in &request.rewards {
        let s = *state_index
            .get(&item.state)
            .ok_or_else(|| format!("unknown reward state {}", item.state))?;
        let a = *action_index
            .get(&item.action)
            .ok_or_else(|| format!("unknown reward action {}", item.action))?;
        reward[s][a] = finite_value(item.value, "reward value")?;
    }

    let mut values = vec![0.0; n_states];
    let mut q_values = vec![vec![0.0; n_actions]; n_states];
    let mut policy_index = vec![0usize; n_states];
    let mut residual = f64::INFINITY;
    let mut iterations = 0usize;
    let mut converged = false;

    for iteration in 1..=max_iterations {
        iterations = iteration;
        let previous = values.clone();
        residual = 0.0;
        for s in 0..n_states {
            let mut best_value = f64::NEG_INFINITY;
            let mut best_action = 0usize;
            for a in 0..n_actions {
                let future = transition[s][a]
                    .iter()
                    .zip(previous.iter())
                    .map(|(probability, value)| probability * value)
                    .sum::<f64>();
                let value = reward[s][a] + gamma * future;
                q_values[s][a] = value;
                if value > best_value {
                    best_value = value;
                    best_action = a;
                }
            }
            values[s] = best_value;
            policy_index[s] = best_action;
            residual = residual.max((values[s] - previous[s]).abs());
        }
        if residual <= tolerance {
            converged = true;
            break;
        }
    }

    let belief = build_belief_summary(
        &request,
        &state_index,
        &action_index,
        &transition,
        &reward,
        &values,
        gamma,
        &mut warnings,
    )?;

    let mut q_value_entries = Vec::with_capacity(n_states * n_actions);
    for (s, state) in request.states.iter().enumerate() {
        for (a, action) in request.actions.iter().enumerate() {
            q_value_entries.push(ActionValue {
                state: state.clone(),
                action: action.clone(),
                value: q_values[s][a],
            });
        }
    }

    Ok(OptimizationResponse {
        ok: true,
        request_id: request
            .request_id
            .unwrap_or_else(|| format!("mdp-{}", now_ms())),
        kind: request
            .kind
            .unwrap_or_else(|| "mdp.value-iteration".to_string()),
        gamma,
        tolerance,
        iterations,
        converged,
        residual,
        policy: request
            .states
            .iter()
            .enumerate()
            .map(|(s, state)| PolicyEntry {
                state: state.clone(),
                action: request.actions[policy_index[s]].clone(),
            })
            .collect(),
        values: request
            .states
            .iter()
            .enumerate()
            .map(|(s, state)| StateValue {
                state: state.clone(),
                value: values[s],
            })
            .collect(),
        q_values: q_value_entries,
        belief,
        warnings,
        generated_at_ms: now_ms(),
    })
}

async fn optimize_in_background(
    request: OptimizationRequest,
) -> Result<OptimizationResponse, String> {
    tokio::task::spawn_blocking(move || optimize(request))
        .await
        .map_err(|error| format!("optimization task join failed: {error}"))?
}

async fn optimize_telemetry_in_background(
    request: TelemetryLearningRequest,
) -> Result<TelemetryLearningResponse, String> {
    tokio::task::spawn_blocking(move || optimize_telemetry(request))
        .await
        .map_err(|error| format!("telemetry optimization task join failed: {error}"))?
}

fn build_belief_summary(
    request: &OptimizationRequest,
    state_index: &HashMap<String, usize>,
    action_index: &HashMap<String, usize>,
    transition: &[Vec<Vec<f64>>],
    reward: &[Vec<f64>],
    values: &[f64],
    gamma: f64,
    warnings: &mut Vec<String>,
) -> Result<Option<BeliefSummary>, String> {
    let observation_table = build_observation_model(request, state_index, action_index, warnings)?;

    let Some(input_belief) = &request.belief else {
        return Ok(None);
    };
    let mut belief = vec![0.0; request.states.len()];
    for item in input_belief {
        let s = *state_index
            .get(&item.state)
            .ok_or_else(|| format!("unknown belief state {}", item.state))?;
        belief[s] += finite_probability(item.probability, "belief probability")?;
    }
    if normalize(&mut belief) == 0.0 {
        return Err("belief probabilities sum to zero".to_string());
    }

    let action_values = request
        .actions
        .iter()
        .enumerate()
        .map(|(a, action)| {
            let immediate = belief
                .iter()
                .enumerate()
                .map(|(s, probability)| probability * reward[s][a])
                .sum::<f64>();
            let mut predicted = vec![0.0; request.states.len()];
            for s in 0..request.states.len() {
                for next in 0..request.states.len() {
                    predicted[next] += belief[s] * transition[s][a][next];
                }
            }
            let future = predicted
                .iter()
                .zip(values.iter())
                .map(|(probability, value)| probability * value)
                .sum::<f64>();
            ActionBeliefValue {
                action: action.clone(),
                value: immediate + gamma * future,
            }
        })
        .collect::<Vec<_>>();

    let selected_action = match request.belief_action.as_ref() {
        Some(action) => Some(action.clone()),
        None => best_belief_action(&action_values).map(|entry| entry.action.clone()),
    };

    let posterior = match (
        request.observed.as_ref(),
        observation_table.as_ref(),
        selected_action.as_ref(),
    ) {
        (Some(observed), Some(model), Some(action)) => {
            let a = *action_index
                .get(action)
                .ok_or_else(|| format!("unknown belief action {action}"))?;
            let observed_index = *model.observation_index.get(observed).ok_or_else(|| {
                format!("observed value {observed} is not listed in observations")
            })?;
            let predicted = predict_next_belief(&belief, transition, a);
            let mut posterior = vec![0.0; request.states.len()];
            for next in 0..request.states.len() {
                posterior[next] = predicted[next] * model.probabilities[a][next][observed_index];
            }
            if normalize(&mut posterior) == 0.0 {
                None
            } else {
                Some(
                    request
                        .states
                        .iter()
                        .enumerate()
                        .map(|(s, state)| StateValue {
                            state: state.clone(),
                            value: posterior[s],
                        })
                        .collect(),
                )
            }
        }
        _ => None,
    };

    Ok(Some(BeliefSummary {
        input: request
            .states
            .iter()
            .enumerate()
            .map(|(s, state)| StateValue {
                state: state.clone(),
                value: belief[s],
            })
            .collect(),
        action_values,
        selected_action,
        posterior,
        observed: request.observed.clone(),
    }))
}

fn build_observation_model(
    request: &OptimizationRequest,
    state_index: &HashMap<String, usize>,
    action_index: &HashMap<String, usize>,
    warnings: &mut Vec<String>,
) -> Result<Option<ObservationModelTable>, String> {
    let Some(model) = request.observation_model.as_ref() else {
        return Ok(None);
    };
    let Some(observations) = request.observations.as_ref() else {
        return Err("observations must be provided when observationModel is provided".to_string());
    };

    let observation_index = index_of(observations);
    let mut probabilities =
        vec![vec![vec![0.0; observations.len()]; request.states.len()]; request.actions.len()];

    for item in model {
        let a = *action_index
            .get(&item.action)
            .ok_or_else(|| format!("unknown observation action {}", item.action))?;
        let next = *state_index
            .get(&item.next_state)
            .ok_or_else(|| format!("unknown observation nextState {}", item.next_state))?;
        let observation = *observation_index
            .get(&item.observation)
            .ok_or_else(|| format!("unknown observation {}", item.observation))?;
        probabilities[a][next][observation] +=
            finite_probability(item.probability, "observation probability")?;
    }

    for (a, action) in request.actions.iter().enumerate() {
        for (next, next_state) in request.states.iter().enumerate() {
            let sum = normalize(&mut probabilities[a][next]);
            if sum > 0.0 && (sum - 1.0).abs() > 1e-9 {
                warnings.push(format!(
                    "normalized observation probabilities for action={action} nextState={next_state} from sum={sum}"
                ));
            }
        }
    }

    Ok(Some(ObservationModelTable {
        observation_index,
        probabilities,
    }))
}

fn best_belief_action(action_values: &[ActionBeliefValue]) -> Option<&ActionBeliefValue> {
    let mut best = None;
    for value in action_values {
        if best
            .map(|current: &ActionBeliefValue| value.value > current.value)
            .unwrap_or(true)
        {
            best = Some(value);
        }
    }
    best
}

fn predict_next_belief(
    belief: &[f64],
    transition: &[Vec<Vec<f64>>],
    action_index: usize,
) -> Vec<f64> {
    let mut predicted = vec![0.0; belief.len()];
    for s in 0..belief.len() {
        for next in 0..belief.len() {
            predicted[next] += belief[s] * transition[s][action_index][next];
        }
    }
    predicted
}

async fn publish_result(state: &AppState, response: &OptimizationResponse) {
    let Some(nats) = &state.nats else {
        return;
    };
    let payload = match serde_json::to_vec(&json!({
        "messageKind": "mdp.optimization.result",
        "source": "dd-mdp-optimizer",
        "result": response,
    })) {
        Ok(payload) => payload,
        Err(error) => {
            eprintln!("failed to encode mdp result: {error}");
            return;
        }
    };
    if let Err(error) = nats
        .publish(state.result_subject.clone(), payload.clone().into())
        .await
    {
        eprintln!("failed to publish mdp result: {error}");
        return;
    }
    let _ = nats
        .publish(
            state.event_subject.clone(),
            json!({
                "type": "mdp.optimization.result",
                "source": "dd-mdp-optimizer",
                "requestId": response.request_id,
                "converged": response.converged,
                "iterations": response.iterations,
                "residual": response.residual,
                "atMs": now_ms(),
            })
            .to_string()
            .into(),
        )
        .await;
}

async fn publish_telemetry_result(state: &AppState, response: &TelemetryLearningResponse) {
    let Some(nats) = &state.nats else {
        return;
    };
    let payload = match serde_json::to_vec(&json!({
        "messageKind": "mdp.telemetry.result",
        "source": "dd-mdp-optimizer",
        "result": response,
    })) {
        Ok(payload) => payload,
        Err(error) => {
            eprintln!("failed to encode mdp telemetry result: {error}");
            return;
        }
    };
    if let Err(error) = nats
        .publish(state.result_subject.clone(), payload.clone().into())
        .await
    {
        eprintln!("failed to publish mdp telemetry result: {error}");
        return;
    }
    let _ = nats
        .publish(
            state.event_subject.clone(),
            json!({
                "type": "mdp.telemetry.result",
                "source": "dd-mdp-optimizer",
                "requestId": response.request_id,
                "scope": response.scope,
                "risk": response.risk,
                "state": response.state,
                "recommendedAction": response.recommended_action,
                "atMs": now_ms(),
            })
            .to_string()
            .into(),
        )
        .await;
}

async fn healthz() -> impl IntoResponse {
    Json(json!({
        "ok": true,
        "service": "dd-mdp-optimizer",
        "mode": "mdp-rl-pomdp-nats",
        "atMs": now_ms(),
    }))
}

async fn metrics(State(state): State<AppState>) -> Response {
    let body = format!(
        "# HELP dd_mdp_optimizer_requests_total HTTP optimization requests.\n\
         # TYPE dd_mdp_optimizer_requests_total counter\n\
         dd_mdp_optimizer_requests_total {}\n\
         # HELP dd_mdp_optimizer_telemetry_requests_total HTTP telemetry learning requests.\n\
         # TYPE dd_mdp_optimizer_telemetry_requests_total counter\n\
         dd_mdp_optimizer_telemetry_requests_total {}\n\
         # HELP dd_mdp_optimizer_optimizations_total Optimization runs completed.\n\
         # TYPE dd_mdp_optimizer_optimizations_total counter\n\
         dd_mdp_optimizer_optimizations_total {}\n\
         # HELP dd_mdp_optimizer_errors_total Optimization or message errors.\n\
         # TYPE dd_mdp_optimizer_errors_total counter\n\
         dd_mdp_optimizer_errors_total {}\n\
         # HELP dd_mdp_optimizer_nats_messages_total NATS optimization requests received.\n\
         # TYPE dd_mdp_optimizer_nats_messages_total counter\n\
         dd_mdp_optimizer_nats_messages_total {}\n\
         # HELP dd_mdp_optimizer_nats_telemetry_messages_total NATS telemetry learning requests received.\n\
         # TYPE dd_mdp_optimizer_nats_telemetry_messages_total counter\n\
         dd_mdp_optimizer_nats_telemetry_messages_total {}\n",
        state.metrics.requests_total.load(Ordering::Relaxed),
        state
            .metrics
            .telemetry_requests_total
            .load(Ordering::Relaxed),
        state.metrics.optimizations_total.load(Ordering::Relaxed),
        state.metrics.errors_total.load(Ordering::Relaxed),
        state.metrics.nats_messages_total.load(Ordering::Relaxed),
        state
            .metrics
            .nats_telemetry_messages_total
            .load(Ordering::Relaxed),
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

async fn optimize_http(
    State(state): State<AppState>,
    Json(request): Json<OptimizationRequest>,
) -> Response {
    state.metrics.requests_total.fetch_add(1, Ordering::Relaxed);
    match optimize_in_background(request).await {
        Ok(response) => {
            state
                .metrics
                .optimizations_total
                .fetch_add(1, Ordering::Relaxed);
            publish_result(&state, &response).await;
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

async fn telemetry_learning_http(
    State(state): State<AppState>,
    Json(request): Json<TelemetryLearningRequest>,
) -> Response {
    state
        .metrics
        .telemetry_requests_total
        .fetch_add(1, Ordering::Relaxed);
    match optimize_telemetry_in_background(request).await {
        Ok(response) => {
            state
                .metrics
                .optimizations_total
                .fetch_add(1, Ordering::Relaxed);
            publish_telemetry_result(&state, &response).await;
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

async fn run_nats_loop(state: AppState, subject: String, queue_group: String) {
    let Some(nats) = state.nats.clone() else {
        println!("mdp optimizer nats loop disabled: NATS_URL is not configured");
        return;
    };
    println!(
        "mdp optimizer nats loop starting: subject={subject} queue_group={queue_group} resultSubject={}",
        state.result_subject
    );
    let mut subscription = match nats.queue_subscribe(subject, queue_group).await {
        Ok(subscription) => subscription,
        Err(error) => {
            eprintln!("mdp optimizer nats subscribe failed: {error}");
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
                "mdp optimizer rejected oversize nats request: bytes={} max={MAX_NATS_PAYLOAD_BYTES}",
                payload.len()
            );
            continue;
        }
        let task_state = state.clone();
        tokio::spawn(async move {
            let parsed = serde_json::from_slice::<OptimizationRequest>(&payload);
            match parsed {
                Ok(request) => match optimize_in_background(request).await {
                    Ok(response) => {
                        task_state
                            .metrics
                            .optimizations_total
                            .fetch_add(1, Ordering::Relaxed);
                        publish_result(&task_state, &response).await;
                    }
                    Err(error) => {
                        task_state
                            .metrics
                            .errors_total
                            .fetch_add(1, Ordering::Relaxed);
                        eprintln!("mdp optimizer failed nats optimization: {error}");
                    }
                },
                Err(error) => {
                    task_state
                        .metrics
                        .errors_total
                        .fetch_add(1, Ordering::Relaxed);
                    eprintln!("mdp optimizer invalid nats request: {error}");
                }
            }
        });
    }
}

async fn run_telemetry_nats_loop(state: AppState, subject: String, queue_group: String) {
    let Some(nats) = state.nats.clone() else {
        println!("mdp optimizer telemetry nats loop disabled: NATS_URL is not configured");
        return;
    };
    println!(
        "mdp optimizer telemetry nats loop starting: subject={subject} queue_group={queue_group} resultSubject={}",
        state.result_subject
    );
    let mut subscription = match nats.queue_subscribe(subject, queue_group).await {
        Ok(subscription) => subscription,
        Err(error) => {
            eprintln!("mdp optimizer telemetry nats subscribe failed: {error}");
            return;
        }
    };
    while let Some(message) = subscription.next().await {
        state
            .metrics
            .nats_telemetry_messages_total
            .fetch_add(1, Ordering::Relaxed);
        let payload = message.payload.to_vec();
        if payload.len() > MAX_NATS_PAYLOAD_BYTES {
            state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
            eprintln!(
                "mdp optimizer rejected oversize telemetry nats request: bytes={} max={MAX_NATS_PAYLOAD_BYTES}",
                payload.len()
            );
            continue;
        }
        let task_state = state.clone();
        tokio::spawn(async move {
            let parsed = serde_json::from_slice::<TelemetryLearningRequest>(&payload);
            match parsed {
                Ok(request) => match optimize_telemetry_in_background(request).await {
                    Ok(response) => {
                        task_state
                            .metrics
                            .optimizations_total
                            .fetch_add(1, Ordering::Relaxed);
                        publish_telemetry_result(&task_state, &response).await;
                    }
                    Err(error) => {
                        task_state
                            .metrics
                            .errors_total
                            .fetch_add(1, Ordering::Relaxed);
                        eprintln!("mdp optimizer failed telemetry optimization: {error}");
                    }
                },
                Err(error) => {
                    task_state
                        .metrics
                        .errors_total
                        .fetch_add(1, Ordering::Relaxed);
                    eprintln!("mdp optimizer invalid telemetry request: {error}");
                }
            }
        });
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

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    let host = env_value("HOST", "0.0.0.0");
    let port = env_value("PORT", "8096").parse::<u16>()?;
    let nats = match env::var("NATS_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
    {
        Some(url) => Some(async_nats::connect(url).await?),
        None => None,
    };
    let state = AppState {
        nats,
        result_subject: env_value("MDP_RESULT_SUBJECT", "dd.remote.mdp.results"),
        event_subject: env_value("MDP_EVENT_SUBJECT", "dd.remote.events"),
        metrics: Arc::new(Metrics::default()),
    };
    let nats_subject = env_value("MDP_OPTIMIZE_SUBJECT", "dd.remote.mdp.optimize");
    let queue_group = env_value("MDP_QUEUE_GROUP", "dd-mdp-optimizer");
    tokio::spawn(run_nats_loop(state.clone(), nats_subject, queue_group));
    let telemetry_subject = env_value("MDP_TELEMETRY_SUBJECT", "dd.remote.telemetry.mdp");
    let telemetry_queue_group = env_value("MDP_TELEMETRY_QUEUE_GROUP", "dd-mdp-telemetry-learner");
    tokio::spawn(run_telemetry_nats_loop(
        state.clone(),
        telemetry_subject,
        telemetry_queue_group,
    ));

    let app = Router::new()
        .route("/", get(healthz))
        .route("/healthz", get(healthz))
        .route("/docs/api", get(api_docs_html))
        .route("/api/docs", get(api_docs_html))
        .route("/api/docs.json", get(api_docs_json))
        .route("/metrics", get(metrics))
        .route("/optimize", post(optimize_http))
        .route("/telemetry/learn", post(telemetry_learning_http))
        .layer(DefaultBodyLimit::max(MAX_HTTP_BODY_BYTES))
        .with_state(state);
    let addr: SocketAddr = format!("{host}:{port}").parse()?;
    println!("dd-mdp-optimizer listening on http://{addr}");
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

    fn labels(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    fn transition(
        state: &str,
        action: &str,
        next_state: &str,
        probability: f64,
    ) -> TransitionInput {
        TransitionInput {
            state: state.to_string(),
            action: action.to_string(),
            next_state: next_state.to_string(),
            probability,
        }
    }

    fn reward(state: &str, action: &str, value: f64) -> RewardInput {
        RewardInput {
            state: state.to_string(),
            action: action.to_string(),
            value,
        }
    }

    fn observation(
        action: &str,
        next_state: &str,
        observation: &str,
        probability: f64,
    ) -> ObservationInput {
        ObservationInput {
            action: action.to_string(),
            next_state: next_state.to_string(),
            observation: observation.to_string(),
            probability,
        }
    }

    fn belief(state: &str, probability: f64) -> BeliefInput {
        BeliefInput {
            state: state.to_string(),
            probability,
        }
    }

    fn telemetry_impact(
        action: &str,
        delta: f64,
        confidence: Option<f64>,
    ) -> TelemetryActionImpactInput {
        TelemetryActionImpactInput {
            action: action.to_string(),
            delta,
            confidence,
        }
    }

    fn telemetry_signal(
        name: &str,
        layer: &str,
        service: &str,
        value: f64,
        warning: f64,
        critical: f64,
    ) -> TelemetrySignalInput {
        TelemetrySignalInput {
            name: name.to_string(),
            service: Some(service.to_string()),
            layer: Some(layer.to_string()),
            value,
            baseline: None,
            target: None,
            warning: Some(warning),
            critical: Some(critical),
            weight: Some(1.0),
            higher_is_better: Some(false),
            action_impacts: None,
        }
    }

    fn assert_close(actual: f64, expected: f64) {
        assert!(
            (actual - expected).abs() < 1e-6,
            "expected {actual} to be close to {expected}"
        );
    }

    fn state_value(response: &OptimizationResponse, state: &str) -> f64 {
        response
            .values
            .iter()
            .find(|entry| entry.state == state)
            .unwrap_or_else(|| panic!("missing state value for {state}"))
            .value
    }

    fn policy_action<'a>(response: &'a OptimizationResponse, state: &str) -> &'a str {
        response
            .policy
            .iter()
            .find(|entry| entry.state == state)
            .unwrap_or_else(|| panic!("missing policy for {state}"))
            .action
            .as_str()
    }

    fn belief_value(values: &[StateValue], state: &str) -> f64 {
        values
            .iter()
            .find(|entry| entry.state == state)
            .unwrap_or_else(|| panic!("missing belief value for {state}"))
            .value
    }

    fn value_iteration_request() -> OptimizationRequest {
        OptimizationRequest {
            request_id: Some("unit-mdp".to_string()),
            kind: None,
            states: labels(&["start", "done"]),
            actions: labels(&["wait", "go"]),
            transitions: vec![
                transition("start", "wait", "start", 1.0),
                transition("start", "go", "done", 1.0),
                transition("done", "wait", "done", 1.0),
                transition("done", "go", "done", 1.0),
            ],
            rewards: vec![
                reward("start", "wait", 0.0),
                reward("start", "go", 1.0),
                reward("done", "wait", 2.0),
                reward("done", "go", 0.0),
            ],
            observations: None,
            observation_model: None,
            belief: None,
            belief_action: None,
            observed: None,
            gamma: Some(0.5),
            tolerance: Some(1e-10),
            max_iterations: Some(10_000),
        }
    }

    fn pomdp_request() -> OptimizationRequest {
        OptimizationRequest {
            request_id: Some("unit-pomdp".to_string()),
            kind: Some("pomdp.belief-update".to_string()),
            states: labels(&["rain", "clear"]),
            actions: labels(&["umbrella", "skip"]),
            transitions: vec![
                transition("rain", "umbrella", "rain", 1.0),
                transition("clear", "umbrella", "clear", 1.0),
                transition("rain", "skip", "rain", 1.0),
                transition("clear", "skip", "clear", 1.0),
            ],
            rewards: vec![
                reward("rain", "umbrella", 2.0),
                reward("clear", "umbrella", 0.0),
                reward("rain", "skip", -2.0),
                reward("clear", "skip", 1.0),
            ],
            observations: Some(labels(&["wet", "dry"])),
            observation_model: Some(vec![
                observation("umbrella", "rain", "wet", 9.0),
                observation("umbrella", "rain", "dry", 1.0),
                observation("umbrella", "clear", "wet", 2.0),
                observation("umbrella", "clear", "dry", 8.0),
            ]),
            belief: Some(vec![belief("rain", 0.25), belief("clear", 0.75)]),
            belief_action: Some("umbrella".to_string()),
            observed: Some("wet".to_string()),
            gamma: Some(0.0),
            tolerance: Some(1e-9),
            max_iterations: Some(10),
        }
    }

    fn telemetry_learning_request() -> TelemetryLearningRequest {
        let mut cpu = telemetry_signal(
            "node_cpu_utilization",
            "infra",
            "dd-dev-server-api",
            0.92,
            0.7,
            0.9,
        );
        cpu.action_impacts = Some(vec![telemetry_impact("scale-up", 0.55, Some(0.9))]);

        TelemetryLearningRequest {
            request_id: Some("unit-telemetry".to_string()),
            scope: Some("infra".to_string()),
            window_ms: Some(300_000),
            signals: vec![
                cpu,
                telemetry_signal("http_5xx_rate", "app", "dd-dev-server-api", 0.08, 0.02, 0.1),
            ],
            actions: Some(labels(&["hold", "observe", "scale-up"])),
            gamma: Some(0.8),
            tolerance: Some(1e-8),
            max_iterations: Some(500),
        }
    }

    #[test]
    fn value_iteration_extracts_greedy_policy_and_values() {
        let response = optimize(value_iteration_request()).expect("optimization should succeed");

        assert!(response.converged);
        assert_eq!(response.request_id, "unit-mdp");
        assert_eq!(policy_action(&response, "start"), "go");
        assert_eq!(policy_action(&response, "done"), "wait");
        assert_close(state_value(&response, "start"), 3.0);
        assert_close(state_value(&response, "done"), 4.0);
        assert!(response.warnings.is_empty());
    }

    #[test]
    fn rejects_duplicate_state_labels_before_indexing() {
        let mut request = value_iteration_request();
        request.states = labels(&["start", "start"]);

        let error = optimize(request).expect_err("duplicate states should be rejected");

        assert!(error.contains("states must be unique"));
    }

    #[test]
    fn normalizes_transitions_and_fills_missing_pairs_with_self_loops() {
        let request = OptimizationRequest {
            request_id: Some("unit-warnings".to_string()),
            kind: None,
            states: labels(&["s0", "s1"]),
            actions: labels(&["a", "b"]),
            transitions: vec![transition("s0", "a", "s1", 2.0)],
            rewards: vec![],
            observations: None,
            observation_model: None,
            belief: None,
            belief_action: None,
            observed: None,
            gamma: Some(0.5),
            tolerance: Some(1e-6),
            max_iterations: Some(3),
        };

        let response = optimize(request).expect("normalizable request should succeed");

        assert!(response.warnings.iter().any(|warning| {
            warning.contains("normalized transition probabilities for state=s0 action=a")
        }));
        assert!(response
            .warnings
            .iter()
            .any(|warning| warning.contains("missing transition for state=s0 action=b")));
        assert!(response
            .warnings
            .iter()
            .any(|warning| warning.contains("missing transition for state=s1 action=a")));
    }

    #[test]
    fn pomdp_belief_summary_uses_selected_action_and_bayes_posterior() {
        let response = optimize(pomdp_request()).expect("pomdp request should succeed");
        let summary = response.belief.expect("belief summary should be present");
        let posterior = summary.posterior.expect("posterior should be present");

        assert_eq!(summary.selected_action.as_deref(), Some("umbrella"));
        assert_eq!(summary.observed.as_deref(), Some("wet"));
        assert_close(belief_value(&summary.input, "rain"), 0.25);
        assert_close(belief_value(&summary.input, "clear"), 0.75);
        assert_close(belief_value(&posterior, "rain"), 0.6);
        assert_close(belief_value(&posterior, "clear"), 0.4);
        assert!(response.warnings.iter().any(|warning| {
            warning
                .contains("normalized observation probabilities for action=umbrella nextState=rain")
        }));
        assert!(response.warnings.iter().any(|warning| {
            warning.contains(
                "normalized observation probabilities for action=umbrella nextState=clear",
            )
        }));
    }

    #[test]
    fn observation_model_requires_declared_observations() {
        let mut request = pomdp_request();
        request.observations = None;

        let error = optimize(request).expect_err("observation model should require observations");

        assert!(error.contains("observations must be provided when observationModel is provided"));
    }

    #[test]
    fn telemetry_learning_turns_infra_signals_into_policy_insights() {
        let response =
            optimize_telemetry(telemetry_learning_request()).expect("telemetry learning succeeds");

        assert_eq!(response.request_id, "unit-telemetry");
        assert_eq!(response.scope, "infra");
        assert_eq!(response.state, "critical");
        assert_eq!(response.recommended_action, "scale-up");
        assert_eq!(response.optimization.kind, "telemetry.mdp.value-iteration");
        assert!(response.risk > 0.8);
        assert_eq!(response.insights[0].signal, "node_cpu_utilization");
        assert_eq!(response.insights[0].layer, "infra");
    }

    #[test]
    fn telemetry_learning_scores_higher_is_better_signals() {
        let mut availability = telemetry_signal(
            "availability",
            "infra",
            "dd-remote-gateway",
            0.91,
            0.98,
            0.9,
        );
        availability.higher_is_better = Some(true);
        let request = TelemetryLearningRequest {
            request_id: Some("unit-availability".to_string()),
            scope: Some("infra".to_string()),
            window_ms: None,
            signals: vec![availability],
            actions: Some(labels(&["hold", "observe", "page-human"])),
            gamma: Some(0.8),
            tolerance: Some(1e-8),
            max_iterations: Some(500),
        };

        let response = optimize_telemetry(request).expect("telemetry learning succeeds");

        assert_eq!(response.state, "critical");
        assert!(response.risk > 0.8);
        assert_eq!(response.insights[0].state, "critical");
    }

    #[test]
    fn telemetry_learning_rejects_empty_signal_sets() {
        let request = TelemetryLearningRequest {
            request_id: Some("unit-empty-telemetry".to_string()),
            scope: None,
            window_ms: None,
            signals: vec![],
            actions: None,
            gamma: None,
            tolerance: None,
            max_iterations: None,
        };

        let error = optimize_telemetry(request).expect_err("empty telemetry should fail");

        assert!(error.contains("telemetry signals must not be empty"));
    }

    #[test]
    fn telemetry_learning_caps_signal_count() {
        let signals = (0..=MAX_TELEMETRY_SIGNALS)
            .map(|index| {
                telemetry_signal(
                    &format!("signal_{index}"),
                    "infra",
                    "dd-dev-server-api",
                    0.1,
                    0.5,
                    0.9,
                )
            })
            .collect::<Vec<_>>();
        let request = TelemetryLearningRequest {
            request_id: Some("unit-too-many-signals".to_string()),
            scope: Some("infra".to_string()),
            window_ms: None,
            signals,
            actions: None,
            gamma: None,
            tolerance: None,
            max_iterations: None,
        };

        let error = optimize_telemetry(request).expect_err("too many signals should fail");

        assert!(error.contains("telemetry signals must contain at most"));
    }

    #[test]
    fn telemetry_learning_rejects_unstable_action_labels() {
        let request = TelemetryLearningRequest {
            request_id: Some("unit-bad-action".to_string()),
            scope: Some("infra".to_string()),
            window_ms: None,
            signals: vec![telemetry_signal(
                "node_cpu_utilization",
                "infra",
                "dd-dev-server-api",
                0.92,
                0.7,
                0.9,
            )],
            actions: Some(labels(&["hold", "scale up"])),
            gamma: None,
            tolerance: None,
            max_iterations: None,
        };

        let error = optimize_telemetry(request).expect_err("bad action label should fail");

        assert!(error.contains("telemetry action may only contain"));
    }

    #[test]
    fn telemetry_learning_honors_negative_custom_action_impacts() {
        let mut cpu = telemetry_signal(
            "node_cpu_utilization",
            "infra",
            "dd-dev-server-api",
            0.92,
            0.7,
            0.9,
        );
        cpu.action_impacts = Some(vec![telemetry_impact("scale-up", -0.8, Some(1.0))]);
        let request = TelemetryLearningRequest {
            request_id: Some("unit-negative-impact".to_string()),
            scope: Some("infra".to_string()),
            window_ms: None,
            signals: vec![cpu],
            actions: Some(labels(&["hold", "observe", "scale-up", "page-human"])),
            gamma: Some(0.8),
            tolerance: Some(1e-8),
            max_iterations: Some(500),
        };

        let response = optimize_telemetry(request).expect("telemetry learning succeeds");

        assert_ne!(response.recommended_action, "scale-up");
        assert!(response
            .warnings
            .iter()
            .any(|warning| warning.contains("action=scale-up lowers expected recovery")));
    }

    #[test]
    fn telemetry_learning_normalizes_scope_layer_and_action_case() {
        let mut cpu = telemetry_signal(
            "node_cpu_utilization",
            "Infrastructure",
            "dd-dev-server-api",
            0.92,
            0.7,
            0.9,
        );
        cpu.action_impacts = Some(vec![telemetry_impact("SCALE-UP", 0.55, Some(0.9))]);
        let request = TelemetryLearningRequest {
            request_id: Some(" unit-normalized ".to_string()),
            scope: Some(" Infrastructure ".to_string()),
            window_ms: Some(60_000),
            signals: vec![cpu],
            actions: Some(labels(&["Hold", "Observe", "SCALE-UP"])),
            gamma: Some(0.8),
            tolerance: Some(1e-8),
            max_iterations: Some(500),
        };

        let response = optimize_telemetry(request).expect("telemetry learning succeeds");

        assert_eq!(response.request_id, "unit-normalized");
        assert_eq!(response.scope, "infra");
        assert_eq!(response.insights[0].layer, "infra");
        assert_eq!(response.recommended_action, "scale-up");
    }
}
