use std::{
    collections::BTreeMap,
    env,
    process::ExitCode,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use reqwest::{redirect::Policy as RedirectPolicy, Client, StatusCode};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::json;

const API_BASE: &str = "https://api.github.com";
const API_VERSION: &str = "2026-03-10";
const USER_AGENT: &str = "oresoftware-gha-capacity-audit/0.1";
const DEFAULT_WARN_PERCENT: f64 = 75.0;
const DEFAULT_CRITICAL_PERCENT: f64 = 90.0;
const DEFAULT_HARD_PERCENT: f64 = 100.0;
const HOSTED_RUNS_ON_JSON: &str = "[\"ubuntu-latest\"]";
const SELF_HOSTED_RUNS_ON_JSON: &str = "[\"self-hosted\",\"linux\",\"sonus-ci\"]";

#[derive(Clone)]
struct Config {
    organization: String,
    token: String,
    included_minutes: Option<f64>,
    warn_percent: f64,
    critical_percent: f64,
    hard_percent: f64,
    mutation_enabled: bool,
    selected_repository_ids: Vec<u64>,
}

impl Config {
    fn from_env() -> Result<Self, String> {
        let organization = required_env("GHA_CAPACITY_ORGANIZATION")?;
        validate_organization(&organization)?;
        let token = optional_env("GHA_CAPACITY_GITHUB_TOKEN")
            .or_else(|| optional_env("GHA_CLONE_GITHUB_TOKEN"))
            .ok_or_else(|| {
                "GHA_CAPACITY_GITHUB_TOKEN or GHA_CLONE_GITHUB_TOKEN is required"
                    .to_string()
            })?;
        let included_minutes = optional_positive_f64("GHA_CAPACITY_INCLUDED_MINUTES")?;
        let warn_percent = optional_positive_f64("GHA_CAPACITY_WARN_PERCENT")?
            .unwrap_or(DEFAULT_WARN_PERCENT);
        let critical_percent = optional_positive_f64("GHA_CAPACITY_CRITICAL_PERCENT")?
            .unwrap_or(DEFAULT_CRITICAL_PERCENT);
        let hard_percent = optional_positive_f64("GHA_CAPACITY_HARD_PERCENT")?
            .unwrap_or(DEFAULT_HARD_PERCENT);
        if !(warn_percent < critical_percent && critical_percent <= hard_percent) {
            return Err("capacity thresholds must satisfy warn < critical <= hard".to_string());
        }
        if hard_percent < 100.0 {
            return Err("GHA_CAPACITY_HARD_PERCENT must be at least 100".to_string());
        }
        let mutation_enabled = env_bool("GHA_CAPACITY_MUTATION_ENABLED", false);
        let selected_repository_ids = parse_repository_ids(
            optional_env("GHA_CAPACITY_SELECTED_REPOSITORY_IDS").as_deref(),
        )?;
        if mutation_enabled && selected_repository_ids.is_empty() {
            return Err(
                "GHA_CAPACITY_SELECTED_REPOSITORY_IDS is required when mutation is enabled"
                    .to_string(),
            );
        }
        Ok(Self {
            organization,
            token,
            included_minutes,
            warn_percent,
            critical_percent,
            hard_percent,
            mutation_enabled,
            selected_repository_ids,
        })
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UsageResponse {
    #[serde(default)]
    usage_items: Vec<UsageItem>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UsageItem {
    #[serde(default)]
    product: String,
    #[serde(default)]
    sku: String,
    #[serde(default)]
    quantity: f64,
    #[serde(default)]
    unit_type: String,
    #[serde(default)]
    gross_amount: f64,
    #[serde(default)]
    discount_amount: f64,
    #[serde(default)]
    net_amount: f64,
    #[serde(default)]
    repository_name: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct BudgetsResponse {
    #[serde(default)]
    budgets: Vec<Budget>,
    #[serde(default)]
    has_next_page: bool,
}

#[derive(Debug, Clone, Deserialize)]
struct Budget {
    #[serde(default)]
    id: String,
    #[serde(default)]
    budget_type: String,
    #[serde(default)]
    budget_product_skus: Vec<String>,
    #[serde(default)]
    budget_product_sku: Option<String>,
    #[serde(default)]
    budget_scope: String,
    #[serde(default)]
    budget_entity_name: Option<String>,
    #[serde(default)]
    budget_amount: f64,
    #[serde(default)]
    prevent_further_usage: bool,
}

impl Budget {
    fn product_skus(&self) -> impl Iterator<Item = &str> {
        self.budget_product_skus
            .iter()
            .map(String::as_str)
            .chain(self.budget_product_sku.as_deref())
    }

    fn is_actions_budget(&self) -> bool {
        self.product_skus().any(is_actions_token)
    }
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
enum CapacityState {
    Healthy,
    Watch,
    Critical,
    Blocked,
    Unknown,
}

impl CapacityState {
    fn severity(self) -> u8 {
        match self {
            Self::Healthy => 0,
            Self::Watch => 1,
            Self::Critical => 2,
            Self::Blocked => 3,
            Self::Unknown => 4,
        }
    }

    fn raise(self, candidate: Self) -> Self {
        if candidate.severity() > self.severity() {
            candidate
        } else {
            self
        }
    }

    fn exit_code(self) -> ExitCode {
        match self {
            Self::Healthy | Self::Watch => ExitCode::SUCCESS,
            Self::Critical | Self::Blocked => ExitCode::from(1),
            Self::Unknown => ExitCode::from(2),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct UsageTotals {
    actions_minutes: f64,
    gross_amount_usd: f64,
    discount_amount_usd: f64,
    net_amount_usd: f64,
    repositories: BTreeMap<String, RepositoryUsage>,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct RepositoryUsage {
    actions_minutes: f64,
    net_amount_usd: f64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct BudgetEvidence {
    id: String,
    budget_type: String,
    product_skus: Vec<String>,
    scope: String,
    entity_name: Option<String>,
    budget_amount_usd: f64,
    observed_net_amount_usd: f64,
    usage_percent: Option<f64>,
    prevent_further_usage: bool,
    state: CapacityState,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct RoutingPlan {
    mode: String,
    runs_on_json: String,
    variable_visibility: String,
    selected_repository_ids: Vec<u64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct CapacityReport {
    organization: String,
    year: i32,
    month: u8,
    state: CapacityState,
    routing: RoutingPlan,
    included_minutes: Option<f64>,
    minute_usage_percent: Option<f64>,
    usage: UsageTotals,
    budgets: Vec<BudgetEvidence>,
    warnings: Vec<String>,
    mutation_enabled: bool,
    mutations_applied: bool,
}

#[derive(Debug, Clone)]
struct VariableMutation {
    name: &'static str,
    value: String,
}

fn optional_env(key: &str) -> Option<String> {
    env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn required_env(key: &str) -> Result<String, String> {
    optional_env(key).ok_or_else(|| format!("{key} is required"))
}

fn optional_positive_f64(key: &str) -> Result<Option<f64>, String> {
    optional_env(key)
        .map(|value| {
            value
                .parse::<f64>()
                .map_err(|error| format!("invalid {key}: {error}"))
                .and_then(|parsed| {
                    if parsed.is_finite() && parsed > 0.0 {
                        Ok(parsed)
                    } else {
                        Err(format!("{key} must be finite and positive"))
                    }
                })
        })
        .transpose()
}

fn env_bool(key: &str, fallback: bool) -> bool {
    optional_env(key)
        .map(|value| matches!(value.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"))
        .unwrap_or(fallback)
}

fn parse_repository_ids(value: Option<&str>) -> Result<Vec<u64>, String> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let mut ids = Vec::new();
    for raw in value.split(',') {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            continue;
        }
        let id = trimmed
            .parse::<u64>()
            .map_err(|error| format!("invalid repository id {trimmed}: {error}"))?;
        if id == 0 {
            return Err("repository ids must be positive".to_string());
        }
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    Ok(ids)
}

fn validate_organization(value: &str) -> Result<(), String> {
    if value.is_empty() || value.len() > 100 {
        return Err("organization must contain between 1 and 100 characters".to_string());
    }
    let bytes = value.as_bytes();
    if !bytes[0].is_ascii_alphanumeric() || !bytes[bytes.len() - 1].is_ascii_alphanumeric() {
        return Err("organization must start and end with an ASCII letter or digit".to_string());
    }
    if !bytes
        .iter()
        .all(|byte| byte.is_ascii_alphanumeric() || *byte == b'-')
    {
        return Err("organization may contain only ASCII letters, digits, and hyphens".to_string());
    }
    Ok(())
}

fn is_actions_token(value: &str) -> bool {
    let normalized = value.to_ascii_lowercase();
    normalized == "actions"
        || normalized.starts_with("actions_")
        || normalized.starts_with("actions ")
        || normalized.starts_with("github actions")
}

fn is_actions_usage(item: &UsageItem) -> bool {
    is_actions_token(&item.product) || is_actions_token(&item.sku)
}

fn normalize_repository(value: &str) -> String {
    value
        .trim()
        .rsplit('/')
        .next()
        .unwrap_or(value)
        .to_ascii_lowercase()
}

fn summarize_usage(response: &UsageResponse) -> UsageTotals {
    let mut totals = UsageTotals {
        actions_minutes: 0.0,
        gross_amount_usd: 0.0,
        discount_amount_usd: 0.0,
        net_amount_usd: 0.0,
        repositories: BTreeMap::new(),
    };
    for item in response.usage_items.iter().filter(|item| is_actions_usage(item)) {
        let minutes = if item.unit_type.eq_ignore_ascii_case("minutes") {
            item.quantity.max(0.0)
        } else {
            0.0
        };
        totals.actions_minutes += minutes;
        totals.gross_amount_usd += item.gross_amount.max(0.0);
        totals.discount_amount_usd += item.discount_amount.max(0.0);
        totals.net_amount_usd += item.net_amount.max(0.0);
        if let Some(repository) = item.repository_name.as_deref() {
            let entry = totals
                .repositories
                .entry(normalize_repository(repository))
                .or_default();
            entry.actions_minutes += minutes;
            entry.net_amount_usd += item.net_amount.max(0.0);
        }
    }
    totals
}

fn budget_observed_amount(budget: &Budget, usage: &UsageTotals) -> f64 {
    if budget.budget_scope.eq_ignore_ascii_case("repository") {
        let Some(entity) = budget.budget_entity_name.as_deref() else {
            return 0.0;
        };
        return usage
            .repositories
            .get(&normalize_repository(entity))
            .map(|value| value.net_amount_usd)
            .unwrap_or(0.0);
    }
    usage.net_amount_usd
}

fn budget_state(
    budget: &Budget,
    observed: f64,
    warn: f64,
    critical: f64,
) -> (CapacityState, Option<f64>) {
    if budget.budget_amount <= 0.0 {
        return if budget.prevent_further_usage {
            (CapacityState::Blocked, None)
        } else {
            (CapacityState::Critical, None)
        };
    }
    let percent = (observed / budget.budget_amount) * 100.0;
    let state = if percent >= 100.0 && budget.prevent_further_usage {
        CapacityState::Blocked
    } else if percent >= critical {
        CapacityState::Critical
    } else if percent >= warn {
        CapacityState::Watch
    } else {
        CapacityState::Healthy
    };
    (state, Some(percent))
}

fn routing_for_state(state: CapacityState, selected_repository_ids: &[u64]) -> RoutingPlan {
    let (mode, runs_on_json) = match state {
        CapacityState::Healthy | CapacityState::Watch => ("hosted", HOSTED_RUNS_ON_JSON),
        CapacityState::Critical | CapacityState::Blocked => {
            ("self-hosted", SELF_HOSTED_RUNS_ON_JSON)
        }
        CapacityState::Unknown => ("hold", "[]"),
    };
    RoutingPlan {
        mode: mode.to_string(),
        runs_on_json: runs_on_json.to_string(),
        variable_visibility: "selected".to_string(),
        selected_repository_ids: selected_repository_ids.to_vec(),
    }
}

fn evaluate(
    organization: &str,
    year: i32,
    month: u8,
    usage_response: &UsageResponse,
    budgets: &[Budget],
    config: &Config,
) -> CapacityReport {
    let usage = summarize_usage(usage_response);
    let minute_usage_percent = config
        .included_minutes
        .map(|included| (usage.actions_minutes / included) * 100.0);
    let mut warnings = Vec::new();
    let mut state = CapacityState::Healthy;

    if let Some(percent) = minute_usage_percent {
        state = state.raise(if percent >= config.critical_percent {
            CapacityState::Critical
        } else if percent >= config.warn_percent {
            CapacityState::Watch
        } else {
            CapacityState::Healthy
        });
        if percent >= config.warn_percent {
            warnings.push(format!(
                "Actions minutes are at {percent:.1}% of the configured included-minute allowance"
            ));
        }
        if percent >= config.hard_percent {
            warnings.push("Configured included-minute hard threshold has been reached".to_string());
        }
    } else {
        warnings.push(
            "No included-minute allowance is configured; the enhanced billing API does not expose the plan allowance directly"
                .to_string(),
        );
    }

    if usage.net_amount_usd > 0.0 {
        state = state.raise(CapacityState::Watch);
        warnings.push(format!(
            "Actions has a positive current-period net billable amount of ${:.2}",
            usage.net_amount_usd
        ));
    }

    let mut budget_evidence = Vec::new();
    for budget in budgets.iter().filter(|budget| budget.is_actions_budget()) {
        let observed = budget_observed_amount(budget, &usage);
        let (budget_state, usage_percent) = budget_state(
            budget,
            observed,
            config.warn_percent,
            config.critical_percent,
        );
        state = state.raise(budget_state);
        budget_evidence.push(BudgetEvidence {
            id: budget.id.clone(),
            budget_type: budget.budget_type.clone(),
            product_skus: budget.product_skus().map(str::to_string).collect(),
            scope: budget.budget_scope.clone(),
            entity_name: budget.budget_entity_name.clone(),
            budget_amount_usd: budget.budget_amount,
            observed_net_amount_usd: observed,
            usage_percent,
            prevent_further_usage: budget.prevent_further_usage,
            state: budget_state,
        });
    }

    if budget_evidence.is_empty() {
        warnings.push("No Actions spending budget was returned for this organization".to_string());
        if config.included_minutes.is_none() && usage.net_amount_usd <= 0.0 {
            state = CapacityState::Unknown;
        }
    }

    let routing = routing_for_state(state, &config.selected_repository_ids);
    CapacityReport {
        organization: organization.to_string(),
        year,
        month,
        state,
        routing,
        included_minutes: config.included_minutes,
        minute_usage_percent,
        usage,
        budgets: budget_evidence,
        warnings,
        mutation_enabled: config.mutation_enabled,
        mutations_applied: false,
    }
}

fn planned_mutations(report: &CapacityReport) -> Result<Vec<VariableMutation>, String> {
    if report.state == CapacityState::Unknown {
        return Err("routing variables are not mutated while capacity state is unknown".to_string());
    }
    if report.routing.selected_repository_ids.is_empty() {
        return Err("selected repository ids are required for variable mutation".to_string());
    }
    Ok(vec![
        VariableMutation {
            name: "CI_EXECUTION_MODE",
            value: report.routing.mode.clone(),
        },
        VariableMutation {
            name: "CI_LINUX_RUNS_ON_JSON",
            value: report.routing.runs_on_json.clone(),
        },
    ])
}

fn utc_year_month(now: SystemTime) -> Result<(i32, u8), String> {
    let seconds = now
        .duration_since(UNIX_EPOCH)
        .map_err(|error| format!("system clock is before Unix epoch: {error}"))?
        .as_secs();
    let (year, month, _) = civil_from_days((seconds / 86_400) as i64);
    Ok((year, month))
}

fn civil_from_days(days_since_epoch: i64) -> (i32, u8, u8) {
    let z = days_since_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let day_of_era = z - era * 146_097;
    let year_of_era = (day_of_era - day_of_era / 1_460 + day_of_era / 36_524
        - day_of_era / 146_096)
        / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    (year as i32, month as u8, day as u8)
}

fn http_client() -> Result<Client, String> {
    Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .redirect(RedirectPolicy::none())
        .user_agent(USER_AGENT)
        .build()
        .map_err(|error| format!("failed to create HTTP client: {error}"))
}

async fn response_json<T: DeserializeOwned>(response: reqwest::Response) -> Result<T, String> {
    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|error| format!("failed to read GitHub API response: {error}"))?;
    if !status.is_success() {
        let compact: String = body.split_whitespace().collect::<Vec<_>>().join(" ");
        return Err(format!(
            "GitHub API returned {status}: {}",
            compact.chars().take(500).collect::<String>()
        ));
    }
    serde_json::from_str(&body)
        .map_err(|error| format!("invalid GitHub API JSON response: {error}"))
}

async fn fetch_usage(
    client: &Client,
    config: &Config,
    year: i32,
    month: u8,
) -> Result<UsageResponse, String> {
    let url = format!(
        "{API_BASE}/organizations/{}/settings/billing/usage",
        config.organization
    );
    let response = client
        .get(url)
        .query(&[("year", year.to_string()), ("month", month.to_string())])
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", API_VERSION)
        .bearer_auth(&config.token)
        .send()
        .await
        .map_err(|error| format!("GitHub usage request failed: {error}"))?;
    response_json(response).await
}

async fn fetch_budgets(client: &Client, config: &Config) -> Result<Vec<Budget>, String> {
    let mut page = 1_u32;
    let mut budgets = Vec::new();
    loop {
        let url = format!(
            "{API_BASE}/organizations/{}/settings/billing/budgets",
            config.organization
        );
        let page_value = page.to_string();
        let response = client
            .get(url)
            .query(&[("per_page", "100"), ("page", page_value.as_str())])
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", API_VERSION)
            .bearer_auth(&config.token)
            .send()
            .await
            .map_err(|error| format!("GitHub budgets request failed: {error}"))?;
        let result: BudgetsResponse = response_json(response).await?;
        budgets.extend(result.budgets);
        if !result.has_next_page {
            break;
        }
        page = page
            .checked_add(1)
            .ok_or_else(|| "budget pagination overflow".to_string())?;
        if page > 100 {
            return Err("budget pagination exceeded 100 pages".to_string());
        }
    }
    Ok(budgets)
}

async fn upsert_variable(
    client: &Client,
    config: &Config,
    mutation: &VariableMutation,
) -> Result<(), String> {
    let body = json!({
        "name": mutation.name,
        "value": &mutation.value,
        "visibility": "selected",
        "selected_repository_ids": &config.selected_repository_ids,
    });
    let update_url = format!(
        "{API_BASE}/orgs/{}/actions/variables/{}",
        config.organization, mutation.name
    );
    let update = client
        .patch(update_url)
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", API_VERSION)
        .bearer_auth(&config.token)
        .json(&body)
        .send()
        .await
        .map_err(|error| format!("GitHub variable update failed: {error}"))?;
    if update.status().is_success() {
        return Ok(());
    }
    if update.status() != StatusCode::NOT_FOUND {
        return response_json::<serde_json::Value>(update)
            .await
            .map(|_| ())
            .map_err(|error| format!("variable {} update failed: {error}", mutation.name));
    }

    let create_url = format!(
        "{API_BASE}/orgs/{}/actions/variables",
        config.organization
    );
    let create = client
        .post(create_url)
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", API_VERSION)
        .bearer_auth(&config.token)
        .json(&body)
        .send()
        .await
        .map_err(|error| format!("GitHub variable create failed: {error}"))?;
    if create.status().is_success() {
        return Ok(());
    }
    response_json::<serde_json::Value>(create)
        .await
        .map(|_| ())
        .map_err(|error| format!("variable {} create failed: {error}", mutation.name))
}

async fn run() -> Result<CapacityReport, String> {
    let config = Config::from_env()?;
    let client = http_client()?;
    let (year, month) = utc_year_month(SystemTime::now())?;
    let usage = fetch_usage(&client, &config, year, month).await?;
    let budgets = fetch_budgets(&client, &config).await?;
    let mut report = evaluate(
        &config.organization,
        year,
        month,
        &usage,
        &budgets,
        &config,
    );
    if config.mutation_enabled {
        for mutation in planned_mutations(&report)? {
            upsert_variable(&client, &config, &mutation).await?;
        }
        report.mutations_applied = true;
    }
    Ok(report)
}

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(report) => {
            let code = report.state.exit_code();
            match serde_json::to_string_pretty(&report) {
                Ok(body) => println!("{body}"),
                Err(error) => {
                    eprintln!("failed to serialize capacity report: {error}");
                    return ExitCode::from(2);
                }
            }
            code
        }
        Err(error) => {
            eprintln!("gha-capacity-audit failed: {error}");
            ExitCode::from(2)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(included_minutes: Option<f64>) -> Config {
        Config {
            organization: "sonus-auris".to_string(),
            token: "not-used-in-unit-tests".to_string(),
            included_minutes,
            warn_percent: 75.0,
            critical_percent: 90.0,
            hard_percent: 100.0,
            mutation_enabled: false,
            selected_repository_ids: vec![1_294_558_398],
        }
    }

    fn usage_item(minutes: f64, net_amount: f64) -> UsageItem {
        UsageItem {
            product: "Actions".to_string(),
            sku: "Actions Linux".to_string(),
            quantity: minutes,
            unit_type: "minutes".to_string(),
            gross_amount: net_amount,
            discount_amount: 0.0,
            net_amount,
            repository_name: Some("sonus-auris/sonus-auris-monorepo".to_string()),
        }
    }

    fn usage(minutes: f64, net_amount: f64) -> UsageResponse {
        UsageResponse {
            usage_items: vec![usage_item(minutes, net_amount)],
        }
    }

    fn actions_budget(amount: f64, prevent: bool) -> Budget {
        Budget {
            id: "budget-1".to_string(),
            budget_type: "ProductPricing".to_string(),
            budget_product_skus: vec!["actions".to_string()],
            budget_product_sku: None,
            budget_scope: "organization".to_string(),
            budget_entity_name: None,
            budget_amount: amount,
            prevent_further_usage: prevent,
        }
    }

    #[test]
    fn validates_org_names() {
        assert!(validate_organization("sonus-auris").is_ok());
        assert!(validate_organization("-sonus").is_err());
        assert!(validate_organization("sonus/auris").is_err());
    }

    #[test]
    fn sums_only_actions_minutes_and_money() {
        let response = UsageResponse {
            usage_items: vec![
                usage_item(100.0, 0.8),
                UsageItem {
                    product: "Packages".to_string(),
                    sku: "storage".to_string(),
                    quantity: 99.0,
                    unit_type: "gigabytes".to_string(),
                    gross_amount: 9.0,
                    discount_amount: 0.0,
                    net_amount: 9.0,
                    repository_name: None,
                },
            ],
        };
        let totals = summarize_usage(&response);
        assert_eq!(totals.actions_minutes, 100.0);
        assert_eq!(totals.net_amount_usd, 0.8);
        assert_eq!(
            totals.repositories["sonus-auris-monorepo"].actions_minutes,
            100.0
        );
    }

    #[test]
    fn routes_to_self_hosted_at_ninety_percent() {
        let report = evaluate(
            "sonus-auris",
            2026,
            8,
            &usage(1_800.0, 0.0),
            &[actions_budget(100.0, true)],
            &config(Some(2_000.0)),
        );
        assert_eq!(report.state, CapacityState::Critical);
        assert_eq!(report.routing.mode, "self-hosted");
        assert_eq!(report.routing.runs_on_json, SELF_HOSTED_RUNS_ON_JSON);
    }

    #[test]
    fn zero_blocking_budget_is_blocked() {
        let report = evaluate(
            "sonus-auris",
            2026,
            8,
            &usage(0.0, 0.0),
            &[actions_budget(0.0, true)],
            &config(Some(2_000.0)),
        );
        assert_eq!(report.state, CapacityState::Blocked);
    }

    #[test]
    fn repository_budget_uses_repository_amount() {
        let mut budget = actions_budget(10.0, true);
        budget.budget_scope = "repository".to_string();
        budget.budget_entity_name = Some("sonus-auris-monorepo".to_string());
        let report = evaluate(
            "sonus-auris",
            2026,
            8,
            &usage(100.0, 9.5),
            &[budget],
            &config(Some(2_000.0)),
        );
        assert_eq!(report.state, CapacityState::Critical);
        assert_eq!(report.budgets[0].usage_percent, Some(95.0));
    }

    #[test]
    fn missing_allowance_and_budget_is_unknown() {
        let report = evaluate(
            "sonus-auris",
            2026,
            8,
            &usage(0.0, 0.0),
            &[],
            &config(None),
        );
        assert_eq!(report.state, CapacityState::Unknown);
        assert_eq!(report.routing.mode, "hold");
        assert!(planned_mutations(&report).is_err());
    }

    #[test]
    fn mutations_are_selected_repository_only() {
        let report = evaluate(
            "sonus-auris",
            2026,
            8,
            &usage(2_000.0, 0.0),
            &[actions_budget(100.0, true)],
            &config(Some(2_000.0)),
        );
        let mutations = planned_mutations(&report).expect("mutation plan");
        assert_eq!(mutations.len(), 2);
        assert_eq!(mutations[0].name, "CI_EXECUTION_MODE");
        assert_eq!(mutations[1].name, "CI_LINUX_RUNS_ON_JSON");
    }

    #[test]
    fn deserializes_current_usage_api_shape() {
        let response: UsageResponse = serde_json::from_str(
            r#"{
              "usageItems": [
                {
                  "product": "Actions",
                  "sku": "Actions Linux",
                  "quantity": 100,
                  "unitType": "minutes",
                  "grossAmount": 0.8,
                  "discountAmount": 0,
                  "netAmount": 0.8,
                  "repositoryName": "sonus-auris/sonus-auris-monorepo"
                }
              ]
            }"#,
        )
        .expect("usage response");
        assert_eq!(response.usage_items.len(), 1);
        assert_eq!(response.usage_items[0].quantity, 100.0);
    }

    #[test]
    fn deserializes_current_budget_api_shape() {
        let response: BudgetsResponse = serde_json::from_str(
            r#"{
              "budgets": [
                {
                  "id": "budget-1",
                  "budget_type": "ProductPricing",
                  "budget_product_skus": ["actions"],
                  "budget_scope": "organization",
                  "budget_amount": 100,
                  "prevent_further_usage": true
                }
              ],
              "has_next_page": false,
              "total_count": 1
            }"#,
        )
        .expect("budget response");
        assert_eq!(response.budgets.len(), 1);
        assert!(response.budgets[0].is_actions_budget());
        assert!(!response.has_next_page);
    }

    #[test]
    fn converts_unix_days_to_civil_date() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(10_957), (2000, 1, 1));
        assert_eq!(civil_from_days(20_668), (2026, 8, 3));
    }
}
