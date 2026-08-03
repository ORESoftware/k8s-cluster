use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

pub const DEFAULT_HOSTED_RUNS_ON: &str = "ubuntu-latest";

#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct OrgPolicy {
    pub included_minutes: Option<f64>,
    #[serde(default = "default_warn_percent")]
    pub warn_percent: f64,
    #[serde(default = "default_self_hosted_percent")]
    pub self_hosted_percent: f64,
    #[serde(default = "default_hard_stop_percent")]
    pub hard_stop_percent: f64,
    #[serde(default)]
    pub prefer_self_hosted: bool,
    #[serde(default)]
    pub self_hosted_ready: bool,
    #[serde(default)]
    pub build_server_enabled: bool,
    #[serde(default = "default_hosted_runs_on")]
    pub hosted_runs_on: Vec<String>,
    #[serde(default)]
    pub self_hosted_runs_on: Vec<String>,
    #[serde(default)]
    pub selected_repository_ids: Vec<u64>,
}

fn default_warn_percent() -> f64 {
    75.0
}

fn default_self_hosted_percent() -> f64 {
    90.0
}

fn default_hard_stop_percent() -> f64 {
    100.0
}

fn default_hosted_runs_on() -> Vec<String> {
    vec![DEFAULT_HOSTED_RUNS_ON.to_string()]
}

impl OrgPolicy {
    pub fn validate(&self) -> Result<(), String> {
        if !self.warn_percent.is_finite() || !(0.0..=100.0).contains(&self.warn_percent) {
            return Err("warnPercent must be between 0 and 100".to_string());
        }
        if !self.self_hosted_percent.is_finite()
            || !(0.0..=100.0).contains(&self.self_hosted_percent)
        {
            return Err("selfHostedPercent must be between 0 and 100".to_string());
        }
        if !self.hard_stop_percent.is_finite() || self.hard_stop_percent < 100.0 {
            return Err("hardStopPercent must be at least 100".to_string());
        }
        if self.warn_percent > self.self_hosted_percent {
            return Err("warnPercent must not exceed selfHostedPercent".to_string());
        }
        if self.self_hosted_percent > self.hard_stop_percent {
            return Err("selfHostedPercent must not exceed hardStopPercent".to_string());
        }
        if self
            .included_minutes
            .is_some_and(|minutes| !minutes.is_finite() || minutes <= 0.0)
        {
            return Err("includedMinutes must be positive when configured".to_string());
        }
        validate_runs_on(&self.hosted_runs_on, "hostedRunsOn")?;
        validate_runs_on(&self.self_hosted_runs_on, "selfHostedRunsOn")?;
        Ok(())
    }
}

fn validate_runs_on(values: &[String], field: &str) -> Result<(), String> {
    if values.is_empty() {
        return Err(format!("{field} must contain at least one label"));
    }
    if values.len() > 8 {
        return Err(format!("{field} must contain no more than eight labels"));
    }
    for value in values {
        let trimmed = value.trim();
        if trimmed.is_empty() || trimmed.len() > 100 {
            return Err(format!("{field} contains an empty or oversized label"));
        }
        if !trimmed
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
        {
            return Err(format!("{field} contains an invalid label: {trimmed}"));
        }
    }
    Ok(())
}

#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct BillingUsageItem {
    pub product: String,
    pub sku: String,
    pub unit_type: String,
    pub quantity: f64,
    #[serde(default)]
    pub organization_name: Option<String>,
    #[serde(default)]
    pub repository_name: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct BillingUsageResponse {
    #[serde(default)]
    pub usage_items: Vec<BillingUsageItem>,
}

impl BillingUsageResponse {
    pub fn actions_minutes(&self) -> f64 {
        self.usage_items
            .iter()
            .filter(|item| {
                item.product.eq_ignore_ascii_case("Actions")
                    && item.unit_type.eq_ignore_ascii_case("minutes")
            })
            .map(|item| item.quantity.max(0.0))
            .sum()
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, ToSchema)]
#[serde(rename_all = "kebab-case")]
pub enum ExecutionMode {
    Hosted,
    SelfHosted,
    BuildServer,
    Hold,
}

