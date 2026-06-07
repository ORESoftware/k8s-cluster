use std::{
    collections::BTreeSet,
    env,
    error::Error,
    net::SocketAddr,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, RwLock,
    },
    time::{SystemTime, UNIX_EPOCH},
};

use axum::{
    extract::{DefaultBodyLimit, Form, Path, State},
    http::{HeaderMap, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::json;

const SERVICE_NAME: &str = "dd-patent-filing-rs";
const SCHEMA_VERSION: &str = "patent_filing.package.v1";
const MAX_HTTP_BODY_BYTES: usize = 1024 * 1024;
const MAX_MATTERS_DEFAULT: usize = 200;
const MAX_TEXT_LEN: usize = 24_000;
const MAX_SHORT_TEXT_LEN: usize = 1_000;
const MAX_LIST_ITEMS: usize = 64;
const MAX_TOKEN_LEN: usize = 160;

#[derive(Clone)]
struct AppState {
    config: Arc<Config>,
    metrics: Arc<Metrics>,
    store: Arc<RwLock<PatentStore>>,
}

#[derive(Clone)]
struct Config {
    server_auth_secret: Option<String>,
    allow_unauthenticated: bool,
    patent_center_url: String,
    max_matters: usize,
}

#[derive(Default)]
struct Metrics {
    http_requests_total: AtomicU64,
    package_requests_total: AtomicU64,
    readiness_requests_total: AtomicU64,
    search_plan_requests_total: AtomicU64,
    package_reviews_total: AtomicU64,
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
    target_filing: Option<String>,
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

#[derive(Debug, Clone, Serialize)]
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
    target_filing: Option<String>,
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
        target_filing: clean_optional(form.target_filing, 64),
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
            "filing-checklist.md".to_string(),
        ],
    }
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
    let mut warnings = readiness
        .warnings
        .iter()
        .map(|finding| finding.message.clone())
        .collect::<Vec<_>>();
    warnings.push("This package is preparation support only; it does not file with the USPTO or replace legal advice.".to_string());
    let matter_id = format!("pf-{}-{generated_at_ms}", slugify(&request.title));
    Ok(PatentMatterPackage {
        ok: true,
        matter_id,
        request_id,
        schema_version: SCHEMA_VERSION,
        generated_at_ms,
        filing_track: normalize_track(request.target_filing.as_ref()),
        title: request.title,
        applicant: request.applicant,
        inventor_names: request.inventor_names,
        readiness,
        draft,
        search_plan,
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
            "docs": "/docs/api"
        },
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
            "targetFiling": "provisional|non-provisional|design|pct",
            "knownPriorArt": [{ "title": "string", "url": "string?", "notes": "string?" }],
            "attachments": [{ "name": "string", "kind": "string?", "url": "string?", "notes": "string?" }]
        },
        "response": {
            "readiness": "score, blockers, warnings, strengths, nextActions",
            "draft": "abstract, sections, claimSeeds, drawingPlan",
            "searchPlan": "queries, sources, classificationHints",
            "filingChecklist": "operator handoff checklist"
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
        "patentCenterUrl": state.config.patent_center_url
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
  <script src="https://unpkg.com/htmx.org@1.9.12"></script>
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
          <label>Public disclosure date
            <input name="public_disclosure_date" placeholder="YYYY-MM-DD">
          </label>
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
</html>"##
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
        target_filing: Some("provisional".to_string()),
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

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    let host = env_value("HOST", "0.0.0.0");
    let port = env_value("PORT", "8116").parse::<u16>()?;
    let state = AppState {
        config: Arc::new(config_from_env()),
        metrics: Arc::new(Metrics::default()),
        store: Arc::new(RwLock::new(PatentStore::default())),
    };

    let app = Router::new()
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
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .route("/metrics", get(metrics))
        .route("/docs/api", get(api_docs_html))
        .route("/api/docs", get(api_docs_html))
        .route("/api/docs.json", get(api_docs_json))
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
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let config = Config {
            server_auth_secret: Some("secret".to_string()),
            allow_unauthenticated: false,
            patent_center_url: "https://patentcenter.uspto.gov/".to_string(),
            max_matters: 10,
        };
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
            config: Arc::new(Config {
                server_auth_secret: Some("secret".to_string()),
                allow_unauthenticated: false,
                patent_center_url: "https://patentcenter.uspto.gov/".to_string(),
                max_matters: 10,
            }),
            metrics: Arc::new(Metrics::default()),
            store: Arc::new(RwLock::new(PatentStore::default())),
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
}
