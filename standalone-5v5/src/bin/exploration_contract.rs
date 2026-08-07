//! Replay-safe exploration profile and behavior-likelihood contract.
//!
//! This binary is hermetic and intentionally independent from the production
//! selector. It proves the profile/trace semantics that the existing selector
//! can adopt without duplicating its tactical scoring logic.

use std::cmp::Ordering;

const SCHEMA_VERSION: u16 = 1;
const MIN_PROBABILITY: f64 = 1.0e-300;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Training,
    Adaptation,
    Validation,
    Production,
}

impl Mode {
    fn stochastic(self) -> bool {
        matches!(self, Self::Training | Self::Adaptation)
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Training => "training",
            Self::Adaptation => "adaptation",
            Self::Validation => "validation",
            Self::Production => "production",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Strategy {
    Greedy,
    EpsilonGreedy { epsilon: f64 },
    RankWeighted { top_three: [f64; 3] },
    Boltzmann { temperature: f64 },
    UncertaintyDirected { beta: f64, temperature: f64 },
}

impl Strategy {
    fn as_str(self) -> &'static str {
        match self {
            Self::Greedy => "greedy",
            Self::EpsilonGreedy { .. } => "epsilon_greedy",
            Self::RankWeighted { .. } => "rank_weighted",
            Self::Boltzmann { .. } => "boltzmann",
            Self::UncertaintyDirected { .. } => "uncertainty_directed",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Profile {
    schema_version: u16,
    name: &'static str,
    mode: Mode,
    strategy: Strategy,
    seed: u64,
}

impl Profile {
    fn rank_70_20_10(seed: u64) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            name: "rank_70_20_10",
            mode: Mode::Training,
            strategy: Strategy::RankWeighted {
                top_three: [0.70, 0.20, 0.10],
            },
            seed,
        }
    }

    fn validate(self) -> Result<(), String> {
        if self.schema_version != SCHEMA_VERSION {
            return Err(format!(
                "unsupported schema version {}; expected {}",
                self.schema_version, SCHEMA_VERSION
            ));
        }
        if self.name.trim().is_empty() {
            return Err("profile name must not be empty".into());
        }
        match self.strategy {
            Strategy::Greedy => Ok(()),
            Strategy::EpsilonGreedy { epsilon } => unit_interval("epsilon", epsilon),
            Strategy::RankWeighted { top_three } => {
                let mut total = 0.0;
                for weight in top_three {
                    if !weight.is_finite() || weight < 0.0 {
                        return Err(format!("invalid rank weight {weight}"));
                    }
                    total += weight;
                }
                if total <= 0.0 {
                    return Err("rank profile must contain positive probability mass".into());
                }
                Ok(())
            }
            Strategy::Boltzmann { temperature } => positive("temperature", temperature),
            Strategy::UncertaintyDirected { beta, temperature } => {
                non_negative("beta", beta)?;
                positive("temperature", temperature)
            }
        }
    }

    fn effective_strategy(self) -> Strategy {
        if self.mode.stochastic() {
            self.strategy
        } else {
            Strategy::Greedy
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Candidate {
    action_id: u32,
    score: f64,
    uncertainty: f64,
    feasible: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LikelihoodStatus {
    ExactExecutedAction,
    UnknownAfterDownstreamOverride,
}

impl LikelihoodStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::ExactExecutedAction => "exact_executed_action",
            Self::UnknownAfterDownstreamOverride => "unknown_after_downstream_override",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
struct Trace {
    schema_version: u16,
    profile_name: &'static str,
    mode: Mode,
    strategy_name: &'static str,
    decision_nonce: u64,
    selected_action_id: u32,
    selected_rank: usize,
    selected_probability: f64,
    selected_log_probability: f64,
    executed_action_id: u32,
    executed_rank: usize,
    executed_probability: Option<f64>,
    likelihood_status: LikelihoodStatus,
}

impl Trace {
    fn reconcile_executed(
        &mut self,
        executed_action_id: u32,
        candidates: &[Candidate],
    ) -> Result<(), String> {
        let ranked = ranked_feasible(candidates)?;
        let executed_rank = ranked
            .iter()
            .position(|candidate| candidate.action_id == executed_action_id)
            .ok_or_else(|| format!("executed action {executed_action_id} is not feasible"))?;
        self.executed_action_id = executed_action_id;
        self.executed_rank = executed_rank;
        if executed_action_id == self.selected_action_id {
            self.executed_probability = Some(self.selected_probability);
            self.likelihood_status = LikelihoodStatus::ExactExecutedAction;
        } else {
            self.executed_probability = None;
            self.likelihood_status = LikelihoodStatus::UnknownAfterDownstreamOverride;
        }
        Ok(())
    }

    fn to_json(&self) -> String {
        let executed_probability = self
            .executed_probability
            .map(|value| value.to_string())
            .unwrap_or_else(|| "null".into());
        format!(
            concat!(
                "{{\"schemaVersion\":{},\"profile\":\"{}\",",
                "\"mode\":\"{}\",\"strategy\":\"{}\",",
                "\"decisionNonce\":{},\"selectedActionId\":{},",
                "\"selectedRank\":{},\"selectedProbability\":{},",
                "\"selectedLogProbability\":{},\"executedActionId\":{},",
                "\"executedRank\":{},\"executedProbability\":{},",
                "\"likelihoodStatus\":\"{}\"}}"
            ),
            self.schema_version,
            self.profile_name,
            self.mode.as_str(),
            self.strategy_name,
            self.decision_nonce,
            self.selected_action_id,
            self.selected_rank,
            self.selected_probability,
            self.selected_log_probability,
            self.executed_action_id,
            self.executed_rank,
            executed_probability,
            self.likelihood_status.as_str(),
        )
    }
}

fn select(profile: Profile, decision_nonce: u64, candidates: &[Candidate]) -> Result<Trace, String> {
    profile.validate()?;
    let ranked = ranked_feasible(candidates)?;
    let strategy = profile.effective_strategy();
    let probabilities = probabilities(strategy, &ranked);
    let draw = deterministic_unit_draw(profile.seed, decision_nonce);
    let selected_rank = sample_index(&probabilities, draw);
    let selected = ranked[selected_rank];
    let probability = probabilities[selected_rank].max(MIN_PROBABILITY);
    Ok(Trace {
        schema_version: SCHEMA_VERSION,
        profile_name: profile.name,
        mode: profile.mode,
        strategy_name: strategy.as_str(),
        decision_nonce,
        selected_action_id: selected.action_id,
        selected_rank,
        selected_probability: probability,
        selected_log_probability: probability.ln(),
        executed_action_id: selected.action_id,
        executed_rank: selected_rank,
        executed_probability: Some(probability),
        likelihood_status: LikelihoodStatus::ExactExecutedAction,
    })
}

fn ranked_feasible(candidates: &[Candidate]) -> Result<Vec<Candidate>, String> {
    let mut ranked = Vec::new();
    for candidate in candidates.iter().copied().filter(|candidate| candidate.feasible) {
        if !candidate.score.is_finite() {
            return Err(format!("action {} has non-finite score", candidate.action_id));
        }
        if !candidate.uncertainty.is_finite() || candidate.uncertainty < 0.0 {
            return Err(format!("action {} has invalid uncertainty", candidate.action_id));
        }
        if ranked
            .iter()
            .any(|existing: &Candidate| existing.action_id == candidate.action_id)
        {
            return Err(format!("duplicate action id {}", candidate.action_id));
        }
        ranked.push(candidate);
    }
    if ranked.is_empty() {
        return Err("no feasible candidates".into());
    }
    ranked.sort_by(|left, right| {
        right
            .score
            .partial_cmp(&left.score)
            .unwrap_or(Ordering::Equal)
            .then_with(|| left.action_id.cmp(&right.action_id))
    });
    Ok(ranked)
}

fn probabilities(strategy: Strategy, ranked: &[Candidate]) -> Vec<f64> {
    let count = ranked.len();
    match strategy {
        Strategy::Greedy => one_hot(count, 0),
        Strategy::EpsilonGreedy { epsilon } => {
            if count == 1 {
                return vec![1.0];
            }
            let explore_each = epsilon / count as f64;
            let mut values = vec![explore_each; count];
            values[0] += 1.0 - epsilon;
            values
        }
        Strategy::RankWeighted { top_three } => {
            let selected_count = count.min(3);
            let total: f64 = top_three[..selected_count].iter().sum();
            let mut values = vec![0.0; count];
            for index in 0..selected_count {
                values[index] = top_three[index] / total;
            }
            values
        }
        Strategy::Boltzmann { temperature } => softmax(ranked, temperature, 0.0),
        Strategy::UncertaintyDirected { beta, temperature } => softmax(ranked, temperature, beta),
    }
}

fn softmax(ranked: &[Candidate], temperature: f64, beta: f64) -> Vec<f64> {
    let adjusted: Vec<f64> = ranked
        .iter()
        .map(|candidate| candidate.score + beta * candidate.uncertainty)
        .collect();
    let maximum = adjusted.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let mut values: Vec<f64> = adjusted
        .iter()
        .map(|value| ((value - maximum) / temperature).exp())
        .collect();
    let total: f64 = values.iter().sum();
    if !total.is_finite() || total <= 0.0 {
        return one_hot(ranked.len(), 0);
    }
    for value in &mut values {
        *value /= total;
    }
    values
}

fn one_hot(count: usize, selected: usize) -> Vec<f64> {
    let mut values = vec![0.0; count];
    values[selected] = 1.0;
    values
}

fn sample_index(probabilities: &[f64], draw: f64) -> usize {
    let mut cumulative = 0.0;
    for (index, probability) in probabilities.iter().copied().enumerate() {
        cumulative += probability;
        if draw < cumulative {
            return index;
        }
    }
    probabilities.len() - 1
}

fn deterministic_unit_draw(seed: u64, nonce: u64) -> f64 {
    let mixed = splitmix64(seed ^ nonce.rotate_left(29));
    (mixed >> 11) as f64 / (1_u64 << 53) as f64
}

fn splitmix64(mut value: u64) -> u64 {
    value = value.wrapping_add(0x9E37_79B9_7F4A_7C15);
    value = (value ^ (value >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    value ^ (value >> 31)
}

fn unit_interval(name: &str, value: f64) -> Result<(), String> {
    if value.is_finite() && (0.0..=1.0).contains(&value) {
        Ok(())
    } else {
        Err(format!("{name} must be finite and in [0, 1]"))
    }
}

fn positive(name: &str, value: f64) -> Result<(), String> {
    if value.is_finite() && value > 0.0 {
        Ok(())
    } else {
        Err(format!("{name} must be finite and positive"))
    }
}

fn non_negative(name: &str, value: f64) -> Result<(), String> {
    if value.is_finite() && value >= 0.0 {
        Ok(())
    } else {
        Err(format!("{name} must be finite and non-negative"))
    }
}

fn fixture_candidates() -> [Candidate; 4] {
    [
        Candidate { action_id: 10, score: 5.0, uncertainty: 0.1, feasible: true },
        Candidate { action_id: 20, score: 4.0, uncertainty: 0.2, feasible: true },
        Candidate { action_id: 30, score: 3.0, uncertainty: 2.0, feasible: true },
        Candidate { action_id: 40, score: 100.0, uncertainty: 0.0, feasible: false },
    ]
}

fn main() {
    let profile = Profile::rank_70_20_10(20260801);
    match select(profile, 7, &fixture_candidates()) {
        Ok(mut trace) => {
            if let Err(error) = trace.reconcile_executed(10, &fixture_candidates()) {
                eprintln!("{error}");
                std::process::exit(1);
            }
            println!("{}", trace.to_json());
        }
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn select_with_draw(profile: Profile, draw: f64) -> Trace {
        profile.validate().unwrap();
        let ranked = ranked_feasible(&fixture_candidates()).unwrap();
        let strategy = profile.effective_strategy();
        let distribution = probabilities(strategy, &ranked);
        let selected_rank = sample_index(&distribution, draw);
        let selected = ranked[selected_rank];
        let probability = distribution[selected_rank];
        Trace {
            schema_version: SCHEMA_VERSION,
            profile_name: profile.name,
            mode: profile.mode,
            strategy_name: strategy.as_str(),
            decision_nonce: 0,
            selected_action_id: selected.action_id,
            selected_rank,
            selected_probability: probability,
            selected_log_probability: probability.ln(),
            executed_action_id: selected.action_id,
            executed_rank: selected_rank,
            executed_probability: Some(probability),
            likelihood_status: LikelihoodStatus::ExactExecutedAction,
        }
    }

    #[test]
    fn rank_profile_selects_first_second_and_third() {
        let profile = Profile::rank_70_20_10(1);
        assert_eq!(select_with_draw(profile, 0.69).selected_action_id, 10);
        assert_eq!(select_with_draw(profile, 0.75).selected_action_id, 20);
        assert_eq!(select_with_draw(profile, 0.95).selected_action_id, 30);
    }

    #[test]
    fn seed_and_nonce_replay_exactly() {
        let profile = Profile::rank_70_20_10(99);
        assert_eq!(
            select(profile, 1234, &fixture_candidates()).unwrap(),
            select(profile, 1234, &fixture_candidates()).unwrap()
        );
    }

    #[test]
    fn evaluation_modes_force_greedy() {
        for mode in [Mode::Validation, Mode::Production] {
            let profile = Profile {
                schema_version: SCHEMA_VERSION,
                name: "eval",
                mode,
                strategy: Strategy::RankWeighted { top_three: [0.1, 0.2, 0.7] },
                seed: 1,
            };
            assert_eq!(select_with_draw(profile, 0.99).selected_action_id, 10);
        }
    }

    #[test]
    fn override_never_reuses_promoted_probability() {
        let mut trace = select_with_draw(Profile::rank_70_20_10(1), 0.75);
        assert_eq!(trace.selected_action_id, 20);
        trace.reconcile_executed(10, &fixture_candidates()).unwrap();
        assert_eq!(trace.executed_probability, None);
        assert_eq!(trace.likelihood_status, LikelihoodStatus::UnknownAfterDownstreamOverride);
    }

    #[test]
    fn uncertainty_can_promote_lower_scored_action() {
        let profile = Profile {
            schema_version: SCHEMA_VERSION,
            name: "uncertainty",
            mode: Mode::Training,
            strategy: Strategy::UncertaintyDirected { beta: 2.0, temperature: 0.05 },
            seed: 1,
        };
        assert_eq!(select_with_draw(profile, 0.5).selected_action_id, 30);
    }

    #[test]
    fn invalid_and_duplicate_candidates_are_rejected() {
        let profile = Profile::rank_70_20_10(1);
        let duplicate = [
            Candidate { action_id: 1, score: 1.0, uncertainty: 0.0, feasible: true },
            Candidate { action_id: 1, score: 0.0, uncertainty: 0.0, feasible: true },
        ];
        assert!(select(profile, 1, &duplicate).is_err());
        let non_finite = [Candidate {
            action_id: 2,
            score: f64::NAN,
            uncertainty: 0.0,
            feasible: true,
        }];
        assert!(select(profile, 1, &non_finite).is_err());
    }

    #[test]
    fn all_supported_strategies_validate() {
        let strategies = [
            Strategy::Greedy,
            Strategy::EpsilonGreedy { epsilon: 0.1 },
            Strategy::RankWeighted { top_three: [0.7, 0.2, 0.1] },
            Strategy::Boltzmann { temperature: 0.5 },
            Strategy::UncertaintyDirected { beta: 1.0, temperature: 0.5 },
        ];
        for strategy in strategies {
            Profile {
                schema_version: SCHEMA_VERSION,
                name: "valid",
                mode: Mode::Adaptation,
                strategy,
                seed: 1,
            }
            .validate()
            .unwrap();
        }
    }
}