#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CapacityDecision {
    pub mode: ExecutionMode,
    pub runs_on: Vec<String>,
    pub reason: String,
    pub actions_minutes: Option<f64>,
    pub usage_percent: Option<f64>,
    pub warnings: Vec<String>,
}

pub fn decide_capacity(policy: &OrgPolicy, actions_minutes: Option<f64>) -> CapacityDecision {
    let usage_percent = match (actions_minutes, policy.included_minutes) {
        (Some(used), Some(included)) if included > 0.0 => Some((used / included) * 100.0),
        _ => None,
    };

    let mut warnings = Vec::new();
    if let Some(percent) = usage_percent {
        if percent >= policy.warn_percent {
            warnings.push(format!(
                "Actions usage is at {percent:.1}% of the configured included-minute budget"
            ));
        }
        if percent >= policy.self_hosted_percent {
            warnings.push("hosted capacity should no longer be the primary Linux lane".to_string());
        }
        if percent >= policy.hard_stop_percent {
            warnings.push("hosted runner allocation may be blocked by budget policy".to_string());
        }
    }

    if policy.prefer_self_hosted && policy.self_hosted_ready {
        return CapacityDecision {
            mode: ExecutionMode::SelfHosted,
            runs_on: policy.self_hosted_runs_on.clone(),
            reason: "policy prefers the validated self-hosted Linux lane".to_string(),
            actions_minutes,
            usage_percent,
            warnings,
        };
    }

    match usage_percent {
        Some(percent) if percent >= policy.hard_stop_percent => {
            if policy.self_hosted_ready {
                CapacityDecision {
                    mode: ExecutionMode::SelfHosted,
                    runs_on: policy.self_hosted_runs_on.clone(),
                    reason:
                        "configured hosted-minute hard stop reached; using validated ARC capacity"
                            .to_string(),
                    actions_minutes,
                    usage_percent,
                    warnings,
                }
            } else if policy.build_server_enabled {
                CapacityDecision {
                    mode: ExecutionMode::BuildServer,
                    runs_on: Vec::new(),
                    reason: "hosted-minute hard stop reached and ARC is not certified; only reviewed build-server profiles may proceed"
                        .to_string(),
                    actions_minutes,
                    usage_percent,
                    warnings,
                }
            } else {
                CapacityDecision {
                    mode: ExecutionMode::Hold,
                    runs_on: Vec::new(),
                    reason:
                        "hosted-minute hard stop reached and no certified fallback is available"
                            .to_string(),
                    actions_minutes,
                    usage_percent,
                    warnings,
                }
            }
        }
        Some(percent) if percent >= policy.self_hosted_percent && policy.self_hosted_ready => {
            CapacityDecision {
                mode: ExecutionMode::SelfHosted,
                runs_on: policy.self_hosted_runs_on.clone(),
                reason: "configured self-hosted threshold reached".to_string(),
                actions_minutes,
                usage_percent,
                warnings,
            }
        }
        Some(_) => CapacityDecision {
            mode: ExecutionMode::Hosted,
            runs_on: policy.hosted_runs_on.clone(),
            reason: "hosted-minute usage remains below the configured routing threshold"
                .to_string(),
            actions_minutes,
            usage_percent,
            warnings,
        },
        None if policy.self_hosted_ready => CapacityDecision {
            mode: ExecutionMode::SelfHosted,
            runs_on: policy.self_hosted_runs_on.clone(),
            reason:
                "billing usage is unavailable; failing closed onto validated self-hosted capacity"
                    .to_string(),
            actions_minutes,
            usage_percent,
            warnings,
        },
        None => CapacityDecision {
            mode: ExecutionMode::Hold,
            runs_on: Vec::new(),
            reason: "billing usage is unavailable and self-hosted readiness is not certified"
                .to_string(),
            actions_minutes,
            usage_percent,
            warnings,
        },
    }
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct VariableMutation {
    pub name: String,
    pub value: String,
    pub visibility: String,
    pub selected_repository_ids: Vec<u64>,
}

pub fn decision_variables(
    policy: &OrgPolicy,
    decision: &CapacityDecision,
) -> Result<BTreeMap<String, VariableMutation>, String> {
    if policy.selected_repository_ids.is_empty() {
        return Err(
            "selectedRepositoryIds must be non-empty before organization variables can mutate"
                .to_string(),
        );
    }
    let runs_on = serde_json::to_string(&decision.runs_on)
        .map_err(|error| format!("failed to serialize runs-on labels: {error}"))?;
    let mode = match decision.mode {
        ExecutionMode::Hosted => "hosted",
        ExecutionMode::SelfHosted => "self-hosted",
        ExecutionMode::BuildServer => "build-server",
        ExecutionMode::Hold => "hold",
    };
    let mut values = BTreeMap::new();
    values.insert(
        "CI_EXECUTION_MODE".to_string(),
        VariableMutation {
            name: "CI_EXECUTION_MODE".to_string(),
            value: mode.to_string(),
            visibility: "selected".to_string(),
            selected_repository_ids: policy.selected_repository_ids.clone(),
        },
    );
    values.insert(
        "CI_LINUX_RUNS_ON_JSON".to_string(),
        VariableMutation {
            name: "CI_LINUX_RUNS_ON_JSON".to_string(),
            value: runs_on,
            visibility: "selected".to_string(),
            selected_repository_ids: policy.selected_repository_ids.clone(),
        },
    );
    Ok(values)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy() -> OrgPolicy {
        OrgPolicy {
            included_minutes: Some(2_000.0),
            warn_percent: 75.0,
            self_hosted_percent: 90.0,
            hard_stop_percent: 100.0,
            prefer_self_hosted: false,
            self_hosted_ready: true,
            build_server_enabled: true,
            hosted_runs_on: vec!["ubuntu-latest".to_string()],
            self_hosted_runs_on: vec!["sonus-ci".to_string()],
            selected_repository_ids: vec![1, 2],
        }
    }

    #[test]
    fn sums_only_actions_minutes() {
        let usage = BillingUsageResponse {
            usage_items: vec![
                BillingUsageItem {
                    product: "Actions".to_string(),
                    sku: "Actions Linux".to_string(),
                    unit_type: "minutes".to_string(),
                    quantity: 125.5,
                    organization_name: None,
                    repository_name: None,
                },
                BillingUsageItem {
                    product: "Packages".to_string(),
                    sku: "storage".to_string(),
                    unit_type: "gigabytes".to_string(),
                    quantity: 900.0,
                    organization_name: None,
                    repository_name: None,
                },
                BillingUsageItem {
                    product: "Actions".to_string(),
                    sku: "Actions Windows".to_string(),
                    unit_type: "minutes".to_string(),
                    quantity: 24.5,
                    organization_name: None,
                    repository_name: None,
                },
            ],
        };
        assert_eq!(usage.actions_minutes(), 150.0);
    }

    #[test]
    fn stays_hosted_below_threshold() {
        let decision = decide_capacity(&policy(), Some(1_000.0));
        assert_eq!(decision.mode, ExecutionMode::Hosted);
        assert_eq!(decision.runs_on, vec!["ubuntu-latest"]);
    }

    #[test]
    fn moves_to_arc_at_threshold() {
        let decision = decide_capacity(&policy(), Some(1_800.0));
        assert_eq!(decision.mode, ExecutionMode::SelfHosted);
        assert_eq!(decision.runs_on, vec!["sonus-ci"]);
    }

    #[test]
    fn uses_bounded_build_server_only_at_hard_stop() {
        let mut value = policy();
        value.self_hosted_ready = false;
        let decision = decide_capacity(&value, Some(2_000.0));
        assert_eq!(decision.mode, ExecutionMode::BuildServer);
        assert!(decision.runs_on.is_empty());
    }

    #[test]
    fn holds_when_no_certified_capacity_exists() {
        let mut value = policy();
        value.self_hosted_ready = false;
        value.build_server_enabled = false;
        let decision = decide_capacity(&value, Some(2_000.0));
        assert_eq!(decision.mode, ExecutionMode::Hold);
    }

    #[test]
    fn billing_failure_fails_closed_to_validated_arc() {
        let decision = decide_capacity(&policy(), None);
        assert_eq!(decision.mode, ExecutionMode::SelfHosted);
    }

    #[test]
    fn variable_mutation_is_selected_repository_only() {
        let decision = decide_capacity(&policy(), Some(1_900.0));
        let variables = decision_variables(&policy(), &decision).expect("variables");
        assert_eq!(variables["CI_LINUX_RUNS_ON_JSON"].visibility, "selected");
        assert_eq!(variables["CI_LINUX_RUNS_ON_JSON"].value, "[\"sonus-ci\"]");
        assert_eq!(variables["CI_EXECUTION_MODE"].value, "self-hosted");
    }

    #[test]
    fn broad_variable_visibility_is_impossible_without_repo_ids() {
        let mut value = policy();
        value.selected_repository_ids.clear();
        let decision = decide_capacity(&value, Some(1_900.0));
        assert!(decision_variables(&value, &decision).is_err());
    }

    #[test]
    fn prefer_self_hosted_overrides_low_usage_after_certification() {
        let mut value = policy();
        value.prefer_self_hosted = true;
        let decision = decide_capacity(&value, Some(10.0));
        assert_eq!(decision.mode, ExecutionMode::SelfHosted);
        assert_eq!(decision.runs_on, vec!["sonus-ci"]);
    }

    #[test]
    fn prefer_self_hosted_does_not_bypass_readiness() {
        let mut value = policy();
        value.prefer_self_hosted = true;
        value.self_hosted_ready = false;
        let decision = decide_capacity(&value, Some(10.0));
        assert_eq!(decision.mode, ExecutionMode::Hosted);
    }

    #[test]
    fn unavailable_billing_holds_before_arc_certification() {
        let mut value = policy();
        value.self_hosted_ready = false;
        let decision = decide_capacity(&value, None);
        assert_eq!(decision.mode, ExecutionMode::Hold);
    }

    #[test]
    fn negative_usage_is_clamped_and_product_matching_is_case_insensitive() {
        let usage = BillingUsageResponse {
            usage_items: vec![
                BillingUsageItem {
                    product: "actions".to_string(),
                    sku: "Actions Linux".to_string(),
                    unit_type: "MINUTES".to_string(),
                    quantity: -50.0,
                    organization_name: None,
                    repository_name: None,
                },
                BillingUsageItem {
                    product: "ACTIONS".to_string(),
                    sku: "Actions Linux".to_string(),
                    unit_type: "minutes".to_string(),
                    quantity: 12.5,
                    organization_name: None,
                    repository_name: None,
                },
            ],
        };
        assert_eq!(usage.actions_minutes(), 12.5);
    }

    #[test]
    fn self_hosted_label_must_be_explicit() {
        let mut value = policy();
        value.self_hosted_runs_on.clear();
        assert!(value.validate().is_err());
    }

    #[test]
    fn rejects_non_finite_policy_numbers() {
        let mut value = policy();
        value.warn_percent = f64::NAN;
        assert!(value.validate().is_err());
        value = policy();
        value.included_minutes = Some(f64::INFINITY);
        assert!(value.validate().is_err());
    }

    #[test]
    fn validates_monotonic_thresholds() {
        let mut value = policy();
        value.warn_percent = 95.0;
        value.self_hosted_percent = 90.0;
        assert!(value.validate().is_err());
    }
}
