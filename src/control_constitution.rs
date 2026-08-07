//! Default-deny control constitution for administrative decision surfaces.
//!
//! The registry is data, not model code. Learned policies may propose registered
//! actions, but they cannot add actions, change governance weights, waive rights,
//! or bypass approvals. Every evaluation returns a proof-like decision record.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use thiserror::Error;

const DEFAULT_REGISTRY_JSON: &str = include_str!("../policy/action-registry.v1.json");

#[derive(Debug, Error)]
pub enum RegistryError {
    #[error("invalid embedded action registry: {0}")]
    InvalidRegistry(#[from] serde_json::Error),
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct ActionRegistry {
    pub schema_version: u32,
    pub policy_version: String,
    pub prohibited_features: Vec<ProhibitedFeature>,
    pub actions: Vec<ActionRule>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct ProhibitedFeature {
    pub id: String,
    pub rationale: String,
    pub governing_sources: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct ActionRule {
    pub id: String,
    pub actor: String,
    pub trigger: String,
    pub allowed_observations: Vec<String>,
    pub prohibited_observations: Vec<String>,
    pub action_class: ActionClass,
    pub reversible: bool,
    pub risk_tier: RiskTier,
    pub required_approvals: Vec<String>,
    pub preconditions: Vec<String>,
    pub postconditions: Vec<String>,
    pub shadow_eligible: bool,
    pub canary_eligible: bool,
    pub rollback: String,
    pub public_disclosure_required: bool,
    pub governing_sources: Vec<String>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ActionClass {
    Advisory,
    ReversibleAdministrative,
    HumanExecuted,
    GovernanceDeployment,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RiskTier {
    Low,
    Moderate,
    High,
    Critical,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionProposal {
    pub action_id: String,
    pub observations: Vec<String>,
    pub approvals: Vec<String>,
    /// Named registry preconditions that are true for this proposal.
    pub context_flags: BTreeMap<String, bool>,
    /// Policies and models are categorically forbidden from changing this registry.
    pub attempts_registry_mutation: bool,
    /// Policies and models are categorically forbidden from changing voting,
    /// quorum, appeal, notice, recusal, or other governance weights.
    pub attempts_governance_weight_mutation: bool,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DecisionOutcome {
    Allow,
    Deny,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct DecisionRecord {
    pub schema_version: u32,
    pub policy_version: String,
    pub action_id: String,
    pub outcome: DecisionOutcome,
    pub satisfied_constraints: Vec<String>,
    pub denial_reasons: Vec<String>,
    pub required_approvals: Vec<String>,
    pub governing_sources: Vec<String>,
}

pub fn default_registry() -> Result<ActionRegistry, RegistryError> {
    Ok(serde_json::from_str(DEFAULT_REGISTRY_JSON)?)
}

/// Evaluate a proposed administrative action under a default-deny policy.
///
/// Unknown actions, unknown observations, prohibited observations, missing
/// approvals, false preconditions, and any attempt to mutate governance are
/// denied. The evaluator never infers an approval or silently drops a failure.
pub fn evaluate(registry: &ActionRegistry, proposal: &ActionProposal) -> DecisionRecord {
    let Some(rule) = registry
        .actions
        .iter()
        .find(|candidate| candidate.id == proposal.action_id)
    else {
        return DecisionRecord {
            schema_version: registry.schema_version,
            policy_version: registry.policy_version.clone(),
            action_id: proposal.action_id.clone(),
            outcome: DecisionOutcome::Deny,
            satisfied_constraints: Vec::new(),
            denial_reasons: vec!["unregistered_action".to_string()],
            required_approvals: Vec::new(),
            governing_sources: Vec::new(),
        };
    };

    let global_prohibited: BTreeSet<&str> = registry
        .prohibited_features
        .iter()
        .map(|feature| feature.id.as_str())
        .collect();
    let action_prohibited: BTreeSet<&str> = rule
        .prohibited_observations
        .iter()
        .map(String::as_str)
        .collect();
    let action_allowed: BTreeSet<&str> = rule
        .allowed_observations
        .iter()
        .map(String::as_str)
        .collect();
    let supplied_approvals: BTreeSet<&str> =
        proposal.approvals.iter().map(String::as_str).collect();

    let mut satisfied_constraints = Vec::new();
    let mut denial_reasons = Vec::new();

    if proposal.attempts_registry_mutation {
        denial_reasons.push("registry_mutation_forbidden".to_string());
    } else {
        satisfied_constraints.push("registry_immutable_to_policy".to_string());
    }

    if proposal.attempts_governance_weight_mutation {
        denial_reasons.push("governance_weight_mutation_forbidden".to_string());
    } else {
        satisfied_constraints.push("governance_weights_immutable_to_policy".to_string());
    }

    for observation in &proposal.observations {
        if global_prohibited.contains(observation.as_str())
            || action_prohibited.contains(observation.as_str())
        {
            denial_reasons.push(format!("prohibited_observation:{observation}"));
        } else if !action_allowed.contains(observation.as_str()) {
            denial_reasons.push(format!("unregistered_observation:{observation}"));
        } else {
            satisfied_constraints.push(format!("allowed_observation:{observation}"));
        }
    }

    for approval in &rule.required_approvals {
        if supplied_approvals.contains(approval.as_str()) {
            satisfied_constraints.push(format!("approval:{approval}"));
        } else {
            denial_reasons.push(format!("missing_approval:{approval}"));
        }
    }

    for precondition in &rule.preconditions {
        if proposal.context_flags.get(precondition).copied() == Some(true) {
            satisfied_constraints.push(format!("precondition:{precondition}"));
        } else {
            denial_reasons.push(format!("unsatisfied_precondition:{precondition}"));
        }
    }

    denial_reasons.sort();
    denial_reasons.dedup();
    satisfied_constraints.sort();
    satisfied_constraints.dedup();

    DecisionRecord {
        schema_version: registry.schema_version,
        policy_version: registry.policy_version.clone(),
        action_id: proposal.action_id.clone(),
        outcome: if denial_reasons.is_empty() {
            DecisionOutcome::Allow
        } else {
            DecisionOutcome::Deny
        },
        satisfied_constraints,
        denial_reasons,
        required_approvals: rule.required_approvals.clone(),
        governing_sources: rule.governing_sources.clone(),
    }
}

/// Render the registry as a stable human-reviewable reference.
pub fn render_markdown(registry: &ActionRegistry) -> String {
    let mut output = format!(
        "# USA-ACC action registry\n\nSchema: `{}`  \nPolicy: `{}`\n\n",
        registry.schema_version, registry.policy_version
    );
    for action in &registry.actions {
        output.push_str(&format!(
            "## `{}`\n\n- Actor: `{}`\n- Trigger: {}\n- Class: `{:?}`\n- Risk: `{:?}`\n- Reversible: `{}`\n- Shadow eligible: `{}`\n- Canary eligible: `{}`\n- Public disclosure required: `{}`\n- Required approvals: {}\n- Rollback: {}\n\n",
            action.id,
            action.actor,
            action.trigger,
            action.action_class,
            action.risk_tier,
            action.reversible,
            action.shadow_eligible,
            action.canary_eligible,
            action.public_disclosure_required,
            action.required_approvals.join(", "),
            action.rollback,
        ));
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proposal(action_id: &str) -> ActionProposal {
        ActionProposal {
            action_id: action_id.to_string(),
            observations: Vec::new(),
            approvals: Vec::new(),
            context_flags: BTreeMap::new(),
            attempts_registry_mutation: false,
            attempts_governance_weight_mutation: false,
        }
    }

    #[test]
    fn embedded_registry_is_parseable_unique_and_reviewable() {
        let registry = default_registry().expect("embedded registry");
        assert_eq!(registry.schema_version, 1);
        assert!(registry.actions.len() >= 11);
        let ids: BTreeSet<_> = registry.actions.iter().map(|action| &action.id).collect();
        assert_eq!(ids.len(), registry.actions.len());
        assert!(registry.actions.iter().all(|action| {
            !action.governing_sources.is_empty()
                && !action.rollback.is_empty()
                && !action.postconditions.is_empty()
        }));
        let reference = render_markdown(&registry);
        assert!(reference.contains("assignment.public_lottery"));
        assert!(reference.contains("policy.deploy"));
    }

    #[test]
    fn unregistered_action_is_denied() {
        let registry = default_registry().expect("registry");
        let decision = evaluate(&registry, &proposal("merits.predict_verdict"));
        assert_eq!(decision.outcome, DecisionOutcome::Deny);
        assert_eq!(decision.denial_reasons, ["unregistered_action"]);
    }

    #[test]
    fn learned_policy_cannot_mutate_registry_or_governance_weights() {
        let registry = default_registry().expect("registry");
        let mut proposal = proposal("capacity.judge_pool");
        proposal.attempts_registry_mutation = true;
        proposal.attempts_governance_weight_mutation = true;
        let decision = evaluate(&registry, &proposal);
        assert_eq!(decision.outcome, DecisionOutcome::Deny);
        assert!(decision
            .denial_reasons
            .contains(&"registry_mutation_forbidden".to_string()));
        assert!(decision
            .denial_reasons
            .contains(&"governance_weight_mutation_forbidden".to_string()));
    }

    #[test]
    fn prohibited_merits_and_credibility_features_are_denied() {
        let registry = default_registry().expect("registry");
        let mut proposal = proposal("priority.expedite");
        proposal.observations = vec![
            "public_media_salience".to_string(),
            "witness_credibility_score".to_string(),
        ];
        let decision = evaluate(&registry, &proposal);
        assert_eq!(decision.outcome, DecisionOutcome::Deny);
        assert_eq!(decision.denial_reasons.len(), 2);
    }

    #[test]
    fn administrative_schedule_can_pass_with_explicit_rights_and_approval() {
        let registry = default_registry().expect("registry");
        let mut proposal = proposal("hearing.schedule");
        proposal.observations = vec![
            "participant_availability".to_string(),
            "accessibility_accommodation".to_string(),
            "statutory_deadline".to_string(),
        ];
        proposal.approvals = vec!["scheduling_officer".to_string()];
        proposal.context_flags = BTreeMap::from([
            ("notice_window_preserved".to_string(), true),
            ("non_digital_fallback_available".to_string(), true),
        ]);
        let decision = evaluate(&registry, &proposal);
        assert_eq!(decision.outcome, DecisionOutcome::Allow);
        assert!(decision.denial_reasons.is_empty());
    }

    #[test]
    fn missing_human_approval_and_fallback_fail_closed() {
        let registry = default_registry().expect("registry");
        let mut proposal = proposal("hearing.schedule");
        proposal.observations = vec!["participant_availability".to_string()];
        proposal
            .context_flags
            .insert("notice_window_preserved".to_string(), true);
        let decision = evaluate(&registry, &proposal);
        assert_eq!(decision.outcome, DecisionOutcome::Deny);
        assert!(decision
            .denial_reasons
            .contains(&"missing_approval:scheduling_officer".to_string()));
        assert!(decision
            .denial_reasons
            .contains(&"unsatisfied_precondition:non_digital_fallback_available".to_string()));
    }

    #[test]
    fn policy_deployment_requires_dual_control_shadow_evidence_and_rollback() {
        let registry = default_registry().expect("registry");
        let mut proposal = proposal("policy.deploy");
        proposal.observations = vec![
            "offline_evaluation_result".to_string(),
            "rights_regression_result".to_string(),
            "rollback_drill_result".to_string(),
        ];
        proposal.approvals = vec![
            "governance_reviewer".to_string(),
            "rights_reviewer".to_string(),
            "security_reviewer".to_string(),
        ];
        proposal.context_flags = BTreeMap::from([
            ("signed_build_provenance".to_string(), true),
            ("shadow_evaluation_passed".to_string(), true),
            ("rollback_tested".to_string(), true),
            ("active_matters_policy_frozen".to_string(), true),
        ]);
        let decision = evaluate(&registry, &proposal);
        assert_eq!(decision.outcome, DecisionOutcome::Allow);
    }
}
