use std::{
    collections::BTreeSet,
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
    extract::{DefaultBodyLimit, Form, Path, State},
    http::{header, HeaderMap, HeaderName, HeaderValue, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tower_http::{
    limit::RequestBodyLimitLayer, set_header::SetResponseHeaderLayer, timeout::TimeoutLayer,
    trace::TraceLayer,
};
use tracing::{error, info};

const SERVICE_NAME: &str = "dd-patent-filing-rs";
const SCHEMA_VERSION: &str = "patent_filing.package.v1";
const MAX_HTTP_BODY_BYTES: usize = 1024 * 1024;
const MAX_MATTERS_DEFAULT: usize = 200;
const MAX_TEXT_LEN: usize = 24_000;
const MAX_SHORT_TEXT_LEN: usize = 1_000;
const MAX_LIST_ITEMS: usize = 64;
const MAX_TOKEN_LEN: usize = 160;
const MAX_CLAIMS: usize = 200;
const ABSTRACT_WORD_LIMIT: usize = 150;
const REQUEST_TIMEOUT_SECS: u64 = 15;
/// AI drafting can take much longer than the deterministic endpoints (model
/// thinking + generation), so it gets its own request + HTTP timeouts.
const AI_REQUEST_TIMEOUT_SECS: u64 = 150;
const AI_HTTP_TIMEOUT_SECS: u64 = 140;
const AI_MAX_TOKENS: u32 = 12_000;
const ANTHROPIC_VERSION: &str = "2023-06-01";
/// Upper bound on the prompt brief sent to the model, so a large intake cannot
/// amplify into unbounded model cost.
const AI_BRIEF_MAX_CHARS: usize = 20_000;
/// Cap on upstream error text echoed back to clients.
const AI_ERROR_SNIPPET_CHARS: usize = 500;
/// USPTO fee schedule effective date encoded in [`fee_schedule`].
const FEE_EFFECTIVE_DATE: &str = "2025-01-19";
/// Pinned htmx asset + Subresource Integrity hash (supply-chain hardening).
const HTMX_SRC: &str = "https://unpkg.com/htmx.org@1.9.12/dist/htmx.min.js";
const HTMX_SRI: &str = "sha384-ujb1lZYygJmzgSwoxRggbCHcjc0rB2XoQrxeTUQyRjrOnlCoYta87iKBWq3EsdM2";

#[derive(Clone)]
struct AppState {
    config: Arc<Config>,
    metrics: Arc<Metrics>,
    store: Arc<RwLock<PatentStore>>,
    http: reqwest::Client,
    ai_permits: Arc<tokio::sync::Semaphore>,
}

#[derive(Clone)]
struct Config {
    server_auth_secret: Option<String>,
    allow_unauthenticated: bool,
    patent_center_url: String,
    max_matters: usize,
    anthropic_api_key: Option<String>,
    anthropic_base_url: String,
    ai_model: String,
    ai_max_concurrency: usize,
}

#[derive(Default)]
struct Metrics {
    http_requests_total: AtomicU64,
    package_requests_total: AtomicU64,
    readiness_requests_total: AtomicU64,
    search_plan_requests_total: AtomicU64,
    package_reviews_total: AtomicU64,
    claim_checks_total: AtomicU64,
    fee_estimates_total: AtomicU64,
    deadline_requests_total: AtomicU64,
    ai_drafts_total: AtomicU64,
    ai_draft_errors_total: AtomicU64,
    ai_throttled_total: AtomicU64,
    auth_failures_total: AtomicU64,
    errors_total: AtomicU64,
}

#[derive(Default)]
struct PatentStore {
    matters: Vec<PatentMatterPackage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PatentIntakeRequest {
    request_id: Option<String>,
    schema_version: Option<String>,
    #[serde(default)]
    title: String,
    #[serde(default)]
    inventor_names: Vec<String>,
    applicant: Option<String>,
    #[serde(default)]
    invention_summary: String,
    #[serde(default)]
    technical_field: String,
    #[serde(default)]
    problem: String,
    #[serde(default)]
    solution: String,
    #[serde(default)]
    novelty_claims: Vec<String>,
    #[serde(default)]
    embodiments: Vec<String>,
    #[serde(default)]
    alternatives: Vec<String>,
    #[serde(default)]
    advantages: Vec<String>,
    public_disclosure_date: Option<String>,
    provisional_filing_date: Option<String>,
    foreign_priority_date: Option<String>,
    target_filing: Option<String>,
    entity_status: Option<String>,
    desired_claim_count: Option<usize>,
    attorney_review: Option<bool>,
    #[serde(default)]
    known_prior_art: Vec<KnownPriorArt>,
    #[serde(default)]
    attachments: Vec<AttachmentEvidence>,
    notes: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct KnownPriorArt {
    title: String,
    url: Option<String>,
    notes: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AttachmentEvidence {
    name: String,
    kind: Option<String>,
    url: Option<String>,
    notes: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct PatentMatterPackage {
    ok: bool,
    matter_id: String,
    request_id: String,
    schema_version: &'static str,
    generated_at_ms: u128,
    filing_track: String,
    title: String,
    applicant: Option<String>,
    inventor_names: Vec<String>,
    readiness: ReadinessReview,
    draft: ProvisionalDraft,
    search_plan: SearchPlan,
    claim_audit: ClaimAudit,
    fee_estimate: FeeEstimate,
    deadlines: DeadlineReport,
    filing_checklist: Vec<ChecklistItem>,
    attorney_handoff: AttorneyHandoff,
    warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PackageReviewRequest {
    matter_id: Option<String>,
    package: Option<PatentMatterPackageInput>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PatentMatterPackageInput {
    readiness_score: Option<u8>,
    blocker_count: Option<usize>,
    section_count: Option<usize>,
    checklist_open_count: Option<usize>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct PackageReviewResponse {
    ok: bool,
    status: String,
    release_gate: String,
    findings: Vec<FilingFinding>,
    next_actions: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ReadinessReview {
    score: u8,
    status: String,
    blockers: Vec<FilingFinding>,
    warnings: Vec<FilingFinding>,
    strengths: Vec<String>,
    next_actions: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct FilingFinding {
    code: String,
    severity: String,
    message: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProvisionalDraft {
    title: String,
    abstract_draft: String,
    sections: Vec<DraftSection>,
    claim_seeds: Vec<String>,
    drawing_plan: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DraftSection {
    heading: String,
    body: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SearchPlan {
    queries: Vec<SearchQuery>,
    classification_hints: Vec<String>,
    sources: Vec<SearchSource>,
    review_notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SearchQuery {
    label: String,
    query: String,
    intent: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SearchSource {
    name: String,
    url: String,
    use_case: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ChecklistItem {
    label: String,
    status: String,
    owner: String,
    notes: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct AttorneyHandoff {
    summary: String,
    questions: Vec<String>,
    package_manifest: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct UiPackageForm {
    title: String,
    inventor_names: String,
    applicant: Option<String>,
    technical_field: Option<String>,
    invention_summary: String,
    problem: String,
    solution: String,
    novelty_claims: String,
    embodiments: Option<String>,
    alternatives: Option<String>,
    advantages: Option<String>,
    known_prior_art: Option<String>,
    attachments: Option<String>,
    public_disclosure_date: Option<String>,
    provisional_filing_date: Option<String>,
    foreign_priority_date: Option<String>,
    target_filing: Option<String>,
    entity_status: Option<String>,
    attorney_review: Option<String>,
}

enum AuthFailure {
    MissingSecret,
    Unauthorized,
}

fn optional_env(key: &str) -> Option<String> {
    env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn env_value(key: &str, fallback: &str) -> String {
    optional_env(key).unwrap_or_else(|| fallback.to_string())
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

fn env_usize(key: &str, fallback: usize) -> usize {
    env::var(key)
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(fallback)
}

fn config_from_env() -> Config {
    Config {
        server_auth_secret: optional_env("PATENT_FILING_SERVER_AUTH_SECRET")
            .or_else(|| optional_env("SERVER_AUTH_SECRET")),
        allow_unauthenticated: env_bool("PATENT_FILING_ALLOW_UNAUTHENTICATED", false),
        patent_center_url: env_value(
            "PATENT_FILING_CENTER_URL",
            "https://patentcenter.uspto.gov/",
        ),
        max_matters: env_usize("PATENT_FILING_MAX_MATTERS", MAX_MATTERS_DEFAULT),
        anthropic_api_key: optional_env("PATENT_FILING_ANTHROPIC_API_KEY")
            .or_else(|| optional_env("ANTHROPIC_API_KEY")),
        anthropic_base_url: env_value("PATENT_FILING_ANTHROPIC_BASE_URL", "https://api.anthropic.com"),
        ai_model: env_value("PATENT_FILING_AI_MODEL", "claude-opus-4-8"),
        ai_max_concurrency: env_usize("PATENT_FILING_AI_MAX_CONCURRENCY", 4),
    }
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn request_id(input: Option<&String>, fallback: &str) -> String {
    input
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .unwrap_or(fallback)
        .chars()
        .take(MAX_TOKEN_LEN)
        .collect()
}

fn clean_text(value: &str, max_len: usize) -> String {
    value
        .trim()
        .chars()
        .filter(|ch| !ch.is_control() || *ch == '\n' || *ch == '\t')
        .take(max_len)
        .collect()
}

fn clean_optional(value: Option<String>, max_len: usize) -> Option<String> {
    value
        .map(|item| clean_text(&item, max_len))
        .filter(|item| !item.is_empty())
}

fn split_lines(value: &str) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut items = Vec::new();
    for raw in value.lines().flat_map(|line| line.split(';')) {
        let item = clean_text(raw, MAX_SHORT_TEXT_LEN);
        if !item.is_empty() && seen.insert(item.to_ascii_lowercase()) {
            items.push(item);
        }
        if items.len() >= MAX_LIST_ITEMS {
            break;
        }
    }
    items
}

fn slugify(value: &str) -> String {
    let slug = value
        .trim()
        .to_ascii_lowercase()
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    if slug.is_empty() {
        "untitled".to_string()
    } else {
        slug.chars().take(48).collect()
    }
}

fn normalize_track(value: Option<&String>) -> String {
    let label = value
        .map(|item| item.trim().to_ascii_lowercase())
        .unwrap_or_else(|| "provisional".to_string());
    match label.as_str() {
        "provisional" | "non-provisional" | "design" | "pct" => label,
        _ => "provisional".to_string(),
    }
}

fn require_auth(headers: &HeaderMap, state: &AppState) -> Result<(), AuthFailure> {
    if state.config.allow_unauthenticated {
        return Ok(());
    }
    let Some(secret) = state.config.server_auth_secret.as_ref() else {
        return Err(AuthFailure::MissingSecret);
    };
    let provided = headers
        .get("x-server-auth")
        .or_else(|| headers.get("auth"))
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    if constant_time_eq(provided, secret) {
        Ok(())
    } else {
        Err(AuthFailure::Unauthorized)
    }
}

fn constant_time_eq(left: &str, right: &str) -> bool {
    let left = left.as_bytes();
    let right = right.as_bytes();
    if left.len() != right.len() {
        return false;
    }
    let mut diff = 0u8;
    for (a, b) in left.iter().zip(right.iter()) {
        diff |= a ^ b;
    }
    diff == 0
}

fn auth_failure_response(state: &AppState, failure: AuthFailure) -> Response {
    state
        .metrics
        .auth_failures_total
        .fetch_add(1, Ordering::Relaxed);
    let (status, message) = match failure {
        AuthFailure::MissingSecret => (
            StatusCode::SERVICE_UNAVAILABLE,
            "server auth secret is not configured",
        ),
        AuthFailure::Unauthorized => (StatusCode::UNAUTHORIZED, "unauthorized"),
    };
    (status, Json(json!({ "ok": false, "error": message }))).into_response()
}

fn ui_auth_failure_response(state: &AppState, failure: AuthFailure) -> Response {
    state
        .metrics
        .auth_failures_total
        .fetch_add(1, Ordering::Relaxed);
    let message = match failure {
        AuthFailure::MissingSecret => {
            "SERVER_AUTH_SECRET is not configured for package generation."
        }
        AuthFailure::Unauthorized => "Package generation is waiting for operator authentication.",
    };
    (
        StatusCode::UNAUTHORIZED,
        Html(format!(
            r#"<div class="result error"><strong>Auth required</strong><p>{}</p></div>"#,
            escape_html(message)
        )),
    )
        .into_response()
}

fn intake_from_form(form: UiPackageForm) -> PatentIntakeRequest {
    let known_prior_art = split_lines(&form.known_prior_art.unwrap_or_default())
        .into_iter()
        .map(|line| KnownPriorArt {
            title: line,
            url: None,
            notes: None,
        })
        .collect();
    let attachments = split_lines(&form.attachments.unwrap_or_default())
        .into_iter()
        .map(|line| AttachmentEvidence {
            name: line,
            kind: Some("figure-or-evidence".to_string()),
            url: None,
            notes: None,
        })
        .collect();

    PatentIntakeRequest {
        request_id: None,
        schema_version: Some(SCHEMA_VERSION.to_string()),
        title: clean_text(&form.title, MAX_SHORT_TEXT_LEN),
        inventor_names: split_lines(&form.inventor_names),
        applicant: clean_optional(form.applicant, MAX_SHORT_TEXT_LEN),
        invention_summary: clean_text(&form.invention_summary, MAX_TEXT_LEN),
        technical_field: clean_text(
            &form.technical_field.unwrap_or_default(),
            MAX_SHORT_TEXT_LEN,
        ),
        problem: clean_text(&form.problem, MAX_TEXT_LEN),
        solution: clean_text(&form.solution, MAX_TEXT_LEN),
        novelty_claims: split_lines(&form.novelty_claims),
        embodiments: split_lines(&form.embodiments.unwrap_or_default()),
        alternatives: split_lines(&form.alternatives.unwrap_or_default()),
        advantages: split_lines(&form.advantages.unwrap_or_default()),
        public_disclosure_date: clean_optional(form.public_disclosure_date, 64),
        provisional_filing_date: clean_optional(form.provisional_filing_date, 64),
        foreign_priority_date: clean_optional(form.foreign_priority_date, 64),
        target_filing: clean_optional(form.target_filing, 64),
        entity_status: clean_optional(form.entity_status, 32),
        desired_claim_count: Some(8),
        attorney_review: Some(form.attorney_review.is_some()),
        known_prior_art,
        attachments,
        notes: None,
    }
}

fn validate_intake(request: &PatentIntakeRequest) -> Result<(), String> {
    for (label, value) in [
        ("title", request.title.as_str()),
        ("inventionSummary", request.invention_summary.as_str()),
        ("problem", request.problem.as_str()),
        ("solution", request.solution.as_str()),
    ] {
        if value.trim().is_empty() {
            return Err(format!("{label} must not be empty"));
        }
    }
    if request.title.len() > MAX_SHORT_TEXT_LEN {
        return Err(format!("title must be at most {MAX_SHORT_TEXT_LEN} bytes"));
    }
    if request.invention_summary.len() > MAX_TEXT_LEN
        || request.problem.len() > MAX_TEXT_LEN
        || request.solution.len() > MAX_TEXT_LEN
    {
        return Err(format!(
            "long text fields must be at most {MAX_TEXT_LEN} bytes"
        ));
    }
    if request.inventor_names.len() > MAX_LIST_ITEMS
        || request.novelty_claims.len() > MAX_LIST_ITEMS
        || request.embodiments.len() > MAX_LIST_ITEMS
        || request.alternatives.len() > MAX_LIST_ITEMS
        || request.advantages.len() > MAX_LIST_ITEMS
        || request.known_prior_art.len() > MAX_LIST_ITEMS
        || request.attachments.len() > MAX_LIST_ITEMS
    {
        return Err(format!(
            "list fields may contain at most {MAX_LIST_ITEMS} items"
        ));
    }
    // Cap individual list-item lengths. The form path already does this via
    // clean_text; the JSON path must enforce it too so a single oversized item
    // cannot bloat a stored package or amplify the AI prompt.
    let short_lists = [
        ("inventorNames", &request.inventor_names),
        ("noveltyClaims", &request.novelty_claims),
        ("embodiments", &request.embodiments),
        ("alternatives", &request.alternatives),
        ("advantages", &request.advantages),
    ];
    for (label, items) in short_lists {
        if items.iter().any(|item| item.len() > MAX_SHORT_TEXT_LEN) {
            return Err(format!(
                "each {label} item must be at most {MAX_SHORT_TEXT_LEN} bytes"
            ));
        }
    }
    for art in &request.known_prior_art {
        if art.title.len() > MAX_SHORT_TEXT_LEN
            || art.url.as_ref().is_some_and(|v| v.len() > MAX_SHORT_TEXT_LEN)
            || art.notes.as_ref().is_some_and(|v| v.len() > MAX_TEXT_LEN)
        {
            return Err("knownPriorArt entries are too long".to_string());
        }
    }
    for attachment in &request.attachments {
        if attachment.name.len() > MAX_SHORT_TEXT_LEN
            || attachment.url.as_ref().is_some_and(|v| v.len() > MAX_SHORT_TEXT_LEN)
            || attachment.notes.as_ref().is_some_and(|v| v.len() > MAX_TEXT_LEN)
        {
            return Err("attachment entries are too long".to_string());
        }
    }
    Ok(())
}

fn evaluate_readiness(request: &PatentIntakeRequest) -> ReadinessReview {
    let mut score = 0u16;
    let mut blockers = Vec::new();
    let mut warnings = Vec::new();
    let mut strengths = Vec::new();
    let mut next_actions = Vec::new();

    if request.title.trim().len() >= 8 {
        score += 8;
    } else {
        blockers.push(finding(
            "missing-title",
            "blocker",
            "Add a descriptive invention title.",
        ));
    }

    if !request.inventor_names.is_empty() {
        score += 8;
        strengths.push(format!(
            "{} inventor(s) captured",
            request.inventor_names.len()
        ));
    } else {
        blockers.push(finding(
            "missing-inventors",
            "blocker",
            "Capture every likely inventor before filing.",
        ));
        next_actions.push("Confirm inventorship with counsel.".to_string());
    }

    if request.invention_summary.chars().count() >= 120 {
        score += 14;
        strengths.push("Invention summary is long enough for drafting context.".to_string());
    } else {
        blockers.push(finding(
            "thin-summary",
            "blocker",
            "Expand the invention summary with structure, operation, and use cases.",
        ));
        next_actions.push("Add concrete component and workflow detail.".to_string());
    }

    if request.problem.chars().count() >= 40 {
        score += 8;
    } else {
        warnings.push(finding(
            "thin-problem",
            "warning",
            "The problem statement is too short for a strong background section.",
        ));
    }

    if request.solution.chars().count() >= 60 {
        score += 12;
    } else {
        blockers.push(finding(
            "thin-solution",
            "blocker",
            "Describe the solution in enough detail to support enablement review.",
        ));
    }

    if request.novelty_claims.len() >= 2 {
        score += 16;
        strengths.push(format!(
            "{} novelty points captured",
            request.novelty_claims.len()
        ));
    } else if request.novelty_claims.len() == 1 {
        score += 8;
        warnings.push(finding(
            "single-novelty-point",
            "warning",
            "Only one novelty point is captured; add alternatives or dependent features.",
        ));
    } else {
        blockers.push(finding(
            "missing-novelty",
            "blocker",
            "List the technical features believed to be new.",
        ));
    }

    if !request.technical_field.trim().is_empty() {
        score += 5;
    } else {
        warnings.push(finding(
            "missing-technical-field",
            "warning",
            "Add a technical field to focus searching and drafting.",
        ));
    }

    if !request.embodiments.is_empty() {
        score += 8;
    } else {
        warnings.push(finding(
            "missing-embodiments",
            "warning",
            "Add at least one implementation embodiment.",
        ));
    }

    if !request.alternatives.is_empty() {
        score += 5;
    } else {
        next_actions
            .push("Capture alternate implementations to avoid a narrow disclosure.".to_string());
    }

    if !request.advantages.is_empty() {
        score += 5;
    }

    if !request.attachments.is_empty() {
        score += 6;
        strengths.push(format!(
            "{} figure/evidence attachment(s) listed",
            request.attachments.len()
        ));
    } else {
        warnings.push(finding(
            "missing-figures",
            "warning",
            "Prepare at least a system diagram or method flow drawing.",
        ));
    }

    if !request.known_prior_art.is_empty() {
        score += 5;
        strengths.push("Known prior art captured for attorney review.".to_string());
    } else {
        warnings.push(finding(
            "no-known-prior-art",
            "warning",
            "No known prior art was provided; run a search before final filing decisions.",
        ));
    }

    if request.public_disclosure_date.is_some() {
        warnings.push(finding(
            "public-disclosure-date",
            "warning",
            "A public disclosure date is present; review filing deadlines and non-US rights.",
        ));
        next_actions.push("Have counsel evaluate public disclosure timing.".to_string());
    }

    if request.attorney_review.unwrap_or(false) {
        score += 3;
    } else {
        next_actions.push("Route the draft package to patent counsel before filing.".to_string());
    }

    let score = score.min(100) as u8;
    let status = if !blockers.is_empty() {
        "needs-invention-detail"
    } else if score >= 82 {
        "ready-for-attorney-review"
    } else if score >= 65 {
        "draftable"
    } else {
        "needs-invention-detail"
    }
    .to_string();

    if next_actions.is_empty() {
        next_actions.push(
            "Review draft claims, figures, and prior-art search results with counsel.".to_string(),
        );
    }

    ReadinessReview {
        score,
        status,
        blockers,
        warnings,
        strengths,
        next_actions,
    }
}

fn finding(code: &str, severity: &str, message: &str) -> FilingFinding {
    FilingFinding {
        code: code.to_string(),
        severity: severity.to_string(),
        message: message.to_string(),
    }
}

fn build_draft(request: &PatentIntakeRequest) -> ProvisionalDraft {
    let field = if request.technical_field.trim().is_empty() {
        "the relevant technical field".to_string()
    } else {
        request.technical_field.trim().to_string()
    };
    let embodiments = if request.embodiments.is_empty() {
        "At least one implementation should describe the components, data flow, control flow, and operating environment in enough detail for a skilled person to reproduce the invention.".to_string()
    } else {
        request
            .embodiments
            .iter()
            .enumerate()
            .map(|(index, item)| format!("Embodiment {}: {}", index + 1, item))
            .collect::<Vec<_>>()
            .join("\n\n")
    };
    let alternatives = if request.alternatives.is_empty() {
        "Alternative implementations may vary component placement, sequence of operations, data structures, materials, integration surfaces, or user interaction while preserving the inventive concept.".to_string()
    } else {
        request
            .alternatives
            .iter()
            .map(|item| format!("- {item}"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let advantages = if request.advantages.is_empty() {
        "Potential advantages should be validated against prior systems and quantified where possible.".to_string()
    } else {
        request
            .advantages
            .iter()
            .map(|item| format!("- {item}"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let novelty = if request.novelty_claims.is_empty() {
        "Novel technical features remain to be identified.".to_string()
    } else {
        request
            .novelty_claims
            .iter()
            .map(|item| format!("- {item}"))
            .collect::<Vec<_>>()
            .join("\n")
    };

    ProvisionalDraft {
        title: request.title.clone(),
        abstract_draft: abstract_draft(request),
        sections: vec![
            DraftSection {
                heading: "Technical Field".to_string(),
                body: format!("The disclosure relates to {field}."),
            },
            DraftSection {
                heading: "Background".to_string(),
                body: request.problem.clone(),
            },
            DraftSection {
                heading: "Summary".to_string(),
                body: format!(
                    "{}\n\nThe proposed solution includes: {}",
                    request.invention_summary, request.solution
                ),
            },
            DraftSection {
                heading: "Potentially Novel Features".to_string(),
                body: novelty,
            },
            DraftSection {
                heading: "Detailed Description".to_string(),
                body: embodiments,
            },
            DraftSection {
                heading: "Alternative Implementations".to_string(),
                body: alternatives,
            },
            DraftSection {
                heading: "Advantages".to_string(),
                body: advantages,
            },
        ],
        claim_seeds: claim_seeds(request),
        drawing_plan: drawing_plan(request),
    }
}

fn abstract_draft(request: &PatentIntakeRequest) -> String {
    let mut parts = vec![request.invention_summary.trim().to_string()];
    if !request.solution.trim().is_empty() {
        parts.push(format!(
            "The invention addresses the problem by {}.",
            sentence_fragment(&request.solution)
        ));
    }
    if let Some(first) = request.novelty_claims.first() {
        parts.push(format!(
            "In some implementations, the system includes {}.",
            sentence_fragment(first)
        ));
    }
    parts
        .join(" ")
        .split_whitespace()
        .take(120)
        .collect::<Vec<_>>()
        .join(" ")
}

fn sentence_fragment(value: &str) -> String {
    value.trim().trim_end_matches(['.', ';', ':']).to_string()
}

fn claim_seeds(request: &PatentIntakeRequest) -> Vec<String> {
    let mut seeds = Vec::new();
    let noun = if request.title.trim().is_empty() {
        "invention"
    } else {
        request.title.trim()
    };
    let count = request.desired_claim_count.unwrap_or(8).clamp(3, 20);
    seeds.push(format!(
        "A {noun} comprising elements configured to perform the solution described in the specification."
    ));
    for novelty in request.novelty_claims.iter().take(count.saturating_sub(1)) {
        seeds.push(format!(
            "The {noun} of claim 1, wherein {}.",
            sentence_fragment(novelty)
        ));
    }
    if !request.alternatives.is_empty() && seeds.len() < count {
        seeds.push(format!(
            "The {noun} of claim 1, wherein the same result is achieved through at least one disclosed alternative implementation."
        ));
    }
    while seeds.len() < count.min(5) {
        seeds.push(format!(
            "A method of using the {noun} to solve the identified technical problem."
        ));
    }
    seeds
}

fn drawing_plan(request: &PatentIntakeRequest) -> Vec<String> {
    let mut plan = vec![
        "Figure 1: system or architecture overview showing major components and interfaces."
            .to_string(),
        "Figure 2: method flow showing the primary operating sequence.".to_string(),
    ];
    for (index, embodiment) in request.embodiments.iter().take(4).enumerate() {
        plan.push(format!(
            "Figure {}: embodiment detail for {}.",
            index + 3,
            sentence_fragment(embodiment)
        ));
    }
    if request.attachments.is_empty() {
        plan.push(
            "Evidence needed: sketches, screenshots, CAD, data-flow diagrams, or lab notes."
                .to_string(),
        );
    } else {
        for attachment in request.attachments.iter().take(6) {
            plan.push(format!("Existing evidence: {}", attachment.name));
        }
    }
    plan
}

fn build_search_plan(request: &PatentIntakeRequest) -> SearchPlan {
    let mut queries = Vec::new();
    let field = if request.technical_field.trim().is_empty() {
        request.title.trim()
    } else {
        request.technical_field.trim()
    };
    queries.push(SearchQuery {
        label: "core invention".to_string(),
        query: format!("\"{}\" patent", request.title.trim()),
        intent: "Find close title and phrase matches.".to_string(),
    });
    if !field.is_empty() {
        queries.push(SearchQuery {
            label: "technical field".to_string(),
            query: format!("{field} {}", request.problem.trim()),
            intent: "Map the problem space and common terminology.".to_string(),
        });
    }
    for (index, novelty) in request.novelty_claims.iter().take(6).enumerate() {
        queries.push(SearchQuery {
            label: format!("novelty point {}", index + 1),
            query: format!("{} {}", request.title.trim(), novelty),
            intent: "Check whether a claimed feature appears in earlier publications.".to_string(),
        });
    }
    let mut classification_hints = Vec::new();
    if !request.technical_field.trim().is_empty() {
        classification_hints.push(format!(
            "Start CPC/USPC exploration around {}.",
            request.technical_field.trim()
        ));
    }
    classification_hints.push(
        "Record patent families, earliest priority dates, claim overlap, and non-patent literature."
            .to_string(),
    );

    SearchPlan {
        queries,
        classification_hints,
        sources: vec![
            SearchSource {
                name: "USPTO Patent Public Search".to_string(),
                url: "https://ppubs.uspto.gov/pubwebapp/".to_string(),
                use_case: "US patent and published application search.".to_string(),
            },
            SearchSource {
                name: "Patent Center".to_string(),
                url: "https://patentcenter.uspto.gov/".to_string(),
                use_case: "Operator filing handoff and application management.".to_string(),
            },
            SearchSource {
                name: "Google Patents".to_string(),
                url: "https://patents.google.com/".to_string(),
                use_case: "Broad keyword and family exploration.".to_string(),
            },
            SearchSource {
                name: "Espacenet".to_string(),
                url: "https://worldwide.espacenet.com/".to_string(),
                use_case: "International patent family and classification review.".to_string(),
            },
            SearchSource {
                name: "WIPO PATENTSCOPE".to_string(),
                url: "https://patentscope.wipo.int/".to_string(),
                use_case: "PCT and international publication search.".to_string(),
            },
        ],
        review_notes: vec![
            "Preserve search strings and reviewed references for counsel.".to_string(),
            "Compare every close reference against the novelty list and embodiments.".to_string(),
        ],
    }
}

fn build_checklist(
    config: &Config,
    request: &PatentIntakeRequest,
    readiness: &ReadinessReview,
) -> Vec<ChecklistItem> {
    let mut items = vec![
        ChecklistItem {
            label: "Invention disclosure intake".to_string(),
            status: if readiness.blockers.is_empty() { "complete" } else { "open" }.to_string(),
            owner: "inventor".to_string(),
            notes: "Title, inventors, problem, solution, novelty, and embodiments.".to_string(),
        },
        ChecklistItem {
            label: "Specification draft".to_string(),
            status: "draft".to_string(),
            owner: "service".to_string(),
            notes: "Generated sections are drafting support, not final legal text.".to_string(),
        },
        ChecklistItem {
            label: "Drawings and figures".to_string(),
            status: if request.attachments.is_empty() { "open" } else { "draft" }.to_string(),
            owner: "inventor".to_string(),
            notes: "Prepare clean figures from the drawing plan before filing.".to_string(),
        },
        ChecklistItem {
            label: "Prior-art search notes".to_string(),
            status: if request.known_prior_art.is_empty() { "open" } else { "draft" }.to_string(),
            owner: "operator".to_string(),
            notes: "Use search plan results to brief counsel.".to_string(),
        },
        ChecklistItem {
            label: "Attorney review".to_string(),
            status: if request.attorney_review.unwrap_or(false) { "requested" } else { "open" }.to_string(),
            owner: "counsel".to_string(),
            notes: "Review inventorship, enablement, claim strategy, disclosure timing, and filing type.".to_string(),
        },
        ChecklistItem {
            label: "Patent Center filing handoff".to_string(),
            status: "operator-action".to_string(),
            owner: "operator".to_string(),
            notes: format!("Use configured handoff URL: {}", config.patent_center_url),
        },
    ];
    if request.target_filing.as_deref() == Some("non-provisional") {
        items.push(ChecklistItem {
            label: "Oath/declaration and ADS".to_string(),
            status: "open".to_string(),
            owner: "counsel".to_string(),
            notes: "Non-provisional filings usually need formal forms and claim review."
                .to_string(),
        });
    }
    items
}

fn build_handoff(
    request: &PatentIntakeRequest,
    draft: &ProvisionalDraft,
    search_plan: &SearchPlan,
) -> AttorneyHandoff {
    AttorneyHandoff {
        summary: format!(
            "{} inventor(s), {} novelty point(s), {} draft section(s), {} search query group(s).",
            request.inventor_names.len(),
            request.novelty_claims.len(),
            draft.sections.len(),
            search_plan.queries.len()
        ),
        questions: vec![
            "Are all named contributors legally inventors for at least one claim?".to_string(),
            "Does the disclosure enable a skilled person to make and use the invention?"
                .to_string(),
            "Should this be filed as provisional, non-provisional, design, or PCT-related work?"
                .to_string(),
            "Do public disclosures or offers for sale create deadline pressure?".to_string(),
            "Which claim seeds should become independent claims?".to_string(),
        ],
        package_manifest: vec![
            "invention-intake.json".to_string(),
            "readiness-review.json".to_string(),
            "draft-specification.md".to_string(),
            "claim-seeds.md".to_string(),
            "drawing-plan.md".to_string(),
            "prior-art-search-plan.md".to_string(),
            "claim-audit.json".to_string(),
            "uspto-fee-estimate.json".to_string(),
            "filing-deadlines.json".to_string(),
            "filing-checklist.md".to_string(),
        ],
    }
}

// ---------------------------------------------------------------------------
// Civil date utilities (dependency-free, Howard Hinnant's algorithms)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CivilDate {
    y: i64,
    m: u32,
    d: u32,
}

fn is_leap(y: i64) -> bool {
    y % 4 == 0 && (y % 100 != 0 || y % 400 == 0)
}

fn days_in_month(y: i64, m: u32) -> u32 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if is_leap(y) {
                29
            } else {
                28
            }
        }
        _ => 30,
    }
}

impl CivilDate {
    fn parse(value: &str) -> Option<CivilDate> {
        let value = value.trim();
        let mut parts = value.split('-');
        let y = parts.next()?.parse::<i64>().ok()?;
        let m = parts.next()?.parse::<u32>().ok()?;
        let d = parts.next()?.parse::<u32>().ok()?;
        if parts.next().is_some() {
            return None;
        }
        if !(1900..=4000).contains(&y) || !(1..=12).contains(&m) {
            return None;
        }
        if d < 1 || d > days_in_month(y, m) {
            return None;
        }
        Some(CivilDate { y, m, d })
    }

    /// Days since the Unix epoch (1970-01-01).
    fn to_days(self) -> i64 {
        let y = self.y - if self.m <= 2 { 1 } else { 0 };
        let era = if y >= 0 { y } else { y - 399 } / 400;
        let yoe = y - era * 400;
        let mp = if self.m > 2 { self.m - 3 } else { self.m + 9 } as i64;
        let doy = (153 * mp + 2) / 5 + self.d as i64 - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        era * 146097 + doe - 719468
    }

    fn from_days(z: i64) -> CivilDate {
        let z = z + 719468;
        let era = if z >= 0 { z } else { z - 146096 } / 146097;
        let doe = z - era * 146097;
        let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
        let y = yoe + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
        let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
        CivilDate {
            y: y + if m <= 2 { 1 } else { 0 },
            m,
            d,
        }
    }

    fn add_months(self, n: i64) -> CivilDate {
        let total = self.y * 12 + (self.m as i64 - 1) + n;
        let y = total.div_euclid(12);
        let m = (total.rem_euclid(12) + 1) as u32;
        let d = self.d.min(days_in_month(y, m));
        CivilDate { y, m, d }
    }

    fn format(self) -> String {
        format!("{:04}-{:02}-{:02}", self.y, self.m, self.d)
    }
}

fn today_civil() -> CivilDate {
    CivilDate::from_days((now_ms() / 86_400_000) as i64)
}

// ---------------------------------------------------------------------------
// USPTO fee estimation
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Entity {
    Large,
    Small,
    Micro,
}

impl Entity {
    fn parse(value: Option<&str>) -> Entity {
        match value.map(|item| item.trim().to_ascii_lowercase()).as_deref() {
            Some("small") => Entity::Small,
            Some("micro") => Entity::Micro,
            _ => Entity::Large,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Entity::Large => "large",
            Entity::Small => "small",
            Entity::Micro => "micro",
        }
    }

    /// Scale an undiscounted (large-entity) fee. Small = 40%, micro = 20%;
    /// the 2025 schedule values are exact multiples so integer math is exact.
    fn scale(self, large_cents: u64) -> u64 {
        match self {
            Entity::Large => large_cents,
            Entity::Small => large_cents * 2 / 5,
            Entity::Micro => large_cents / 5,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct FeeLineItem {
    code: String,
    label: String,
    unit_usd: f64,
    quantity: u64,
    amount_usd: f64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct FeeEstimate {
    entity: String,
    filing_track: String,
    currency: &'static str,
    effective_date: &'static str,
    line_items: Vec<FeeLineItem>,
    total_usd: f64,
    disclaimer: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FeeEstimateRequest {
    entity_status: Option<String>,
    filing_track: Option<String>,
    total_claims: Option<usize>,
    independent_claims: Option<usize>,
    has_multiple_dependent_claim: Option<bool>,
}

/// Large-entity (undiscounted) USPTO fee amounts in whole US dollars, effective
/// 2025-01-19. Source: USPTO fee schedule.
fn fee_line(
    entity: Entity,
    code: &str,
    label: &str,
    large_usd: u64,
    quantity: u64,
) -> Option<FeeLineItem> {
    if quantity == 0 {
        return None;
    }
    let unit = entity.scale(large_usd) as f64;
    Some(FeeLineItem {
        code: code.to_string(),
        label: label.to_string(),
        unit_usd: unit,
        quantity,
        amount_usd: unit * quantity as f64,
    })
}

fn estimate_fees(
    entity: Entity,
    track: &str,
    total_claims: usize,
    independent_claims: usize,
    has_multiple_dependent_claim: bool,
) -> FeeEstimate {
    let total_claims = total_claims.min(MAX_CLAIMS);
    let independent_claims = independent_claims.min(total_claims.max(1)).max(1);
    let mut items = Vec::new();

    if track == "provisional" {
        items.extend(fee_line(
            entity,
            "provisional-filing",
            "Provisional application filing fee",
            325,
            1,
        ));
    } else {
        items.extend(fee_line(
            entity,
            "basic-filing",
            "Utility nonprovisional basic filing fee",
            350,
            1,
        ));
        items.extend(fee_line(entity, "search", "Utility search fee", 770, 1));
        items.extend(fee_line(
            entity,
            "examination",
            "Utility examination fee",
            880,
            1,
        ));
        let excess_independent = independent_claims.saturating_sub(3) as u64;
        items.extend(fee_line(
            entity,
            "excess-independent-claims",
            "Each independent claim in excess of 3",
            600,
            excess_independent,
        ));
        let excess_total = total_claims.saturating_sub(20) as u64;
        items.extend(fee_line(
            entity,
            "excess-claims",
            "Each claim in excess of 20",
            200,
            excess_total,
        ));
        if has_multiple_dependent_claim {
            items.extend(fee_line(
                entity,
                "multiple-dependent-claim",
                "Multiple dependent claim fee (per application)",
                925,
                1,
            ));
        }
    }

    let total_usd = items.iter().map(|item| item.amount_usd).sum();
    FeeEstimate {
        entity: entity.label().to_string(),
        filing_track: track.to_string(),
        currency: "USD",
        effective_date: FEE_EFFECTIVE_DATE,
        line_items: items,
        total_usd,
        disclaimer:
            "Estimate of standard USPTO fees only (effective 2025-01-19). Excludes attorney fees, \
             extensions, petitions, IDS, issue/maintenance, and any fee changes after the effective date."
                .to_string(),
    }
}

// ---------------------------------------------------------------------------
// Filing deadline analysis
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct DeadlineMilestone {
    code: String,
    label: String,
    basis_date: String,
    due_date: String,
    days_remaining: i64,
    status: String,
    severity: String,
    note: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct DeadlineReport {
    today: String,
    milestones: Vec<DeadlineMilestone>,
    warnings: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeadlineRequest {
    provisional_filing_date: Option<String>,
    public_disclosure_date: Option<String>,
    foreign_priority_date: Option<String>,
    today: Option<String>,
}

fn milestone(
    today: CivilDate,
    code: &str,
    label: &str,
    basis: CivilDate,
    months: i64,
    note: &str,
) -> DeadlineMilestone {
    let due = basis.add_months(months);
    let days_remaining = due.to_days() - today.to_days();
    let (status, severity) = if days_remaining < 0 {
        ("past", "blocker")
    } else if days_remaining <= 30 {
        ("due-soon", "warning")
    } else if days_remaining <= 90 {
        ("approaching", "warning")
    } else {
        ("ok", "info")
    };
    DeadlineMilestone {
        code: code.to_string(),
        label: label.to_string(),
        basis_date: basis.format(),
        due_date: due.format(),
        days_remaining,
        status: status.to_string(),
        severity: severity.to_string(),
        note: note.to_string(),
    }
}

fn analyze_deadlines(
    provisional_filing_date: Option<&str>,
    public_disclosure_date: Option<&str>,
    foreign_priority_date: Option<&str>,
    today_override: Option<&str>,
) -> DeadlineReport {
    let today = today_override
        .and_then(CivilDate::parse)
        .unwrap_or_else(today_civil);
    let mut milestones = Vec::new();
    let mut warnings = Vec::new();

    let provisional = provisional_filing_date.and_then(CivilDate::parse);
    if let Some(basis) = provisional {
        milestones.push(milestone(
            today,
            "nonprovisional-from-provisional",
            "Nonprovisional or PCT must claim provisional benefit",
            basis,
            12,
            "37 CFR 1.78: a provisional has 12 months of pendency and cannot be extended.",
        ));
        milestones.push(milestone(
            today,
            "provisional-restoration",
            "Restoration-of-priority outer limit",
            basis,
            14,
            "Benefit may be restored under 37 CFR 1.78 only within 14 months and only on petition.",
        ));
        milestones.push(milestone(
            today,
            "paris-convention-foreign",
            "Paris Convention foreign filing deadline",
            basis,
            12,
            "File foreign / PCT applications within 12 months to claim provisional priority.",
        ));
    }
    if provisional_filing_date.is_some() && provisional.is_none() {
        warnings.push("provisionalFilingDate is not a valid YYYY-MM-DD date.".to_string());
    }

    if let Some(basis) = foreign_priority_date.and_then(CivilDate::parse) {
        if provisional.is_none() {
            milestones.push(milestone(
                today,
                "paris-convention-foreign",
                "Paris Convention / PCT priority deadline",
                basis,
                12,
                "Downstream filings claiming this priority date are generally due within 12 months.",
            ));
        }
    } else if foreign_priority_date.is_some() {
        warnings.push("foreignPriorityDate is not a valid YYYY-MM-DD date.".to_string());
    }

    if let Some(basis) = public_disclosure_date.and_then(CivilDate::parse) {
        milestones.push(milestone(
            today,
            "us-grace-period-bar",
            "US one-year grace-period statutory bar (35 USC 102(b)(1))",
            basis,
            12,
            "A US application is generally barred 12 months after the inventor's public disclosure.",
        ));
        warnings.push(
            "A public disclosure was recorded: most non-US jurisdictions require absolute novelty, \
             so foreign rights may already be lost regardless of the US grace period."
                .to_string(),
        );
    } else if public_disclosure_date.is_some() {
        warnings.push("publicDisclosureDate is not a valid YYYY-MM-DD date.".to_string());
    }

    if milestones.is_empty() {
        warnings.push(
            "No filing/disclosure/priority dates were provided, so no deadlines were computed."
                .to_string(),
        );
    }

    milestones.sort_by_key(|item| item.days_remaining);
    DeadlineReport {
        today: today.format(),
        milestones,
        warnings,
    }
}

// ---------------------------------------------------------------------------
// Claim formality / proofreading checks
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ClaimAudit {
    total_claims: usize,
    independent_claims: usize,
    dependent_claims: usize,
    multiple_dependent_claims: usize,
    has_multiple_dependent_claim: bool,
    abstract_word_count: Option<usize>,
    findings: Vec<FilingFinding>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClaimCheckRequest {
    #[serde(default)]
    claims: Vec<String>,
    #[serde(rename = "abstract", alias = "abstractText", default)]
    abstract_text: Option<String>,
}

/// Pull the claim numbers a claim depends on, plus whether the reference spans
/// multiple base claims (i.e. it is a multiple-dependent claim).
fn parse_claim_dependencies(text: &str) -> (Vec<usize>, bool) {
    let lower = text.to_ascii_lowercase();
    let bytes = lower.as_bytes();
    let mut refs = BTreeSet::new();
    let mut multi_phrase = false;
    // "any of", "any one of", "either of" signal multiple-dependent form.
    for marker in ["any of", "any one of", "either of", "one of claims"] {
        if lower.contains(marker) {
            multi_phrase = true;
        }
    }
    let mut idx = 0;
    while let Some(pos) = lower[idx..].find("claim") {
        let mut cursor = idx + pos + "claim".len();
        if lower[cursor..].starts_with('s') {
            cursor += 1;
        }
        // Parse a run of claim numbers possibly joined by ranges/lists.
        let mut local: Vec<usize> = Vec::new();
        let mut last_num: Option<usize> = None;
        let mut pending_range = false;
        loop {
            while cursor < bytes.len() && (bytes[cursor] as char).is_whitespace() {
                cursor += 1;
            }
            let connector = &lower[cursor..];
            if connector.starts_with('-') || connector.starts_with("to ") || connector.starts_with("through ") {
                pending_range = true;
                cursor += if connector.starts_with('-') { 1 } else if connector.starts_with("to ") { 3 } else { 8 };
                continue;
            }
            if connector.starts_with(',') || connector.starts_with("or ") || connector.starts_with("and ") {
                cursor += if connector.starts_with(',') { 1 } else if connector.starts_with("or ") { 3 } else { 4 };
                continue;
            }
            let num_start = cursor;
            while cursor < bytes.len() && (bytes[cursor] as char).is_ascii_digit() {
                cursor += 1;
            }
            if cursor == num_start {
                break;
            }
            if let Ok(num) = lower[num_start..cursor].parse::<usize>() {
                if pending_range {
                    if let Some(start) = last_num {
                        // Clamp the expansion: a referenced claim above MAX_CLAIMS
                        // is invalid anyway, and an unclamped range parsed from
                        // untrusted digits (e.g. "claim 1 to 9999999999") would
                        // be an unbounded-loop / OOM DoS.
                        let end = num.min(start.saturating_add(MAX_CLAIMS));
                        for n in (start + 1)..=end {
                            if local.len() >= MAX_CLAIMS {
                                break;
                            }
                            local.push(n);
                        }
                        if num > end {
                            local.push(num); // record the out-of-range ref so it is flagged
                        }
                    }
                    pending_range = false;
                } else {
                    local.push(num);
                }
                last_num = Some(num);
            }
            if local.len() >= MAX_CLAIMS {
                break;
            }
        }
        if local.len() > 1 {
            multi_phrase = true;
        }
        for n in local {
            refs.insert(n);
        }
        if refs.len() >= MAX_CLAIMS {
            break;
        }
        idx = cursor.max(idx + pos + 1);
    }
    let mut refs: Vec<usize> = refs.into_iter().collect();
    refs.sort_unstable();
    let multiple = multi_phrase && refs.len() > 1;
    (refs, multiple)
}

const ANTECEDENT_STOPWORDS: &[&str] = &[
    "the", "a", "an", "said", "wherein", "comprising", "comprises", "including", "includes",
    "of", "to", "and", "or", "for", "with", "in", "on", "at", "by", "from", "as", "is", "are",
    "claim", "claims", "method", "system", "apparatus", "device", "invention", "art", "group",
    "same", "step", "steps", "first", "second", "third", "one", "more", "least", "plurality",
    "according", "preceding", "any", "each", "which", "that", "wherein", "being", "having",
];

fn is_noun_token(w: &str) -> bool {
    !ANTECEDENT_STOPWORDS.contains(&w) && !w.chars().all(|c| c.is_ascii_digit())
}

/// Split a claim into (terms introduced with `a`/`an`, definite uses of
/// `the`/`said X` as `(article, noun)` pairs in order).
fn introduced_and_definite(text: &str) -> (BTreeSet<String>, Vec<(String, String)>) {
    let lower = text.to_ascii_lowercase();
    let words: Vec<String> = lower
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(|w| w.to_string())
        .collect();
    let mut introduced = BTreeSet::new();
    let mut definite = Vec::new();
    for i in 0..words.len() {
        let w = words[i].as_str();
        let next = words.get(i + 1).map(|s| s.as_str());
        if w == "a" || w == "an" {
            if let Some(n) = next.filter(|n| is_noun_token(n)) {
                introduced.insert(n.to_string());
            }
        } else if w == "the" || w == "said" {
            if let Some(n) = next.filter(|n| is_noun_token(n)) {
                definite.push((w.to_string(), n.to_string()));
            }
        }
    }
    (introduced, definite)
}

/// Conservative, advisory antecedent-basis scan over a single claim with a set
/// of terms already introduced by ancestor claims. Flags `the X` / `said X`
/// where `X` was never introduced with `a X` / `an X`.
fn antecedent_findings_with_context(
    claim_number: usize,
    text: &str,
    inherited: &BTreeSet<String>,
) -> Vec<FilingFinding> {
    let (own, definite) = introduced_and_definite(text);
    let mut flagged = BTreeSet::new();
    let mut findings = Vec::new();
    for (article, noun) in definite {
        if own.contains(&noun) || inherited.contains(&noun) || flagged.contains(&noun) {
            continue;
        }
        flagged.insert(noun.clone());
        findings.push(finding(
            "antecedent-basis",
            "warning",
            &format!(
                "Claim {claim_number}: '{article} {noun}' may lack antecedent basis (no earlier 'a {noun}'/'an {noun}'). Advisory heuristic — confirm manually."
            ),
        ));
    }
    findings
}

#[cfg(test)]
fn antecedent_findings(claim_number: usize, text: &str) -> Vec<FilingFinding> {
    antecedent_findings_with_context(claim_number, text, &BTreeSet::new())
}

fn audit_claims(claims: &[String], abstract_text: Option<&str>) -> ClaimAudit {
    let claims: Vec<String> = claims
        .iter()
        .map(|claim| clean_text(claim, MAX_TEXT_LEN))
        .filter(|claim| !claim.is_empty())
        .take(MAX_CLAIMS)
        .collect();
    let total_claims = claims.len();
    let mut independent_claims = 0;
    let mut dependent_claims = 0;
    let mut multiple_dependent_claims = 0;
    let mut findings = Vec::new();
    let mut multi_dependent_positions: Vec<usize> = Vec::new();
    let mut claim_refs: Vec<Vec<usize>> = Vec::with_capacity(total_claims);
    // Terms introduced by each claim plus everything inherited from its valid
    // ancestor chain, so dependent claims do not falsely flag parent terms.
    let mut effective_intro: Vec<BTreeSet<String>> = Vec::with_capacity(total_claims);

    for (index, claim) in claims.iter().enumerate() {
        let claim_number = index + 1;
        let (refs, is_multiple) = parse_claim_dependencies(claim);
        if refs.is_empty() {
            independent_claims += 1;
        } else {
            dependent_claims += 1;
            if is_multiple {
                multiple_dependent_claims += 1;
                multi_dependent_positions.push(claim_number);
            }
            for &referenced in &refs {
                if referenced == 0 || referenced > total_claims {
                    findings.push(finding(
                        "invalid-claim-reference",
                        "blocker",
                        &format!(
                            "Claim {claim_number} references claim {referenced}, which does not exist."
                        ),
                    ));
                } else if referenced >= claim_number {
                    findings.push(finding(
                        "improper-claim-dependency",
                        "blocker",
                        &format!(
                            "Claim {claim_number} depends on claim {referenced}; a claim may only depend on a lower-numbered preceding claim (35 USC 112(d))."
                        ),
                    ));
                }
            }
        }
        let (own_intro, _) = introduced_and_definite(claim);
        let mut inherited = own_intro;
        for &referenced in &refs {
            if referenced >= 1 && referenced < claim_number {
                if let Some(parent) = effective_intro.get(referenced - 1) {
                    inherited.extend(parent.iter().cloned());
                }
            }
        }
        findings.extend(antecedent_findings_with_context(claim_number, claim, &inherited));
        effective_intro.push(inherited);
        claim_refs.push(refs);
    }

    // A multiple dependent claim may not serve as a basis for another
    // multiple dependent claim (35 USC 112(e)).
    for &pos in &multi_dependent_positions {
        let refs = &claim_refs[pos - 1];
        if refs.iter().any(|&r| multi_dependent_positions.contains(&r)) {
            findings.push(finding(
                "multiple-dependent-on-multiple-dependent",
                "blocker",
                &format!(
                    "Claim {pos} is a multiple dependent claim that references another multiple dependent claim, which 35 USC 112(e) prohibits."
                ),
            ));
        }
    }

    if total_claims == 0 {
        findings.push(finding(
            "no-claims",
            "warning",
            "No claims were provided to check.",
        ));
    } else if independent_claims == 0 {
        findings.push(finding(
            "no-independent-claim",
            "blocker",
            "A claim set must contain at least one independent claim.",
        ));
    }
    if independent_claims > 3 {
        findings.push(finding(
            "excess-independent-claims",
            "info",
            &format!(
                "{independent_claims} independent claims: each over 3 carries an excess-claim fee."
            ),
        ));
    }
    if total_claims > 20 {
        findings.push(finding(
            "excess-claims",
            "info",
            &format!("{total_claims} total claims: each over 20 carries an excess-claim fee."),
        ));
    }

    let abstract_word_count = abstract_text.map(|text| {
        let count = text.split_whitespace().count();
        if count > ABSTRACT_WORD_LIMIT {
            findings.push(finding(
                "abstract-too-long",
                "warning",
                &format!(
                    "Abstract is {count} words; 37 CFR 1.72(b) limits it to {ABSTRACT_WORD_LIMIT} words."
                ),
            ));
        }
        count
    });

    ClaimAudit {
        total_claims,
        independent_claims,
        dependent_claims,
        multiple_dependent_claims,
        has_multiple_dependent_claim: multiple_dependent_claims > 0,
        abstract_word_count,
        findings,
    }
}

// ---------------------------------------------------------------------------
// AI-assisted drafting (Claude) with a deterministic self-audit + repair loop
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct AiDraft {
    #[serde(rename = "abstract", alias = "abstractText", default)]
    abstract_text: String,
    #[serde(default)]
    claims: Vec<String>,
    #[serde(default)]
    sections: Vec<DraftSection>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct AiDraftResponse {
    ok: bool,
    model: String,
    repair_applied: bool,
    draft: AiDraft,
    claim_audit: ClaimAudit,
    fee_estimate: FeeEstimate,
    disclaimer: String,
}

#[derive(Deserialize)]
struct AnthropicTextBlock {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    text: Option<String>,
}

#[derive(Deserialize)]
struct AnthropicResponse {
    #[serde(default)]
    content: Vec<AnthropicTextBlock>,
    #[serde(default)]
    stop_reason: Option<String>,
}

const AI_SYSTEM_PROMPT: &str = "You are a patent drafting assistant that prepares provisional-application \
drafting support for review by a registered patent practitioner. You do not give legal advice and you do not \
file anything. Draft in clear, enabling, US-practice style.\n\n\
Return ONLY a JSON object (no prose, no markdown fences) with exactly these keys:\n\
- \"abstract\": a single paragraph of at most 150 words.\n\
- \"claims\": an array of claim strings. Claim 1 must be independent. Every dependent claim must reference an \
earlier, lower-numbered claim by number (e.g. \"The system of claim 1, wherein ...\") and must not forward- or \
self-reference. Maintain proper antecedent basis: introduce each element with \"a\"/\"an\" before later \
referring to it with \"the\"/\"said\". Include at least one independent apparatus/system claim and one \
independent method claim when the invention supports both.\n\
- \"sections\": an array of {\"heading\", \"body\"} objects covering at least Field, Background, Summary, \
Detailed Description, and Alternative Embodiments.";

/// JSON schema constraining the model output (structured outputs).
fn ai_output_schema() -> serde_json::Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "abstract": { "type": "string" },
            "claims": { "type": "array", "items": { "type": "string" } },
            "sections": {
                "type": "array",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "properties": {
                        "heading": { "type": "string" },
                        "body": { "type": "string" }
                    },
                    "required": ["heading", "body"]
                }
            }
        },
        "required": ["abstract", "claims", "sections"]
    })
}

fn intake_brief(request: &PatentIntakeRequest) -> String {
    let list = |label: &str, items: &[String]| {
        if items.is_empty() {
            String::new()
        } else {
            format!("\n{label}:\n- {}", items.join("\n- "))
        }
    };
    let brief = format!(
        "Title: {title}\nTechnical field: {field}\nInventors: {inventors}\n\nProblem:\n{problem}\n\nSolution:\n{solution}\n\nInvention summary:\n{summary}{novelty}{embodiments}{alternatives}{advantages}\n\nDesired claim count (approximate): {claims}",
        title = request.title,
        field = if request.technical_field.trim().is_empty() { "(unspecified)" } else { request.technical_field.trim() },
        inventors = if request.inventor_names.is_empty() { "(unspecified)".to_string() } else { request.inventor_names.join(", ") },
        problem = request.problem,
        solution = request.solution,
        summary = request.invention_summary,
        novelty = list("Novelty points", &request.novelty_claims),
        embodiments = list("Embodiments", &request.embodiments),
        alternatives = list("Alternatives", &request.alternatives),
        advantages = list("Advantages", &request.advantages),
        claims = request.desired_claim_count.unwrap_or(10),
    );
    // List fields are not individually length-capped by validate_intake, so bound
    // the whole brief to keep model cost predictable regardless of input size.
    if brief.chars().count() > AI_BRIEF_MAX_CHARS {
        brief.chars().take(AI_BRIEF_MAX_CHARS).collect()
    } else {
        brief
    }
}

/// Strip an optional ```json ... ``` fence and parse the model's JSON output.
fn parse_ai_draft(text: &str) -> Result<AiDraft, String> {
    let trimmed = text.trim();
    let body = if let Some(rest) = trimmed.strip_prefix("```") {
        let rest = rest.strip_prefix("json").unwrap_or(rest);
        rest.trim_start_matches('\n')
            .strip_suffix("```")
            .unwrap_or(rest)
            .trim()
            .trim_end_matches("```")
            .trim()
    } else {
        trimmed
    };
    serde_json::from_str::<AiDraft>(body)
        .map_err(|error| format!("model did not return the expected JSON: {error}"))
}

async fn anthropic_messages(
    state: &AppState,
    api_key: &str,
    system: &str,
    user_messages: &[serde_json::Value],
) -> Result<AiDraft, String> {
    let body = json!({
        "model": state.config.ai_model,
        "max_tokens": AI_MAX_TOKENS,
        "thinking": { "type": "adaptive" },
        "output_config": {
            "effort": "high",
            "format": { "type": "json_schema", "schema": ai_output_schema() }
        },
        "system": system,
        "messages": user_messages,
    });
    let response = state
        .http
        .post(format!("{}/v1/messages", state.config.anthropic_base_url))
        .header("x-api-key", api_key)
        .header("anthropic-version", ANTHROPIC_VERSION)
        .header("content-type", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|error| format!("request to model failed: {error}"))?;
    let status = response.status();
    let text = response
        .text()
        .await
        .map_err(|error| format!("failed to read model response: {error}"))?;
    if !status.is_success() {
        let snippet: String = text.chars().take(AI_ERROR_SNIPPET_CHARS).collect();
        return Err(format!("model returned HTTP {}: {}", status.as_u16(), snippet));
    }
    let parsed: AnthropicResponse = serde_json::from_str(&text)
        .map_err(|error| format!("could not parse model envelope: {error}"))?;
    if parsed.stop_reason.as_deref() == Some("refusal") {
        return Err("model declined to produce a draft for this input".to_string());
    }
    let json_text = parsed
        .content
        .iter()
        .filter(|block| block.kind == "text")
        .filter_map(|block| block.text.clone())
        .collect::<Vec<_>>()
        .join("");
    if json_text.trim().is_empty() {
        return Err("model returned no text content".to_string());
    }
    parse_ai_draft(&json_text)
}

fn user_message(text: String) -> serde_json::Value {
    json!({ "role": "user", "content": text })
}

async fn generate_ai_draft(
    state: &AppState,
    request: PatentIntakeRequest,
) -> Result<AiDraftResponse, String> {
    let api_key = state
        .config
        .anthropic_api_key
        .clone()
        .ok_or("AI drafting is not configured")?;
    validate_intake(&request)?;
    let brief = intake_brief(&request);

    let draft = anthropic_messages(
        state,
        &api_key,
        AI_SYSTEM_PROMPT,
        &[user_message(format!(
            "Draft a provisional patent application from this invention disclosure.\n\n{brief}"
        ))],
    )
    .await?;

    let audit = audit_claims(&draft.claims, Some(&draft.abstract_text));
    let blockers: Vec<String> = audit
        .findings
        .iter()
        .filter(|finding| finding.severity == "blocker")
        .map(|finding| finding.message.clone())
        .collect();

    // Self-audit repair pass: feed the deterministic checker's blockers back to
    // the model exactly once and re-audit the result.
    let (draft, audit, repair_applied) = if blockers.is_empty() {
        (draft, audit, false)
    } else {
        let prior = serde_json::to_string(&draft).unwrap_or_default();
        let repair = anthropic_messages(
            state,
            &api_key,
            AI_SYSTEM_PROMPT,
            &[
                user_message(format!(
                    "Draft a provisional patent application from this invention disclosure.\n\n{brief}"
                )),
                user_message(format!(
                    "Your previous draft was:\n{prior}\n\nAn automated formality checker found these blocking issues:\n- {}\n\nReturn a corrected JSON draft that resolves every issue while keeping the same invention scope.",
                    blockers.join("\n- ")
                )),
            ],
        )
        .await;
        match repair {
            Ok(repaired) => {
                let repaired_audit = audit_claims(&repaired.claims, Some(&repaired.abstract_text));
                (repaired, repaired_audit, true)
            }
            // If the repair call fails, keep the first draft and its findings.
            Err(_) => (draft, audit, false),
        }
    };

    let entity = Entity::parse(request.entity_status.as_deref());
    let track = normalize_track(request.target_filing.as_ref());
    let fee_estimate = estimate_fees(
        entity,
        &track,
        audit.total_claims,
        audit.independent_claims,
        audit.has_multiple_dependent_claim,
    );

    Ok(AiDraftResponse {
        ok: true,
        model: state.config.ai_model.clone(),
        repair_applied,
        draft,
        claim_audit: audit,
        fee_estimate,
        disclaimer:
            "AI-generated drafting support only. Not legal advice and not a filing. A registered patent \
             practitioner must review inventorship, enablement, claim scope, and prior art before any filing."
                .to_string(),
    })
}

fn build_package(
    config: &Config,
    request: PatentIntakeRequest,
) -> Result<PatentMatterPackage, String> {
    validate_intake(&request)?;
    let request_id = request_id(request.request_id.as_ref(), "patent-package");
    let generated_at_ms = now_ms();
    let readiness = evaluate_readiness(&request);
    let draft = build_draft(&request);
    let search_plan = build_search_plan(&request);
    let filing_checklist = build_checklist(config, &request, &readiness);
    let attorney_handoff = build_handoff(&request, &draft, &search_plan);
    let filing_track = normalize_track(request.target_filing.as_ref());
    let claim_audit = audit_claims(&draft.claim_seeds, Some(&draft.abstract_draft));
    let entity = Entity::parse(request.entity_status.as_deref());
    let fee_estimate = estimate_fees(
        entity,
        &filing_track,
        claim_audit.total_claims,
        claim_audit.independent_claims,
        claim_audit.has_multiple_dependent_claim,
    );
    let deadlines = analyze_deadlines(
        request.provisional_filing_date.as_deref(),
        request.public_disclosure_date.as_deref(),
        request.foreign_priority_date.as_deref(),
        None,
    );
    let mut warnings = readiness
        .warnings
        .iter()
        .map(|finding| finding.message.clone())
        .collect::<Vec<_>>();
    for milestone in deadlines.milestones.iter().filter(|m| m.status == "past") {
        warnings.push(format!(
            "Deadline likely missed: {} (due {}).",
            milestone.label, milestone.due_date
        ));
    }
    warnings.push("This package is preparation support only; it does not file with the USPTO or replace legal advice.".to_string());
    let matter_id = format!("pf-{}-{generated_at_ms}", slugify(&request.title));
    Ok(PatentMatterPackage {
        ok: true,
        matter_id,
        request_id,
        schema_version: SCHEMA_VERSION,
        generated_at_ms,
        filing_track,
        title: request.title,
        applicant: request.applicant,
        inventor_names: request.inventor_names,
        readiness,
        draft,
        search_plan,
        claim_audit,
        fee_estimate,
        deadlines,
        filing_checklist,
        attorney_handoff,
        warnings,
    })
}

fn store_package(state: &AppState, package: PatentMatterPackage) {
    let mut store = state.store.write().unwrap_or_else(|lock| lock.into_inner());
    store.matters.insert(0, package);
    if store.matters.len() > state.config.max_matters {
        store.matters.truncate(state.config.max_matters);
    }
}

fn package_snapshot(state: &AppState) -> Vec<PatentMatterPackage> {
    state
        .store
        .read()
        .unwrap_or_else(|lock| lock.into_inner())
        .matters
        .clone()
}

fn get_package(state: &AppState, matter_id: &str) -> Option<PatentMatterPackage> {
    state
        .store
        .read()
        .unwrap_or_else(|lock| lock.into_inner())
        .matters
        .iter()
        .find(|package| package.matter_id == matter_id)
        .cloned()
}

async fn root() -> Html<String> {
    Html(render_home())
}

async fn descriptor(State(state): State<AppState>) -> impl IntoResponse {
    Json(json!({
        "ok": true,
        "service": SERVICE_NAME,
        "schemaVersion": SCHEMA_VERSION,
        "patentCenterUrl": state.config.patent_center_url,
        "routes": {
            "home": "/",
            "package": "/packages/provisional",
            "readiness": "/readiness",
            "searchPlan": "/search/plan",
            "review": "/review/package",
            "claimsCheck": "/claims/check",
            "feesEstimate": "/fees/estimate",
            "deadlines": "/deadlines",
            "draftAi": "/draft/ai",
            "docs": "/docs/api"
        },
        "feeScheduleEffectiveDate": FEE_EFFECTIVE_DATE,
        "aiConfigured": state.config.anthropic_api_key.is_some(),
        "aiModel": state.config.ai_model,
        "stance": "preparation-only"
    }))
}

async fn schema() -> impl IntoResponse {
    Json(json!({
        "ok": true,
        "schemaVersion": SCHEMA_VERSION,
        "request": {
            "title": "string",
            "inventorNames": ["string"],
            "applicant": "string?",
            "inventionSummary": "string",
            "technicalField": "string?",
            "problem": "string",
            "solution": "string",
            "noveltyClaims": ["string"],
            "embodiments": ["string"],
            "alternatives": ["string"],
            "advantages": ["string"],
            "publicDisclosureDate": "YYYY-MM-DD?",
            "provisionalFilingDate": "YYYY-MM-DD?",
            "foreignPriorityDate": "YYYY-MM-DD?",
            "targetFiling": "provisional|non-provisional|design|pct",
            "entityStatus": "large|small|micro",
            "knownPriorArt": [{ "title": "string", "url": "string?", "notes": "string?" }],
            "attachments": [{ "name": "string", "kind": "string?", "url": "string?", "notes": "string?" }]
        },
        "response": {
            "readiness": "score, blockers, warnings, strengths, nextActions",
            "draft": "abstract, sections, claimSeeds, drawingPlan",
            "searchPlan": "queries, sources, classificationHints",
            "claimAudit": "claim counts, dependency validity, antecedent-basis advisories, abstract length",
            "feeEstimate": "USPTO fee line items by entity status (effective 2025-01-19)",
            "deadlines": "provisional/Paris/grace-period milestones with days remaining",
            "filingChecklist": "operator handoff checklist"
        },
        "auxiliaryEndpoints": {
            "claimsCheck": { "claims": ["string"], "abstract": "string?" },
            "feesEstimate": {
                "entityStatus": "large|small|micro",
                "filingTrack": "provisional|non-provisional|design|pct",
                "totalClaims": "number",
                "independentClaims": "number",
                "hasMultipleDependentClaim": "bool"
            },
            "deadlines": {
                "provisionalFilingDate": "YYYY-MM-DD?",
                "publicDisclosureDate": "YYYY-MM-DD?",
                "foreignPriorityDate": "YYYY-MM-DD?",
                "today": "YYYY-MM-DD?"
            },
            "draftAi": {
                "request": "same intake contract as /packages/provisional",
                "response": "abstract, claims, sections generated by Claude, then re-checked by /claims/check and priced; one automatic repair pass if the checker finds blockers",
                "requires": "ANTHROPIC_API_KEY"
            }
        }
    }))
}

async fn example() -> impl IntoResponse {
    Json(json!(example_request()))
}

async fn matters(State(state): State<AppState>, headers: HeaderMap) -> Response {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    if let Err(failure) = require_auth(&headers, &state) {
        return auth_failure_response(&state, failure);
    }
    let matters = package_snapshot(&state);
    Json(json!({ "ok": true, "count": matters.len(), "matters": matters })).into_response()
}

async fn matter(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(matter_id): Path<String>,
) -> Response {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    if let Err(failure) = require_auth(&headers, &state) {
        return auth_failure_response(&state, failure);
    }
    match get_package(&state, &matter_id) {
        Some(package) => Json(package).into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(json!({ "ok": false, "error": "matter not found" })),
        )
            .into_response(),
    }
}

async fn package_json(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<PatentIntakeRequest>,
) -> Response {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    if let Err(failure) = require_auth(&headers, &state) {
        return auth_failure_response(&state, failure);
    }
    match build_package(&state.config, request) {
        Ok(package) => {
            state
                .metrics
                .package_requests_total
                .fetch_add(1, Ordering::Relaxed);
            store_package(&state, package.clone());
            Json(package).into_response()
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

async fn package_form(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<UiPackageForm>,
) -> Response {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    if let Err(failure) = require_auth(&headers, &state) {
        return ui_auth_failure_response(&state, failure);
    }
    match build_package(&state.config, intake_from_form(form)) {
        Ok(package) => {
            state
                .metrics
                .package_requests_total
                .fetch_add(1, Ordering::Relaxed);
            store_package(&state, package.clone());
            Html(render_package_fragment(&package)).into_response()
        }
        Err(error) => {
            state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
            (
                StatusCode::BAD_REQUEST,
                Html(format!(
                    r#"<div class="result error"><strong>Package error</strong><p>{}</p></div>"#,
                    escape_html(&error)
                )),
            )
                .into_response()
        }
    }
}

async fn readiness_json(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<PatentIntakeRequest>,
) -> Response {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    if let Err(failure) = require_auth(&headers, &state) {
        return auth_failure_response(&state, failure);
    }
    state
        .metrics
        .readiness_requests_total
        .fetch_add(1, Ordering::Relaxed);
    Json(json!({ "ok": true, "readiness": evaluate_readiness(&request) })).into_response()
}

async fn search_plan_json(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<PatentIntakeRequest>,
) -> Response {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    if let Err(failure) = require_auth(&headers, &state) {
        return auth_failure_response(&state, failure);
    }
    state
        .metrics
        .search_plan_requests_total
        .fetch_add(1, Ordering::Relaxed);
    Json(json!({ "ok": true, "searchPlan": build_search_plan(&request) })).into_response()
}

async fn claims_check_json(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<ClaimCheckRequest>,
) -> Response {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    if let Err(failure) = require_auth(&headers, &state) {
        return auth_failure_response(&state, failure);
    }
    state
        .metrics
        .claim_checks_total
        .fetch_add(1, Ordering::Relaxed);
    let audit = audit_claims(&request.claims, request.abstract_text.as_deref());
    Json(json!({ "ok": true, "claimAudit": audit })).into_response()
}

async fn fees_estimate_json(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<FeeEstimateRequest>,
) -> Response {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    if let Err(failure) = require_auth(&headers, &state) {
        return auth_failure_response(&state, failure);
    }
    state
        .metrics
        .fee_estimates_total
        .fetch_add(1, Ordering::Relaxed);
    let entity = Entity::parse(request.entity_status.as_deref());
    let track = normalize_track(request.filing_track.as_ref());
    let total_claims = request.total_claims.unwrap_or(0);
    let independent_claims = request.independent_claims.unwrap_or(1);
    let estimate = estimate_fees(
        entity,
        &track,
        total_claims,
        independent_claims,
        request.has_multiple_dependent_claim.unwrap_or(false),
    );
    Json(json!({ "ok": true, "feeEstimate": estimate })).into_response()
}

async fn deadlines_json(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<DeadlineRequest>,
) -> Response {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    if let Err(failure) = require_auth(&headers, &state) {
        return auth_failure_response(&state, failure);
    }
    state
        .metrics
        .deadline_requests_total
        .fetch_add(1, Ordering::Relaxed);
    let report = analyze_deadlines(
        request.provisional_filing_date.as_deref(),
        request.public_disclosure_date.as_deref(),
        request.foreign_priority_date.as_deref(),
        request.today.as_deref(),
    );
    Json(json!({ "ok": true, "deadlines": report })).into_response()
}

async fn draft_ai_json(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<PatentIntakeRequest>,
) -> Response {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    if let Err(failure) = require_auth(&headers, &state) {
        return auth_failure_response(&state, failure);
    }
    if state.config.anthropic_api_key.is_none() {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({
                "ok": false,
                "error": "AI drafting is not configured; set ANTHROPIC_API_KEY (or PATENT_FILING_ANTHROPIC_API_KEY)."
            })),
        )
            .into_response();
    }
    // Bound concurrent outbound model calls: caps resource use and Anthropic
    // spend, and stops a burst of expensive long-lived requests from piling up.
    let _permit = match state.ai_permits.try_acquire() {
        Ok(permit) => permit,
        Err(_) => {
            state
                .metrics
                .ai_throttled_total
                .fetch_add(1, Ordering::Relaxed);
            return (
                StatusCode::TOO_MANY_REQUESTS,
                [(header::RETRY_AFTER, "30")],
                Json(json!({
                    "ok": false,
                    "error": "AI drafting is at capacity; retry shortly."
                })),
            )
                .into_response();
        }
    };
    match generate_ai_draft(&state, request).await {
        Ok(response) => {
            state.metrics.ai_drafts_total.fetch_add(1, Ordering::Relaxed);
            Json(response).into_response()
        }
        Err(error) => {
            state
                .metrics
                .ai_draft_errors_total
                .fetch_add(1, Ordering::Relaxed);
            // 400 for validation problems, 502 for upstream model failures.
            let status = if error.contains("model") || error.contains("request to model") {
                StatusCode::BAD_GATEWAY
            } else {
                StatusCode::BAD_REQUEST
            };
            (status, Json(json!({ "ok": false, "error": error }))).into_response()
        }
    }
}

async fn review_package_json(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<PackageReviewRequest>,
) -> Response {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    if let Err(failure) = require_auth(&headers, &state) {
        return auth_failure_response(&state, failure);
    }
    let response = review_package(&state, request);
    state
        .metrics
        .package_reviews_total
        .fetch_add(1, Ordering::Relaxed);
    Json(response).into_response()
}

fn review_package(state: &AppState, request: PackageReviewRequest) -> PackageReviewResponse {
    let package = request
        .matter_id
        .as_deref()
        .and_then(|matter_id| get_package(state, matter_id));
    let input = request.package;
    let readiness_score = package
        .as_ref()
        .map(|package| package.readiness.score)
        .or_else(|| input.as_ref().and_then(|item| item.readiness_score))
        .unwrap_or(0);
    let blocker_count = package
        .as_ref()
        .map(|package| package.readiness.blockers.len())
        .or_else(|| input.as_ref().and_then(|item| item.blocker_count))
        .unwrap_or(0);
    let section_count = package
        .as_ref()
        .map(|package| package.draft.sections.len())
        .or_else(|| input.as_ref().and_then(|item| item.section_count))
        .unwrap_or(0);
    let checklist_open_count = package
        .as_ref()
        .map(|package| {
            package
                .filing_checklist
                .iter()
                .filter(|item| item.status == "open")
                .count()
        })
        .or_else(|| input.as_ref().and_then(|item| item.checklist_open_count))
        .unwrap_or(0);

    let mut findings = Vec::new();
    let mut next_actions = Vec::new();
    if blocker_count > 0 {
        findings.push(finding(
            "readiness-blockers",
            "blocker",
            "Resolve readiness blockers before package release.",
        ));
    }
    if readiness_score < 70 {
        findings.push(finding(
            "low-readiness-score",
            "warning",
            "Readiness score is below the drafting threshold.",
        ));
        next_actions.push("Expand the invention disclosure before counsel review.".to_string());
    }
    if section_count < 5 {
        findings.push(finding(
            "thin-specification",
            "warning",
            "Draft specification has fewer than five sections.",
        ));
    }
    if checklist_open_count > 0 {
        findings.push(finding(
            "open-checklist",
            "warning",
            "One or more filing checklist items remain open.",
        ));
    }
    next_actions.push("Review package with patent counsel before filing.".to_string());

    let release_gate = if blocker_count == 0 && readiness_score >= 82 && checklist_open_count <= 1 {
        "attorney-review-ready"
    } else if blocker_count == 0 && readiness_score >= 65 {
        "draft-revision-needed"
    } else {
        "blocked"
    };
    PackageReviewResponse {
        ok: true,
        status: if findings.iter().any(|item| item.severity == "blocker") {
            "blocked".to_string()
        } else {
            "reviewed".to_string()
        },
        release_gate: release_gate.to_string(),
        findings,
        next_actions,
    }
}

async fn healthz(State(state): State<AppState>) -> impl IntoResponse {
    let count = state
        .store
        .read()
        .unwrap_or_else(|lock| lock.into_inner())
        .matters
        .len();
    Json(json!({ "ok": true, "service": SERVICE_NAME, "matterCount": count }))
}

async fn readyz(State(state): State<AppState>) -> impl IntoResponse {
    Json(json!({
        "ok": state.config.allow_unauthenticated || state.config.server_auth_secret.is_some(),
        "service": SERVICE_NAME,
        "authConfigured": state.config.server_auth_secret.is_some(),
        "allowUnauthenticated": state.config.allow_unauthenticated,
        "patentCenterUrl": state.config.patent_center_url,
        "aiConfigured": state.config.anthropic_api_key.is_some()
    }))
}

async fn metrics(State(state): State<AppState>) -> Response {
    let matter_count = state
        .store
        .read()
        .unwrap_or_else(|lock| lock.into_inner())
        .matters
        .len();
    let body = format!(
        "# HELP dd_patent_filing_http_requests_total HTTP requests observed by the patent filing service.\n\
         # TYPE dd_patent_filing_http_requests_total counter\n\
         dd_patent_filing_http_requests_total {}\n\
         # HELP dd_patent_filing_package_requests_total Filing package generation requests accepted.\n\
         # TYPE dd_patent_filing_package_requests_total counter\n\
         dd_patent_filing_package_requests_total {}\n\
         # HELP dd_patent_filing_readiness_requests_total Readiness review requests accepted.\n\
         # TYPE dd_patent_filing_readiness_requests_total counter\n\
         dd_patent_filing_readiness_requests_total {}\n\
         # HELP dd_patent_filing_search_plan_requests_total Prior-art search plan requests accepted.\n\
         # TYPE dd_patent_filing_search_plan_requests_total counter\n\
         dd_patent_filing_search_plan_requests_total {}\n\
         # HELP dd_patent_filing_package_reviews_total Package review requests accepted.\n\
         # TYPE dd_patent_filing_package_reviews_total counter\n\
         dd_patent_filing_package_reviews_total {}\n\
         # HELP dd_patent_filing_claim_checks_total Claim formality check requests accepted.\n\
         # TYPE dd_patent_filing_claim_checks_total counter\n\
         dd_patent_filing_claim_checks_total {}\n\
         # HELP dd_patent_filing_fee_estimates_total Fee estimate requests accepted.\n\
         # TYPE dd_patent_filing_fee_estimates_total counter\n\
         dd_patent_filing_fee_estimates_total {}\n\
         # HELP dd_patent_filing_deadline_requests_total Filing deadline requests accepted.\n\
         # TYPE dd_patent_filing_deadline_requests_total counter\n\
         dd_patent_filing_deadline_requests_total {}\n\
         # HELP dd_patent_filing_ai_drafts_total AI drafting requests completed.\n\
         # TYPE dd_patent_filing_ai_drafts_total counter\n\
         dd_patent_filing_ai_drafts_total {}\n\
         # HELP dd_patent_filing_ai_draft_errors_total AI drafting requests that failed.\n\
         # TYPE dd_patent_filing_ai_draft_errors_total counter\n\
         dd_patent_filing_ai_draft_errors_total {}\n\
         # HELP dd_patent_filing_ai_throttled_total AI drafting requests rejected at the concurrency limit.\n\
         # TYPE dd_patent_filing_ai_throttled_total counter\n\
         dd_patent_filing_ai_throttled_total {}\n\
         # HELP dd_patent_filing_auth_failures_total Rejected requests with missing or invalid auth.\n\
         # TYPE dd_patent_filing_auth_failures_total counter\n\
         dd_patent_filing_auth_failures_total {}\n\
         # HELP dd_patent_filing_errors_total Request or package generation errors.\n\
         # TYPE dd_patent_filing_errors_total counter\n\
         dd_patent_filing_errors_total {}\n\
         # HELP dd_patent_filing_current_matters Current retained patent matter packages.\n\
         # TYPE dd_patent_filing_current_matters gauge\n\
         dd_patent_filing_current_matters {}\n",
        state.metrics.http_requests_total.load(Ordering::Relaxed),
        state.metrics.package_requests_total.load(Ordering::Relaxed),
        state.metrics.readiness_requests_total.load(Ordering::Relaxed),
        state.metrics.search_plan_requests_total.load(Ordering::Relaxed),
        state.metrics.package_reviews_total.load(Ordering::Relaxed),
        state.metrics.claim_checks_total.load(Ordering::Relaxed),
        state.metrics.fee_estimates_total.load(Ordering::Relaxed),
        state.metrics.deadline_requests_total.load(Ordering::Relaxed),
        state.metrics.ai_drafts_total.load(Ordering::Relaxed),
        state.metrics.ai_draft_errors_total.load(Ordering::Relaxed),
        state.metrics.ai_throttled_total.load(Ordering::Relaxed),
        state.metrics.auth_failures_total.load(Ordering::Relaxed),
        state.metrics.errors_total.load(Ordering::Relaxed),
        matter_count,
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

async fn api_docs_html() -> Html<&'static str> {
    Html(include_str!("../generated/api-docs.html"))
}

async fn api_docs_json() -> impl IntoResponse {
    (
        [("content-type", "application/json; charset=utf-8")],
        include_str!("../generated/api-docs.json"),
    )
}

fn render_home() -> String {
    format!(
        r##"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>Patent Filing Workbench</title>
  <script src="{htmx_src}" integrity="{htmx_sri}" crossorigin="anonymous" referrerpolicy="no-referrer"></script>
  <style>
    :root {{
      color-scheme: light;
      --bg: #f5f7f3;
      --ink: #172026;
      --muted: #5f6b73;
      --line: #cfd8d3;
      --panel: #ffffff;
      --green: #126d57;
      --blue: #294f7a;
      --red: #9f3f32;
      --gold: #9a6b18;
      --code: #eef2f0;
    }}
    * {{ box-sizing: border-box; }}
    body {{ margin: 0; background: var(--bg); color: var(--ink); font: 14px/1.45 ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif; }}
    header {{ border-bottom: 1px solid var(--line); background: #ffffff; }}
    .top {{ width: min(1240px, calc(100% - 28px)); margin: 0 auto; padding: 18px 0 14px; display: flex; justify-content: space-between; align-items: center; gap: 14px; }}
    h1 {{ margin: 0; font-size: 22px; line-height: 1.1; letter-spacing: 0; }}
    .status-rail {{ display: flex; gap: 8px; flex-wrap: wrap; color: var(--muted); font-size: 12px; }}
    .status-rail span {{ border: 1px solid var(--line); background: #f9faf8; border-radius: 6px; padding: 5px 8px; }}
    main {{ width: min(1240px, calc(100% - 28px)); margin: 16px auto 30px; display: grid; grid-template-columns: minmax(320px, 0.92fr) minmax(340px, 1.08fr); gap: 16px; align-items: start; }}
    .panel {{ background: var(--panel); border: 1px solid var(--line); border-radius: 8px; overflow: hidden; }}
    .panel-head {{ padding: 12px 14px; border-bottom: 1px solid var(--line); display: flex; justify-content: space-between; gap: 10px; align-items: center; }}
    .panel-head h2 {{ margin: 0; font-size: 15px; letter-spacing: 0; }}
    .panel-head a {{ color: var(--blue); text-decoration: none; font-size: 12px; }}
    form {{ padding: 14px; display: grid; gap: 12px; }}
    .grid-two {{ display: grid; grid-template-columns: 1fr 1fr; gap: 10px; }}
    label {{ display: grid; gap: 5px; color: var(--muted); font-size: 12px; font-weight: 700; }}
    input, textarea, select {{ width: 100%; border: 1px solid var(--line); border-radius: 6px; background: #fff; color: var(--ink); padding: 9px 10px; font: inherit; letter-spacing: 0; }}
    textarea {{ min-height: 74px; resize: vertical; }}
    textarea.tall {{ min-height: 118px; }}
    .checkline {{ display: flex; gap: 8px; align-items: center; color: var(--ink); font-weight: 600; }}
    .checkline input {{ width: auto; }}
    .actions {{ display: flex; justify-content: flex-end; gap: 10px; align-items: center; border-top: 1px solid var(--line); padding-top: 12px; }}
    button {{ border: 0; border-radius: 6px; background: var(--green); color: white; padding: 10px 14px; font-weight: 800; cursor: pointer; }}
    button:hover {{ background: #0e5a48; }}
    .htmx-indicator {{ opacity: 0; color: var(--muted); font-size: 12px; }}
    .htmx-request .htmx-indicator, .htmx-request.htmx-indicator {{ opacity: 1; }}
    #package-output {{ min-height: 520px; }}
    .placeholder {{ color: var(--muted); padding: 18px; }}
    .result {{ padding: 14px; }}
    .result.error {{ border-left: 4px solid var(--red); }}
    .score-row {{ display: grid; grid-template-columns: 110px 1fr; gap: 14px; align-items: center; margin-bottom: 12px; }}
    .score {{ width: 96px; height: 96px; border: 8px solid var(--green); border-radius: 50%; display: grid; place-items: center; font-size: 24px; font-weight: 900; color: var(--green); }}
    .badge {{ display: inline-flex; align-items: center; border-radius: 6px; padding: 4px 8px; font-size: 12px; font-weight: 800; background: #e9f4ef; color: var(--green); }}
    .badge.warn {{ background: #fff4d9; color: var(--gold); }}
    .badge.blocked {{ background: #fbe8e4; color: var(--red); }}
    h3 {{ margin: 14px 0 7px; font-size: 13px; text-transform: uppercase; color: var(--muted); letter-spacing: 0; }}
    ul {{ margin: 0; padding-left: 18px; }}
    li {{ margin: 4px 0; }}
    .columns {{ display: grid; grid-template-columns: 1fr 1fr; gap: 12px; }}
    .mini {{ border: 1px solid var(--line); border-radius: 8px; padding: 10px; background: #fbfcfb; }}
    code {{ background: var(--code); border-radius: 5px; padding: 2px 5px; font-family: ui-monospace, "SFMono-Regular", Consolas, monospace; font-size: 12px; overflow-wrap: anywhere; }}
    @media (max-width: 880px) {{
      .top {{ align-items: flex-start; flex-direction: column; }}
      main {{ grid-template-columns: 1fr; }}
      .grid-two, .columns, .score-row {{ grid-template-columns: 1fr; }}
      .score {{ width: 82px; height: 82px; font-size: 21px; }}
    }}
  </style>
</head>
<body>
  <header>
    <div class="top">
      <h1>Patent Filing Workbench</h1>
      <div class="status-rail">
        <span>Intake</span>
        <span>Readiness</span>
        <span>Draft Package</span>
        <span>Patent Center Handoff</span>
      </div>
    </div>
  </header>
  <main>
    <section class="panel">
      <div class="panel-head">
        <h2>Invention Intake</h2>
        <a href="docs/api">API docs</a>
      </div>
      <form hx-post="ui/packages" hx-target="#package-output" hx-swap="innerHTML" hx-indicator="#package-spinner">
        <div class="grid-two">
          <label>Title
            <input name="title" value="Adaptive thermal sensor array" required>
          </label>
          <label>Target filing
            <select name="target_filing">
              <option value="provisional" selected>provisional</option>
              <option value="non-provisional">non-provisional</option>
              <option value="design">design</option>
              <option value="pct">pct</option>
            </select>
          </label>
        </div>
        <div class="grid-two">
          <label>Inventors
            <textarea name="inventor_names" required>Avery Chen
Morgan Patel</textarea>
          </label>
          <label>Applicant
            <input name="applicant" value="Example Robotics LLC">
          </label>
        </div>
        <label>Technical field
          <input name="technical_field" value="distributed sensing and thermal control">
        </label>
        <label>Summary
          <textarea class="tall" name="invention_summary" required>A distributed sensor array combines low-cost temperature probes, edge calibration, and a controller that changes sampling frequency based on local thermal gradients. Each node reports confidence and drift estimates so the controller can prioritize high-risk zones without flooding the network.</textarea>
        </label>
        <label>Problem
          <textarea name="problem" required>Existing thermal monitoring systems either sample too slowly to catch fast changes or sample every node constantly, which wastes network capacity and power in dense installations.</textarea>
        </label>
        <label>Solution
          <textarea name="solution" required>The array estimates local gradients at each node, assigns an adaptive sampling budget, and routes high-confidence alerts through a compact priority protocol while slower regions remain in a low-power cadence.</textarea>
        </label>
        <div class="grid-two">
          <label>Novelty points
            <textarea name="novelty_claims" required>Node-level drift confidence changes sampling rates
Gradient-triggered priority routing reduces bandwidth
Controller fuses confidence scores with thermal risk zones</textarea>
          </label>
          <label>Embodiments
            <textarea name="embodiments">Warehouse battery pack monitoring
Server rack airflow diagnostics
Factory motor enclosure monitoring</textarea>
          </label>
        </div>
        <div class="grid-two">
          <label>Alternatives
            <textarea name="alternatives">Wireless mesh nodes
Wired industrial bus nodes
Cloud or local controller deployment</textarea>
          </label>
          <label>Advantages
            <textarea name="advantages">Lower power usage
Reduced telemetry volume
Faster high-risk thermal alerts</textarea>
          </label>
        </div>
        <div class="grid-two">
          <label>Known prior art
            <textarea name="known_prior_art">Static threshold thermal monitoring systems
Uniform polling sensor networks</textarea>
          </label>
          <label>Figures and evidence
            <textarea name="attachments">System block diagram
Sampling-state flow chart
Prototype calibration notes</textarea>
          </label>
        </div>
        <div class="grid-two">
          <label>Entity status
            <select name="entity_status">
              <option value="large">large</option>
              <option value="small">small</option>
              <option value="micro" selected>micro</option>
            </select>
          </label>
          <label>Public disclosure date
            <input name="public_disclosure_date" placeholder="YYYY-MM-DD">
          </label>
        </div>
        <div class="grid-two">
          <label>Provisional filing date
            <input name="provisional_filing_date" placeholder="YYYY-MM-DD">
          </label>
          <label>Foreign priority date
            <input name="foreign_priority_date" placeholder="YYYY-MM-DD">
          </label>
        </div>
        <div class="grid-two">
          <label class="checkline">
            <input type="checkbox" name="attorney_review" checked>
            Attorney review requested
          </label>
        </div>
        <div class="actions">
          <span id="package-spinner" class="htmx-indicator">Generating package...</span>
          <button type="submit">Generate Filing Package</button>
        </div>
      </form>
    </section>
    <section class="panel">
      <div class="panel-head">
        <h2>Package Preview</h2>
        <a href="example">JSON example</a>
      </div>
      <div id="package-output">
        <div class="placeholder">
          <strong>Pending intake</strong>
          <p>The package preview will show readiness, draft sections, claim seeds, drawing plan, search plan, and filing handoff.</p>
        </div>
      </div>
    </section>
  </main>
</body>
</html>"##,
        htmx_src = HTMX_SRC,
        htmx_sri = HTMX_SRI,
    )
}

fn render_package_fragment(package: &PatentMatterPackage) -> String {
    let readiness_class = if !package.readiness.blockers.is_empty() {
        "blocked"
    } else if package.readiness.score < 82 {
        "warn"
    } else {
        ""
    };
    let blockers = if package.readiness.blockers.is_empty() {
        "<li>No blockers detected.</li>".to_string()
    } else {
        package
            .readiness
            .blockers
            .iter()
            .map(|item| format!("<li>{}</li>", escape_html(&item.message)))
            .collect::<String>()
    };
    let sections = package
        .draft
        .sections
        .iter()
        .take(4)
        .map(|section| {
            format!(
                "<li><strong>{}</strong>: {}</li>",
                escape_html(&section.heading),
                escape_html(&section.body.chars().take(220).collect::<String>())
            )
        })
        .collect::<String>();
    let claim_seeds = package
        .draft
        .claim_seeds
        .iter()
        .take(5)
        .map(|item| format!("<li>{}</li>", escape_html(item)))
        .collect::<String>();
    let drawing_plan = package
        .draft
        .drawing_plan
        .iter()
        .take(5)
        .map(|item| format!("<li>{}</li>", escape_html(item)))
        .collect::<String>();
    let checklist = package
        .filing_checklist
        .iter()
        .map(|item| {
            format!(
                "<li><strong>{}</strong> <code>{}</code> - {}</li>",
                escape_html(&item.label),
                escape_html(&item.status),
                escape_html(&item.notes)
            )
        })
        .collect::<String>();
    let search_queries = package
        .search_plan
        .queries
        .iter()
        .take(5)
        .map(|item| {
            format!(
                "<li><code>{}</code> {}</li>",
                escape_html(&item.label),
                escape_html(&item.query)
            )
        })
        .collect::<String>();
    let fee = &package.fee_estimate;
    let fee_rows = fee
        .line_items
        .iter()
        .map(|item| {
            format!(
                "<li>{} · {} × ${:.0} = <strong>${:.0}</strong></li>",
                escape_html(&item.label),
                item.quantity,
                item.unit_usd,
                item.amount_usd
            )
        })
        .collect::<String>();
    let deadline_rows = if package.deadlines.milestones.is_empty() {
        "<li>No filing/disclosure/priority dates provided.</li>".to_string()
    } else {
        package
            .deadlines
            .milestones
            .iter()
            .map(|item| {
                format!(
                    "<li><code>{}</code> {} — due {} ({} days)</li>",
                    escape_html(&item.status),
                    escape_html(&item.label),
                    escape_html(&item.due_date),
                    item.days_remaining
                )
            })
            .collect::<String>()
    };
    let claim_findings = if package.claim_audit.findings.is_empty() {
        "<li>No claim formality findings.</li>".to_string()
    } else {
        package
            .claim_audit
            .findings
            .iter()
            .take(8)
            .map(|item| {
                format!(
                    "<li><code>{}</code> {}</li>",
                    escape_html(&item.severity),
                    escape_html(&item.message)
                )
            })
            .collect::<String>()
    };
    let abstract_words = package
        .claim_audit
        .abstract_word_count
        .map(|count| format!("{count} words"))
        .unwrap_or_else(|| "n/a".to_string());

    format!(
        r#"<div class="result">
  <div class="score-row">
    <div class="score">{score}</div>
    <div>
      <span class="badge {readiness_class}">{status}</span>
      <h2>{title}</h2>
      <p><code>{matter_id}</code> · {track}</p>
    </div>
  </div>
  <div class="columns">
    <div class="mini">
      <h3>Blockers</h3>
      <ul>{blockers}</ul>
    </div>
    <div class="mini">
      <h3>Attorney Handoff</h3>
      <p>{handoff}</p>
    </div>
  </div>
  <h3>Draft Sections</h3>
  <ul>{sections}</ul>
  <div class="columns">
    <div class="mini">
      <h3>Claim Seeds</h3>
      <ul>{claim_seeds}</ul>
    </div>
    <div class="mini">
      <h3>Drawing Plan</h3>
      <ul>{drawing_plan}</ul>
    </div>
  </div>
  <div class="columns">
    <div class="mini">
      <h3>USPTO Fee Estimate ({entity}, eff. {fee_date})</h3>
      <ul>{fee_rows}</ul>
      <p><strong>Estimated total: ${fee_total:.0} USD</strong></p>
    </div>
    <div class="mini">
      <h3>Claim Audit · abstract {abstract_words}</h3>
      <p>{ind} independent / {dep} dependent / {total} total{multi}</p>
      <ul>{claim_findings}</ul>
    </div>
  </div>
  <h3>Filing Deadlines (today {today})</h3>
  <ul>{deadline_rows}</ul>
  <h3>Search Queries</h3>
  <ul>{search_queries}</ul>
  <h3>Filing Checklist</h3>
  <ul>{checklist}</ul>
</div>"#,
        score = package.readiness.score,
        status = escape_html(&package.readiness.status),
        title = escape_html(&package.title),
        matter_id = escape_html(&package.matter_id),
        track = escape_html(&package.filing_track),
        handoff = escape_html(&package.attorney_handoff.summary),
        entity = escape_html(&fee.entity),
        fee_date = fee.effective_date,
        fee_total = fee.total_usd,
        today = escape_html(&package.deadlines.today),
        ind = package.claim_audit.independent_claims,
        dep = package.claim_audit.dependent_claims,
        total = package.claim_audit.total_claims,
        multi = if package.claim_audit.has_multiple_dependent_claim {
            " · multiple-dependent present"
        } else {
            ""
        },
    )
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn example_request() -> PatentIntakeRequest {
    PatentIntakeRequest {
        request_id: Some("example-patent-package".to_string()),
        schema_version: Some(SCHEMA_VERSION.to_string()),
        title: "Adaptive thermal sensor array".to_string(),
        inventor_names: vec!["Avery Chen".to_string(), "Morgan Patel".to_string()],
        applicant: Some("Example Robotics LLC".to_string()),
        invention_summary: "A distributed sensor array combines low-cost temperature probes, edge calibration, and a controller that changes sampling frequency based on local thermal gradients. Each node reports confidence and drift estimates so the controller can prioritize high-risk zones without flooding the network.".to_string(),
        technical_field: "distributed sensing and thermal control".to_string(),
        problem: "Existing thermal monitoring systems either sample too slowly to catch fast changes or sample every node constantly, which wastes network capacity and power in dense installations.".to_string(),
        solution: "The array estimates local gradients at each node, assigns an adaptive sampling budget, and routes high-confidence alerts through a compact priority protocol while slower regions remain in a low-power cadence.".to_string(),
        novelty_claims: vec![
            "node-level drift confidence changes sampling rates".to_string(),
            "gradient-triggered priority routing reduces bandwidth".to_string(),
            "controller fuses confidence scores with thermal risk zones".to_string(),
        ],
        embodiments: vec![
            "warehouse battery pack monitoring".to_string(),
            "server rack airflow diagnostics".to_string(),
        ],
        alternatives: vec![
            "wireless mesh nodes".to_string(),
            "wired industrial bus nodes".to_string(),
        ],
        advantages: vec![
            "lower power usage".to_string(),
            "reduced telemetry volume".to_string(),
            "faster high-risk thermal alerts".to_string(),
        ],
        public_disclosure_date: None,
        provisional_filing_date: None,
        foreign_priority_date: None,
        target_filing: Some("provisional".to_string()),
        entity_status: Some("micro".to_string()),
        desired_claim_count: Some(8),
        attorney_review: Some(true),
        known_prior_art: vec![KnownPriorArt {
            title: "Static threshold thermal monitoring systems".to_string(),
            url: None,
            notes: None,
        }],
        attachments: vec![
            AttachmentEvidence {
                name: "System block diagram".to_string(),
                kind: Some("figure".to_string()),
                url: None,
                notes: None,
            },
            AttachmentEvidence {
                name: "Sampling-state flow chart".to_string(),
                kind: Some("figure".to_string()),
                url: None,
                notes: None,
            },
        ],
        notes: None,
    }
}

fn security_header_layers() -> [SetResponseHeaderLayer<HeaderValue>; 5] {
    let csp = "default-src 'self'; \
               script-src 'self' https://unpkg.com; \
               style-src 'self' 'unsafe-inline'; \
               img-src 'self' data:; \
               connect-src 'self'; \
               base-uri 'none'; \
               form-action 'self'; \
               frame-ancestors 'none'";
    [
        SetResponseHeaderLayer::overriding(
            HeaderName::from_static("x-content-type-options"),
            HeaderValue::from_static("nosniff"),
        ),
        SetResponseHeaderLayer::overriding(
            HeaderName::from_static("x-frame-options"),
            HeaderValue::from_static("DENY"),
        ),
        SetResponseHeaderLayer::overriding(
            header::REFERRER_POLICY,
            HeaderValue::from_static("no-referrer"),
        ),
        SetResponseHeaderLayer::overriding(
            HeaderName::from_static("cross-origin-opener-policy"),
            HeaderValue::from_static("same-origin"),
        ),
        SetResponseHeaderLayer::overriding(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static(csp),
        ),
    ]
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut stream) => {
                stream.recv().await;
            }
            Err(error) => error!(%error, "failed to install SIGTERM handler"),
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => info!("received SIGINT, beginning graceful shutdown"),
        _ = terminate => info!("received SIGTERM, beginning graceful shutdown"),
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    let _otel = dd_telemetry::init(SERVICE_NAME);
    let host = env_value("HOST", "0.0.0.0");
    let port = env_value("PORT", "8116").parse::<u16>()?;
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(AI_HTTP_TIMEOUT_SECS))
        .user_agent(format!("{SERVICE_NAME}/0.2"))
        // The API client must never follow redirects: reqwest does not strip the
        // custom `x-api-key` header on cross-host redirects, so a redirecting or
        // hijacked base URL could leak the key to another host.
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let config = config_from_env();
    let ai_permits = Arc::new(tokio::sync::Semaphore::new(config.ai_max_concurrency));
    let state = AppState {
        config: Arc::new(config),
        metrics: Arc::new(Metrics::default()),
        store: Arc::new(RwLock::new(PatentStore::default())),
        http,
        ai_permits,
    };

    let [sec0, sec1, sec2, sec3, sec4] = security_header_layers();
    // AI drafting needs a much longer per-request timeout than the deterministic
    // endpoints, so it lives on its own sub-router with its own TimeoutLayer.
    let ai_routes = Router::new()
        .route("/draft/ai", post(draft_ai_json))
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            Duration::from_secs(AI_REQUEST_TIMEOUT_SECS),
        ));
    let fast_routes = Router::new()
        .route("/", get(root))
        .route("/descriptor", get(descriptor))
        .route("/schema", get(schema))
        .route("/example", get(example))
        .route("/matters", get(matters))
        .route("/matters/:matter_id", get(matter))
        .route("/packages/provisional", post(package_json))
        .route("/ui/packages", post(package_form))
        .route("/readiness", post(readiness_json))
        .route("/search/plan", post(search_plan_json))
        .route("/review/package", post(review_package_json))
        .route("/claims/check", post(claims_check_json))
        .route("/fees/estimate", post(fees_estimate_json))
        .route("/deadlines", post(deadlines_json))
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .route("/metrics", get(metrics))
        .route("/docs/api", get(api_docs_html))
        .route("/api/docs", get(api_docs_html))
        .route("/api/docs.json", get(api_docs_json))
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            Duration::from_secs(REQUEST_TIMEOUT_SECS),
        ));
    let app = fast_routes
        .merge(ai_routes)
        .layer(DefaultBodyLimit::max(MAX_HTTP_BODY_BYTES))
        .layer(RequestBodyLimitLayer::new(MAX_HTTP_BODY_BYTES))
        .layer(sec0)
        .layer(sec1)
        .layer(sec2)
        .layer(sec3)
        .layer(sec4)
        .layer(TraceLayer::new_for_http())
        .with_state(state.clone())
        .merge(dd_runtime_config_client::router());

    tokio::spawn(dd_runtime_config_client::register_with_control_plane());

    let addr: SocketAddr = format!("{host}:{port}").parse()?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    info!(
        addr = %addr,
        auth_configured = state.config.server_auth_secret.is_some(),
        allow_unauthenticated = state.config.allow_unauthenticated,
        "{SERVICE_NAME} listening"
    );
    axum::serve(listener, app.layer(dd_telemetry::http_trace_layer()))
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    info!("{SERVICE_NAME} shut down cleanly");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> Config {
        Config {
            server_auth_secret: Some("secret".to_string()),
            allow_unauthenticated: false,
            patent_center_url: "https://patentcenter.uspto.gov/".to_string(),
            max_matters: 10,
            anthropic_api_key: None,
            anthropic_base_url: "https://api.anthropic.com".to_string(),
            ai_model: "claude-opus-4-8".to_string(),
            ai_max_concurrency: 4,
        }
    }

    #[test]
    fn complete_intake_scores_ready_for_attorney_review() {
        let request = example_request();
        let review = evaluate_readiness(&request);
        assert!(review.score >= 82, "score was {}", review.score);
        assert_eq!(review.status, "ready-for-attorney-review");
        assert!(review.blockers.is_empty());
    }

    #[test]
    fn thin_intake_has_blockers() {
        let request = PatentIntakeRequest {
            title: "Idea".to_string(),
            invention_summary: "too short".to_string(),
            problem: "unknown".to_string(),
            solution: "unknown".to_string(),
            ..example_request()
        };
        let mut request = request;
        request.inventor_names.clear();
        request.novelty_claims.clear();
        let review = evaluate_readiness(&request);
        assert!(review.score < 65);
        assert!(review
            .blockers
            .iter()
            .any(|finding| finding.code == "missing-inventors"));
        assert!(review
            .blockers
            .iter()
            .any(|finding| finding.code == "missing-novelty"));
    }

    #[test]
    fn package_contains_claim_seeds_drawings_and_checklist() {
        let config = test_config();
        let package = build_package(&config, example_request()).expect("package");
        assert!(!package.draft.claim_seeds.is_empty());
        assert!(package
            .draft
            .drawing_plan
            .iter()
            .any(|item| item.contains("Figure 1")));
        assert!(package
            .filing_checklist
            .iter()
            .any(|item| item.label == "Patent Center filing handoff"));
    }

    #[test]
    fn package_review_blocks_low_readiness() {
        let state = AppState {
            config: Arc::new(test_config()),
            metrics: Arc::new(Metrics::default()),
            store: Arc::new(RwLock::new(PatentStore::default())),
            http: reqwest::Client::new(),
            ai_permits: Arc::new(tokio::sync::Semaphore::new(4)),
        };
        let response = review_package(
            &state,
            PackageReviewRequest {
                matter_id: None,
                package: Some(PatentMatterPackageInput {
                    readiness_score: Some(40),
                    blocker_count: Some(2),
                    section_count: Some(3),
                    checklist_open_count: Some(3),
                }),
            },
        );
        assert_eq!(response.release_gate, "blocked");
        assert!(response
            .findings
            .iter()
            .any(|finding| finding.severity == "blocker"));
    }

    #[test]
    fn civil_date_add_months_clamps_and_roundtrips() {
        let jan31 = CivilDate::parse("2025-01-31").unwrap();
        assert_eq!(jan31.add_months(1).format(), "2025-02-28");
        let leap = CivilDate::parse("2024-01-31").unwrap();
        assert_eq!(leap.add_months(1).format(), "2024-02-29");
        let d = CivilDate::parse("2025-06-09").unwrap();
        assert_eq!(CivilDate::from_days(d.to_days()), d);
        assert_eq!(d.add_months(12).format(), "2026-06-09");
        assert!(CivilDate::parse("2025-13-01").is_none());
        assert!(CivilDate::parse("2025-02-30").is_none());
    }

    #[test]
    fn fee_scaling_matches_published_2025_schedule() {
        // Large-entity nonprovisional with 5 independent + 25 total claims.
        let large = estimate_fees(Entity::Large, "non-provisional", 25, 5, false);
        // basic 350 + search 770 + exam 880 + 2*600 + 5*200 = 4200
        assert_eq!(large.total_usd as u64, 4200);
        let micro = estimate_fees(Entity::Micro, "non-provisional", 25, 5, false);
        // micro = 20% of each line: 70 + 154 + 176 + 2*120 + 5*40 = 840
        assert_eq!(micro.total_usd as u64, 840);
        let prov = estimate_fees(Entity::Small, "provisional", 30, 9, true);
        assert_eq!(prov.total_usd as u64, 130); // small provisional filing fee only
        assert_eq!(prov.line_items.len(), 1);
    }

    #[test]
    fn deadlines_flag_missed_and_upcoming() {
        let report = analyze_deadlines(Some("2024-01-01"), None, None, Some("2025-06-01"));
        let np = report
            .milestones
            .iter()
            .find(|m| m.code == "nonprovisional-from-provisional")
            .unwrap();
        assert_eq!(np.due_date, "2025-01-01");
        assert!(np.days_remaining < 0);
        assert_eq!(np.status, "past");

        let fresh = analyze_deadlines(Some("2025-05-15"), None, None, Some("2025-06-01"));
        let np = fresh
            .milestones
            .iter()
            .find(|m| m.code == "nonprovisional-from-provisional")
            .unwrap();
        assert_eq!(np.due_date, "2026-05-15");
        assert_eq!(np.status, "ok");
    }

    #[test]
    fn disclosure_warns_about_foreign_rights() {
        let report = analyze_deadlines(None, Some("2025-01-01"), None, Some("2025-06-01"));
        assert!(report
            .milestones
            .iter()
            .any(|m| m.code == "us-grace-period-bar"));
        assert!(report.warnings.iter().any(|w| w.contains("absolute novelty")));
    }

    #[test]
    fn claim_audit_detects_independence_and_bad_dependency() {
        let claims = vec![
            "A widget comprising a frame and a sensor coupled to the frame.".to_string(),
            "The widget of claim 1, wherein the sensor is thermal.".to_string(),
            "The widget of claim 5, wherein the frame is metal.".to_string(),
        ];
        let audit = audit_claims(&claims, None);
        assert_eq!(audit.total_claims, 3);
        assert_eq!(audit.independent_claims, 1);
        assert_eq!(audit.dependent_claims, 2);
        assert!(audit
            .findings
            .iter()
            .any(|f| f.code == "invalid-claim-reference"));
    }

    #[test]
    fn claim_audit_detects_multiple_dependent_and_forward_reference() {
        let claims = vec![
            "A method comprising sensing a value.".to_string(),
            "The method of claim 1, further comprising logging.".to_string(),
            "The method of any of claims 1 or 2, wherein the value is temperature.".to_string(),
        ];
        let audit = audit_claims(&claims, None);
        assert!(audit.has_multiple_dependent_claim);
        assert_eq!(audit.multiple_dependent_claims, 1);
        // Self/forward reference is rejected.
        let bad = vec!["The system of claim 2.".to_string(), "A system.".to_string()];
        let audit = audit_claims(&bad, None);
        assert!(audit
            .findings
            .iter()
            .any(|f| f.code == "improper-claim-dependency"));
    }

    #[test]
    fn dependent_claims_inherit_parent_antecedents() {
        let claims = vec![
            "A gadget comprising a housing and a motor in the housing.".to_string(),
            "The gadget of claim 1, wherein the motor is electric.".to_string(),
            "The gadget of any of claims 1 or 2, wherein the housing is sealed.".to_string(),
        ];
        let audit = audit_claims(&claims, None);
        assert!(
            !audit.findings.iter().any(|f| f.code == "antecedent-basis"),
            "parent-introduced terms must not be flagged in dependent claims: {:?}",
            audit.findings
        );
        // A term that no ancestor introduced is still flagged.
        let novel = vec![
            "A gadget comprising a housing.".to_string(),
            "The gadget of claim 1, wherein the flywheel is balanced.".to_string(),
        ];
        let audit = audit_claims(&novel, None);
        assert!(audit
            .findings
            .iter()
            .any(|f| f.code == "antecedent-basis" && f.message.contains("flywheel")));
    }

    #[test]
    fn validate_intake_rejects_oversized_list_items() {
        let mut request = example_request();
        request.novelty_claims = vec!["x".repeat(MAX_SHORT_TEXT_LEN + 1)];
        let err = validate_intake(&request).unwrap_err();
        assert!(err.contains("noveltyClaims"), "unexpected error: {err}");
        // A normal-sized item passes.
        let mut ok = example_request();
        ok.novelty_claims = vec!["a reasonable novelty point".to_string()];
        assert!(validate_intake(&ok).is_ok());
    }

    #[test]
    fn audit_handles_multibyte_utf8_without_panicking() {
        // parse_claim_dependencies walks byte offsets over the lowercased text and
        // slices `lower[cursor..]`; multibyte UTF-8 next to "claim"/numbers is where
        // a char-boundary panic would surface. This must not panic.
        let claims = vec![
            "A café système comprising a naïve wîdget, 日本語.".to_string(),
            "The système of claim 1, wherein the wîdget café is 設計 — 1 to 3.".to_string(),
            "Claim™ café of any of claims 1 or 2, naïve 日本.".to_string(),
        ];
        let audit = audit_claims(&claims, Some("Abstract with café 日本語 ™ characters."));
        assert_eq!(audit.total_claims, 3);
        let (refs, _) = parse_claim_dependencies("The wîdget café of claims 1–2, 日本語.");
        assert!(refs.len() <= MAX_CLAIMS + 1);
    }

    #[test]
    fn claim_range_expansion_is_bounded() {
        // A huge range parsed from untrusted digits must not blow up.
        let (refs, _) = parse_claim_dependencies("The system of claims 1 to 9999999999.");
        assert!(refs.len() <= MAX_CLAIMS + 1, "refs unbounded: {}", refs.len());
        // The out-of-range endpoint is still recorded so it gets flagged.
        assert!(refs.iter().any(|&r| r > MAX_CLAIMS));
        // And auditing such a claim terminates and flags it.
        let audit = audit_claims(
            &[
                "A system comprising a part.".to_string(),
                "The system of claims 1 to 9999999999.".to_string(),
            ],
            None,
        );
        assert!(audit
            .findings
            .iter()
            .any(|f| f.code == "invalid-claim-reference"));
    }

    #[test]
    fn abstract_over_limit_is_flagged() {
        let long_abstract = "word ".repeat(180);
        let audit = audit_claims(&["A device.".to_string()], Some(&long_abstract));
        assert_eq!(audit.abstract_word_count, Some(180));
        assert!(audit.findings.iter().any(|f| f.code == "abstract-too-long"));
    }

    #[test]
    fn antecedent_basis_flags_unintroduced_term() {
        let findings = antecedent_findings(1, "A device wherein the rotor spins.");
        assert!(findings.iter().any(|f| f.code == "antecedent-basis"));
        // Properly introduced term is not flagged.
        let ok = antecedent_findings(1, "A device comprising a rotor, wherein the rotor spins.");
        assert!(ok.is_empty());
    }

    #[test]
    fn parse_ai_draft_handles_plain_and_fenced_json() {
        let plain = r#"{"abstract":"An abstract.","claims":["A device."],"sections":[{"heading":"Field","body":"..."}]}"#;
        let draft = parse_ai_draft(plain).expect("plain json");
        assert_eq!(draft.abstract_text, "An abstract.");
        assert_eq!(draft.claims.len(), 1);
        assert_eq!(draft.sections.len(), 1);

        let fenced = "```json\n{\"abstract\":\"X\",\"claims\":[\"A method.\"],\"sections\":[]}\n```";
        let draft = parse_ai_draft(fenced).expect("fenced json");
        assert_eq!(draft.claims, vec!["A method.".to_string()]);

        assert!(parse_ai_draft("not json at all").is_err());
    }

    #[test]
    fn ai_output_schema_is_well_formed() {
        let schema = ai_output_schema();
        assert_eq!(schema["additionalProperties"], serde_json::json!(false));
        assert!(schema["properties"]["claims"]["items"]["type"] == "string");
        // Generated drafts feed straight back into the deterministic auditor.
        let draft = AiDraft {
            abstract_text: "An abstract.".to_string(),
            claims: vec![
                "A widget comprising a frame.".to_string(),
                "The widget of claim 1, wherein the frame is metal.".to_string(),
            ],
            sections: vec![],
        };
        let audit = audit_claims(&draft.claims, Some(&draft.abstract_text));
        assert_eq!(audit.independent_claims, 1);
        assert_eq!(audit.dependent_claims, 1);
    }

    #[test]
    fn intake_brief_includes_core_fields() {
        let brief = intake_brief(&example_request());
        assert!(brief.contains("Adaptive thermal sensor array"));
        assert!(brief.contains("Novelty points"));
        assert!(brief.contains("Problem:"));
    }

    #[test]
    fn generated_package_includes_fee_deadline_and_claim_audit() {
        let config = test_config();
        let mut request = example_request();
        request.provisional_filing_date = Some("2025-01-01".to_string());
        let package = build_package(&config, request).expect("package");
        assert_eq!(package.fee_estimate.entity, "micro");
        assert!(package.fee_estimate.total_usd > 0.0);
        assert!(!package.claim_audit.findings.is_empty() || package.claim_audit.total_claims > 0);
        assert!(package
            .deadlines
            .milestones
            .iter()
            .any(|m| m.code == "nonprovisional-from-provisional"));
    }
}
