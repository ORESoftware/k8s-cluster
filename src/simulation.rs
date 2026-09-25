use des_engine::des::fel::engine::Engine;
use serde_json::{json, Value};

use crate::models::{SimulationRunRequest, SimulationRunResponse};

const MAX_TRACE_EVENTS: usize = 1000;

#[derive(Debug, Clone)]
struct SimConfig {
    case_id: Option<String>,
    seed: u64,
    horizon_days: i32,
    actor_count: i32,
    target_signatures: u32,
    sponsor_response_rate: f64,
    admission_approval_rate: f64,
    judge_conviction_rate: f64,
    panel_size: u32,
    conviction_threshold_count: u32,
    input: Value,
}

#[derive(Debug)]
struct World {
    config: SimConfig,
    rng: Lcg,
    signatures: u32,
    admission_for: u32,
    admission_against: u32,
    guilty_votes: u32,
    not_guilty_votes: u32,
    admitted: bool,
    convicted: bool,
    trace: Vec<Value>,
}

#[derive(Debug)]
struct Lcg {
    state: u64,
}

impl Lcg {
    fn new(seed: u64) -> Self {
        Self { state: seed.max(1) }
    }

    fn next_unit(&mut self) -> f64 {
        self.state = self
            .state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let upper = self.state >> 11;
        (upper as f64) / ((1_u64 << 53) as f64)
    }
}

pub fn run_simulation(request: SimulationRunRequest) -> SimulationRunResponse {
    let config = config_from_request(request);
    let mut engine = Engine::new(World {
        rng: Lcg::new(config.seed),
        config,
        signatures: 0,
        admission_for: 0,
        admission_against: 0,
        guilty_votes: 0,
        not_guilty_votes: 0,
        admitted: false,
        convicted: false,
        trace: Vec::new(),
    });

    engine.schedule_at(0.0, start_signature_collection);
    engine.run_until(engine.world.config.horizon_days as f64);

    let metrics = json!({
        "signaturesCollected": engine.world.signatures,
        "targetSignatures": engine.world.config.target_signatures,
        "admissionVotesFor": engine.world.admission_for,
        "admissionVotesAgainst": engine.world.admission_against,
        "admitted": engine.world.admitted,
        "guiltyVotes": engine.world.guilty_votes,
        "notGuiltyVotes": engine.world.not_guilty_votes,
        "panelSize": engine.world.config.panel_size,
        "convictionThresholdCount": engine.world.config.conviction_threshold_count,
        "convicted": engine.world.convicted,
        "traceTruncated": engine.world.trace.len() >= MAX_TRACE_EVENTS,
        "input": engine.world.config.input,
    });

    SimulationRunResponse {
        ok: true,
        persisted: false,
        run_id: None,
        case_id: engine.world.config.case_id.clone(),
        seed: engine.world.config.seed,
        horizon_days: engine.world.config.horizon_days,
        actor_count: engine.world.config.actor_count,
        event_count: engine.events_processed(),
        metrics,
        trace: Value::Array(engine.world.trace),
    }
}

fn config_from_request(request: SimulationRunRequest) -> SimConfig {
    let actor_count = request.actor_count.unwrap_or(100).clamp(1, 100_000);
    let target_signatures = request
        .target_signatures
        .unwrap_or(actor_count.min(1000) as u32)
        .clamp(1, 100_000);
    let panel_size = request.panel_size.unwrap_or(15).clamp(1, 101);
    let default_threshold = ((panel_size as f64) * 0.80).ceil() as u32;

    SimConfig {
        case_id: request.case_id,
        seed: request.seed.unwrap_or(42),
        horizon_days: request.horizon_days.unwrap_or(180).clamp(1, 3650),
        actor_count,
        target_signatures,
        sponsor_response_rate: clamp_probability(request.sponsor_response_rate.unwrap_or(0.72)),
        admission_approval_rate: clamp_probability(request.admission_approval_rate.unwrap_or(0.67)),
        judge_conviction_rate: clamp_probability(request.judge_conviction_rate.unwrap_or(0.80)),
        panel_size,
        conviction_threshold_count: request
            .conviction_threshold_count
            .unwrap_or(default_threshold)
            .clamp(1, panel_size),
        input: request.input.unwrap_or_else(|| json!({})),
    }
}

