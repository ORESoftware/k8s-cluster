use gha_capacity_broker::{
    decide_capacity, decision_variables, BillingUsageItem, BillingUsageResponse, ExecutionMode,
    OrgPolicy, CI_HOLD_RUNNER_LABEL,
};

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
        selected_repository_ids: vec![101, 202],
    }
}

#[test]
fn threshold_boundaries_preserve_warn_route_and_hard_stop_semantics() {
    let below_warning = decide_capacity(&policy(), Some(1_499.0));
    assert_eq!(below_warning.mode, ExecutionMode::Hosted);
    assert!(below_warning.warnings.is_empty());

    let at_warning = decide_capacity(&policy(), Some(1_500.0));
    assert_eq!(at_warning.mode, ExecutionMode::Hosted);
    assert_eq!(at_warning.usage_percent, Some(75.0));
    assert!(at_warning
        .warnings
        .iter()
        .any(|warning| warning.contains("75.0%")));

    let at_self_hosted = decide_capacity(&policy(), Some(1_800.0));
    assert_eq!(at_self_hosted.mode, ExecutionMode::SelfHosted);
    assert_eq!(at_self_hosted.runs_on, vec!["sonus-ci"]);

    let at_hard_stop = decide_capacity(&policy(), Some(2_000.0));
    assert_eq!(at_hard_stop.mode, ExecutionMode::SelfHosted);
    assert!(at_hard_stop
        .warnings
        .iter()
        .any(|warning| warning.contains("hard stop")));
}

#[test]
fn unknown_billing_never_falls_back_to_unverified_hosted_capacity() {
    let certified = decide_capacity(&policy(), None);
    assert_eq!(certified.mode, ExecutionMode::SelfHosted);

    let mut unready = policy();
    unready.self_hosted_ready = false;
    let held = decide_capacity(&unready, None);
    assert_eq!(held.mode, ExecutionMode::Hold);

    let variables = decision_variables(&unready, &held).expect("hold variables");
    assert_eq!(variables["CI_EXECUTION_MODE"].value, "hold");
    assert_eq!(
        variables["CI_LINUX_RUNS_ON_JSON"].value,
        serde_json::to_string(&vec![CI_HOLD_RUNNER_LABEL]).expect("sentinel JSON")
    );
}

#[test]
fn build_server_is_only_a_signal_and_never_an_empty_github_runner_target() {
    let mut value = policy();
    value.self_hosted_ready = false;
    let decision = decide_capacity(&value, Some(2_000.0));
    assert_eq!(decision.mode, ExecutionMode::BuildServer);
    assert!(decision.runs_on.is_empty());

    let variables = decision_variables(&value, &decision).expect("build-server variables");
    assert_eq!(variables["CI_EXECUTION_MODE"].value, "build-server");
    assert_eq!(
        variables["CI_LINUX_RUNS_ON_JSON"].value,
        "[\"ci-capacity-hold-no-runner\"]"
    );
}

#[test]
fn variable_visibility_and_repository_allowlist_are_preserved_exactly() {
    let value = policy();
    let decision = decide_capacity(&value, Some(1_900.0));
    let variables = decision_variables(&value, &decision).expect("variables");

    for mutation in variables.values() {
        assert_eq!(mutation.visibility, "selected");
        assert_eq!(mutation.selected_repository_ids, vec![101, 202]);
    }
}

#[test]
fn invalid_label_and_repository_policies_fail_before_mutation() {
    let mut overlapping = policy();
    overlapping.self_hosted_runs_on = vec!["UBUNTU-LATEST".to_string()];
    assert!(overlapping.validate().is_err());
    let decision = decide_capacity(&overlapping, Some(1_900.0));
    assert!(decision_variables(&overlapping, &decision).is_err());

    let mut duplicate_repositories = policy();
    duplicate_repositories.selected_repository_ids = vec![101, 101];
    assert!(duplicate_repositories.validate().is_err());
    let decision = decide_capacity(&duplicate_repositories, Some(1_900.0));
    assert!(decision_variables(&duplicate_repositories, &decision).is_err());
}

#[test]
fn billing_summary_counts_only_nonnegative_actions_minutes() {
    let usage = BillingUsageResponse {
        usage_items: vec![
            BillingUsageItem {
                product: "Actions".to_string(),
                sku: "Actions Linux".to_string(),
                unit_type: "minutes".to_string(),
                quantity: 123.0,
                organization_name: Some("sonus-auris".to_string()),
                repository_name: Some("sonus-auris-monorepo".to_string()),
            },
            BillingUsageItem {
                product: "actions".to_string(),
                sku: "Actions Windows".to_string(),
                unit_type: "MINUTES".to_string(),
                quantity: 7.0,
                organization_name: Some("sonus-auris".to_string()),
                repository_name: None,
            },
            BillingUsageItem {
                product: "Actions".to_string(),
                sku: "refund".to_string(),
                unit_type: "minutes".to_string(),
                quantity: -50.0,
                organization_name: Some("sonus-auris".to_string()),
                repository_name: None,
            },
            BillingUsageItem {
                product: "Packages".to_string(),
                sku: "storage".to_string(),
                unit_type: "gigabytes".to_string(),
                quantity: 900.0,
                organization_name: Some("sonus-auris".to_string()),
                repository_name: None,
            },
        ],
    };

    assert_eq!(usage.actions_minutes(), 130.0);
}

#[test]
fn explicit_self_hosted_preference_still_requires_certification() {
    let mut certified = policy();
    certified.prefer_self_hosted = true;
    let decision = decide_capacity(&certified, Some(1.0));
    assert_eq!(decision.mode, ExecutionMode::SelfHosted);

    certified.self_hosted_ready = false;
    let decision = decide_capacity(&certified, Some(1.0));
    assert_eq!(decision.mode, ExecutionMode::Hosted);
}