fn clamp_probability(value: f64) -> f64 {
    if value.is_finite() {
        value.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

fn push_trace(engine: &mut Engine<World>, event: &str, payload: Value) {
    if engine.world.trace.len() >= MAX_TRACE_EVENTS {
        return;
    }
    engine.world.trace.push(json!({
        "timeDays": round3(engine.now()),
        "event": event,
        "payload": payload,
    }));
}

fn round3(value: f64) -> f64 {
    (value * 1000.0).round() / 1000.0
}

fn start_signature_collection(engine: &mut Engine<World>) {
    push_trace(
        engine,
        "signature_collection.opened",
        json!({
            "targetSignatures": engine.world.config.target_signatures,
            "actorCount": engine.world.config.actor_count,
        }),
    );
    engine.schedule_after(0.05, collect_signature);
}

fn collect_signature(engine: &mut Engine<World>) {
    let response_draw = engine.world.rng.next_unit();
    if response_draw <= engine.world.config.sponsor_response_rate {
        engine.world.signatures += 1;
        if engine.world.signatures <= 25 || engine.world.signatures.is_multiple_of(100) {
            push_trace(
                engine,
                "signature.received",
                json!({ "signatures": engine.world.signatures }),
            );
        }
    }

    if engine.world.signatures >= engine.world.config.target_signatures {
        push_trace(
            engine,
            "signature_collection.threshold_reached",
            json!({ "signatures": engine.world.signatures }),
        );
        engine.schedule_after(1.0, screening_review);
        return;
    }

    if engine.now() < engine.world.config.horizon_days as f64 {
        let delay_days = 0.05 + engine.world.rng.next_unit() * 0.95;
        engine.schedule_after(delay_days, collect_signature);
    }
}

fn screening_review(engine: &mut Engine<World>) {
    push_trace(
        engine,
        "screening.completed",
        json!({ "recommendation": "advance_to_admission_review" }),
    );
    engine.schedule_after(2.0, admission_review);
}

fn admission_review(engine: &mut Engine<World>) {
    for _ in 0..3 {
        if engine.world.rng.next_unit() <= engine.world.config.admission_approval_rate {
            engine.world.admission_for += 1;
        } else {
            engine.world.admission_against += 1;
        }
    }

    engine.world.admitted = engine.world.admission_for >= 2;
    push_trace(
        engine,
        "admission_review.tallied",
        json!({
            "for": engine.world.admission_for,
            "against": engine.world.admission_against,
            "admitted": engine.world.admitted,
        }),
    );

    if engine.world.admitted {
        engine.schedule_after(7.0, trial_verdict);
    }
}

fn trial_verdict(engine: &mut Engine<World>) {
    for _ in 0..engine.world.config.panel_size {
        if engine.world.rng.next_unit() <= engine.world.config.judge_conviction_rate {
            engine.world.guilty_votes += 1;
        } else {
            engine.world.not_guilty_votes += 1;
        }
    }

    engine.world.convicted =
        engine.world.guilty_votes >= engine.world.config.conviction_threshold_count;
    push_trace(
        engine,
        "trial_panel.verdict_tallied",
        json!({
            "guiltyVotes": engine.world.guilty_votes,
            "notGuiltyVotes": engine.world.not_guilty_votes,
            "threshold": engine.world.config.conviction_threshold_count,
            "convicted": engine.world.convicted,
        }),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simulation_is_deterministic_for_seed() {
        let request = SimulationRunRequest {
            case_id: Some("case-1".to_string()),
            seed: Some(123),
            horizon_days: Some(90),
            actor_count: Some(150),
            target_signatures: Some(20),
            sponsor_response_rate: Some(0.9),
            admission_approval_rate: Some(0.8),
            judge_conviction_rate: Some(0.7),
            panel_size: Some(15),
            conviction_threshold_count: Some(12),
            persist: Some(false),
            input: None,
        };

        let left = run_simulation(request.clone());
        let right = run_simulation(request);

        assert_eq!(left.metrics, right.metrics);
        assert_eq!(left.trace, right.trace);
    }

    #[test]
    fn strong_case_reaches_conviction_threshold() {
        let response = run_simulation(SimulationRunRequest {
            case_id: None,
            seed: Some(7),
            horizon_days: Some(180),
            actor_count: Some(500),
            target_signatures: Some(10),
            sponsor_response_rate: Some(1.0),
            admission_approval_rate: Some(1.0),
            judge_conviction_rate: Some(1.0),
            panel_size: Some(15),
            conviction_threshold_count: Some(12),
            persist: Some(false),
            input: None,
        });

        assert_eq!(response.metrics["convicted"], true);
        assert_eq!(response.metrics["guiltyVotes"], 15);
    }
}

#[cfg(test)]
mod config_tests {
    use super::*;

    fn empty_request() -> SimulationRunRequest {
        SimulationRunRequest {
            case_id: None,
            seed: None,
            horizon_days: None,
            actor_count: None,
            target_signatures: None,
            sponsor_response_rate: None,
            admission_approval_rate: None,
            judge_conviction_rate: None,
            panel_size: None,
            conviction_threshold_count: None,
            persist: None,
            input: None,
        }
    }

    #[test]
    fn defaults_applied_when_request_is_empty() {
        let config = config_from_request(empty_request());
        assert_eq!(config.seed, 42);
        assert_eq!(config.horizon_days, 180);
        assert_eq!(config.actor_count, 100);
        // Default target = min(actor_count, 1000).
        assert_eq!(config.target_signatures, 100);
        assert_eq!(config.panel_size, 15);
        // Default threshold = ceil(0.8 * panel_size) = 12.
        assert_eq!(config.conviction_threshold_count, 12);
        assert_eq!(config.input, json!({}));
    }

    #[test]
    fn out_of_range_values_are_clamped() {
        let config = config_from_request(SimulationRunRequest {
            actor_count: Some(-5),
            horizon_days: Some(1_000_000),
            panel_size: Some(500),
            target_signatures: Some(0),
            conviction_threshold_count: Some(9_999),
            ..empty_request()
        });
        assert_eq!(config.actor_count, 1);
        assert_eq!(config.horizon_days, 3650);
        assert_eq!(config.panel_size, 101);
        assert_eq!(config.target_signatures, 1);
        // Threshold can never exceed the panel size.
        assert_eq!(config.conviction_threshold_count, 101);
    }

    #[test]
    fn probabilities_are_clamped_and_nan_is_zeroed() {
        assert_eq!(clamp_probability(0.5), 0.5);
        assert_eq!(clamp_probability(-0.2), 0.0);
        assert_eq!(clamp_probability(1.7), 1.0);
        assert_eq!(clamp_probability(f64::NAN), 0.0);
        assert_eq!(clamp_probability(f64::INFINITY), 0.0);
        assert_eq!(clamp_probability(f64::NEG_INFINITY), 0.0);
    }

    #[test]
    fn lcg_stream_is_deterministic_and_in_unit_range() {
        let mut a = Lcg::new(99);
        let mut b = Lcg::new(99);
        for _ in 0..1000 {
            let next = a.next_unit();
            assert_eq!(next, b.next_unit());
            assert!((0.0..1.0).contains(&next), "out of range: {next}");
        }
        // Zero seed is coerced to a nonzero state rather than a frozen stream.
        let mut zero_seeded = Lcg::new(0);
        let mut one_seeded = Lcg::new(1);
        assert_eq!(zero_seeded.next_unit(), one_seeded.next_unit());
    }

    #[test]
    fn zero_response_rate_never_collects_signatures() {
        let response = run_simulation(SimulationRunRequest {
            seed: Some(11),
            horizon_days: Some(30),
            sponsor_response_rate: Some(0.0),
            ..empty_request()
        });
        assert_eq!(response.metrics["signaturesCollected"], 0);
        assert_eq!(response.metrics["admitted"], false);
        assert_eq!(response.metrics["convicted"], false);
    }
}
