use std::{collections::BTreeSet, path::Path, sync::Arc};

use reqwest::Url;
use tokio::{fs, process::Command, time::timeout};
use walkdir::WalkDir;

use crate::{
    config::{Config, SCHEMA_VERSION},
    models::{
        AuditRequest, AuditTargetKind, CollectedArtifact, ComplianceStatus, ControlResult,
        ControlStatus, EvidenceItem, EvidenceStatus, Finding, FindingSeverity, StandardResult,
    },
    standards::{
        control_by_id, standard_by_id_or_alias, ControlDef, StandardDef, CONTROL_CATALOG, STANDARDS,
    },
    util::{clean_text, is_private_or_local_url, next_id, normalize_key, now_ms},
};

struct TextCorpus {
    items: Vec<(String, String)>,
    artifacts: Vec<CollectedArtifact>,
    findings: Vec<Finding>,
}

pub fn validate_request(config: &Config, request: &AuditRequest) -> Result<(), String> {
    if let Some(version) = request.schema_version.as_deref() {
        if version.trim() != SCHEMA_VERSION {
            return Err(format!("schemaVersion must be {SCHEMA_VERSION}"));
        }
    }
    if let Some(ids) = request.standard_ids.as_ref() {
        if ids.len() > STANDARDS.len() {
            return Err("standardIds contains more entries than the registry".to_string());
        }
        for id in ids {
            if standard_by_id_or_alias(id).is_none()
                && request
                    .options
                    .as_ref()
                    .and_then(|options| options.fail_on_unknown_standards)
                    .unwrap_or(true)
            {
                return Err(format!("unknown compliance standard: {id}"));
            }
        }
    }
    if let Some(text) = request.target.inline_text.as_deref() {
        if text.len() > config.max_artifact_bytes {
            return Err(format!(
                "target.inlineText must be {} bytes or fewer",
                config.max_artifact_bytes
            ));
        }
    }
    if request.evidence.len() > 10_000 {
        return Err("evidence may contain at most 10000 items".to_string());
    }
    for item in &request.evidence {
        if control_by_id(&item.control_id).is_none() {
            return Err(format!("unknown controlId: {}", item.control_id));
        }
        if let Some(standard_id) = item.standard_id.as_deref() {
            if standard_by_id_or_alias(standard_id).is_none() {
                return Err(format!("unknown evidence.standardId: {standard_id}"));
            }
        }
        if item.description.len() > 4000 {
            return Err("evidence.description must be 4000 bytes or fewer".to_string());
        }
    }
    if let Some(uri) = request.target.uri.as_deref() {
        if uri.len() > 2048 {
            return Err("target.uri must be 2048 bytes or fewer".to_string());
        }
    }
    if let Some(repo_url) = request.target.repo_url.as_deref() {
        validate_repo_url(config, repo_url)?;
    }
    Ok(())
}

pub async fn run_audit(
    config: Arc<Config>,
    http: reqwest::Client,
    request: AuditRequest,
    job_id: String,
) -> Result<crate::models::AuditReport, String> {
    validate_request(&config, &request)?;
    let selected_standards = resolve_standards(&request)?;
    let request_id = request
        .request_id
        .clone()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| next_id("compliance-request", &std::sync::atomic::AtomicU64::new(0)));
    let corpus = collect_target_corpus(&config, http, &request, &job_id).await;
    let mut evidence = request.evidence.clone();
    evidence.extend(infer_evidence_from_text(&corpus.items));

    let mut findings = corpus.findings;
    findings.extend(unknown_or_gap_evidence_findings(&evidence));
    findings.extend(heuristic_findings(&request, &corpus.items));

    let mut standard_results = Vec::new();
    for standard in selected_standards {
        standard_results.push(evaluate_standard(standard, &evidence, &mut findings));
    }

    let max_findings = request
        .options
        .as_ref()
        .and_then(|options| options.max_findings)
        .unwrap_or(config.max_findings_per_job);
    if findings.len() > max_findings {
        findings.truncate(max_findings);
        findings.push(Finding {
            severity: FindingSeverity::Warning,
            standard_id: None,
            control_id: None,
            message: format!("finding list truncated at {max_findings} entries"),
            recommendation: "Narrow standardIds or provide stronger evidence to reduce repeated missing-evidence findings.".to_string(),
            evidence_required: vec![],
        });
    }

    let score = if standard_results.is_empty() {
        0.0
    } else {
        standard_results
            .iter()
            .map(|result| result.score)
            .sum::<f64>()
            / standard_results.len() as f64
    };
    let status = overall_status(score, &standard_results);
    let summary = format!(
        "{} standard(s) evaluated for {} target; {:.1}% evidence coverage.",
        standard_results.len(),
        target_kind_label(request.target.kind),
        score
    );
    Ok(crate::models::AuditReport {
        ok: !matches!(status, ComplianceStatus::NonCompliant),
        request_id,
        schema_version: SCHEMA_VERSION.to_string(),
        target: request.target,
        standards: standard_results
            .iter()
            .map(|result| result.standard_id.clone())
            .collect(),
        score,
        status,
        summary,
        standard_results,
        findings,
        collected_artifacts: corpus.artifacts,
        generated_at_ms: now_ms(),
        notes: vec![
            "Automated readiness assessment only; this is not a regulator, auditor, legal, or certification decision.".to_string(),
            "Provide auditor-reviewed evidence items to turn inferred keyword coverage into durable evidence.".to_string(),
        ],
    })
}

fn resolve_standards(request: &AuditRequest) -> Result<Vec<&'static StandardDef>, String> {
    let Some(ids) = request.standard_ids.as_ref().filter(|ids| !ids.is_empty()) else {
        return Ok(STANDARDS.iter().collect());
    };
    let mut seen = BTreeSet::new();
    let mut selected = Vec::new();
    for id in ids {
        if let Some(standard) = standard_by_id_or_alias(id) {
            if seen.insert(standard.id) {
                selected.push(standard);
            }
        } else if request
            .options
            .as_ref()
            .and_then(|options| options.fail_on_unknown_standards)
            .unwrap_or(true)
        {
            return Err(format!("unknown compliance standard: {id}"));
        }
    }
    Ok(selected)
}

async fn collect_target_corpus(
    config: &Config,
    http: reqwest::Client,
    request: &AuditRequest,
    job_id: &str,
) -> TextCorpus {
    let mut corpus = TextCorpus {
        items: Vec::new(),
        artifacts: Vec::new(),
        findings: Vec::new(),
    };
    if let Some(text) = request.target.inline_text.as_deref() {
        let text = clean_text(text, config.max_artifact_bytes);
        corpus.artifacts.push(CollectedArtifact {
            kind: "inlineText".to_string(),
            source: "target.inlineText".to_string(),
            bytes: text.len(),
            scanned_files: 0,
            notes: vec!["operator-supplied inline text scanned for control keywords".to_string()],
        });
        corpus.items.push(("target.inlineText".to_string(), text));
    }
    if request
        .options
        .as_ref()
        .and_then(|options| options.fetch_external_artifact)
        .unwrap_or(false)
    {
        collect_external_artifact(config, http, request, &mut corpus).await;
    }
    if request.target.kind == AuditTargetKind::Codebase
        && request
            .options
            .as_ref()
            .and_then(|options| options.clone_repo)
            .unwrap_or(false)
    {
        collect_codebase(config, request, job_id, &mut corpus).await;
    }
    corpus
}

/// Read a response body into memory, aborting as soon as it exceeds `max` bytes
/// so a large or unbounded (chunked) body cannot exhaust memory before the size
/// check. Returns `Ok(None)` when the body crosses the limit.
async fn read_bounded_body(
    mut response: reqwest::Response,
    max: usize,
) -> Result<Option<Vec<u8>>, reqwest::Error> {
    let mut buf: Vec<u8> = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if buf.len().saturating_add(chunk.len()) > max {
            return Ok(None);
        }
        buf.extend_from_slice(&chunk);
    }
    Ok(Some(buf))
}

async fn collect_external_artifact(
    config: &Config,
    http: reqwest::Client,
    request: &AuditRequest,
    corpus: &mut TextCorpus,
) {
    let Some(uri) = request.target.uri.as_deref() else {
        corpus.findings.push(policy_finding(
            "target.uri is required when fetchExternalArtifact is true",
            "Set target.uri to an HTTPS artifact URL or disable fetchExternalArtifact.",
        ));
        return;
    };
    if !config.allow_external_fetch {
        corpus.findings.push(policy_finding(
            "external artifact fetch is disabled by COMPLIANCE_ALLOW_EXTERNAL_FETCH=false",
            "Enable the service-level gate only for trusted operator use.",
        ));
        return;
    }
    let Ok(url) = Url::parse(uri) else {
        corpus.findings.push(policy_finding(
            "target.uri is not a valid URL",
            "Provide a valid http or https artifact URL.",
        ));
        return;
    };
    if !matches!(url.scheme(), "https" | "http") {
        corpus.findings.push(policy_finding(
            "target.uri must use http or https",
            "Use an HTTPS artifact URL for external collection.",
        ));
        return;
    }
    if !config.allow_private_targets && is_private_or_local_url(&url) {
        corpus.findings.push(policy_finding(
            "private, local, or in-cluster artifact URL blocked",
            "Set COMPLIANCE_ALLOW_PRIVATE_TARGETS=true only for a deliberately isolated scanner.",
        ));
        return;
    }
    match timeout(config.job_timeout, http.get(url.clone()).send()).await {
        Ok(Ok(response)) => {
            let status = response.status();
            if !status.is_success() {
                corpus.findings.push(policy_finding(
                    &format!("external artifact returned HTTP {status}"),
                    "Publish a reachable evidence artifact or provide inlineText.",
                ));
                return;
            }
            // Fail fast when the server declares an over-limit body...
            if let Some(declared) = response.content_length() {
                if declared > config.max_artifact_bytes as u64 {
                    corpus.findings.push(policy_finding(
                        &format!(
                            "external artifact declared {declared} bytes, above the {} byte scan limit",
                            config.max_artifact_bytes
                        ),
                        "Reduce artifact size or raise COMPLIANCE_MAX_ARTIFACT_BYTES after review.",
                    ));
                    return;
                }
            }
            // ...and bound the streamed read so a chunked/under-declared body cannot
            // exhaust memory: we stop as soon as it crosses the limit.
            match read_bounded_body(response, config.max_artifact_bytes).await {
                Ok(Some(bytes)) => {
                    let text = String::from_utf8_lossy(&bytes).to_string();
                    corpus.artifacts.push(CollectedArtifact {
                        kind: "externalArtifact".to_string(),
                        source: url.to_string(),
                        bytes: bytes.len(),
                        scanned_files: 0,
                        notes: vec!["bounded external artifact fetched and scanned".to_string()],
                    });
                    corpus.items.push((url.to_string(), text));
                }
                Ok(None) => corpus.findings.push(policy_finding(
                    &format!(
                        "external artifact exceeded the {} byte scan limit",
                        config.max_artifact_bytes
                    ),
                    "Reduce artifact size or raise COMPLIANCE_MAX_ARTIFACT_BYTES after review.",
                )),
                Err(error) => corpus.findings.push(policy_finding(
                    &format!("failed to read external artifact body: {error}"),
                    "Retry with a reachable artifact URL or provide inline evidence.",
                )),
            }
        }
        Ok(Err(error)) => corpus.findings.push(policy_finding(
            &format!("failed to fetch external artifact: {error}"),
            "Retry with a reachable artifact URL or provide inline evidence.",
        )),
        Err(_) => corpus.findings.push(policy_finding(
            "external artifact fetch timed out",
            "Use a smaller artifact or provide inline evidence.",
        )),
    }
}

async fn collect_codebase(
    config: &Config,
    request: &AuditRequest,
    job_id: &str,
    corpus: &mut TextCorpus,
) {
    let Some(repo_url) = request.target.repo_url.as_deref() else {
        corpus.findings.push(policy_finding(
            "target.repoUrl is required when cloneRepo is true",
            "Set target.repoUrl to a trusted repository URL or disable cloneRepo.",
        ));
        return;
    };
    if !config.allow_repo_clone {
        corpus.findings.push(policy_finding(
            "repository cloning is disabled by COMPLIANCE_ALLOW_REPO_CLONE=false",
            "Enable the service-level gate only for allowlisted repositories.",
        ));
        return;
    }
    if let Err(error) = validate_repo_url(config, repo_url) {
        corpus.findings.push(policy_finding(
            &error,
            "Use an allowlisted HTTPS or SSH repository URL.",
        ));
        return;
    }
    if let Err(error) = fs::create_dir_all(&config.work_root).await {
        corpus.findings.push(policy_finding(
            &format!("failed to create compliance work root: {error}"),
            "Check COMPLIANCE_WORK_ROOT volume permissions.",
        ));
        return;
    }
    let repo_dir = config.work_root.join(job_id).join("repo");
    let mut command = Command::new(&config.git_bin);
    // Hardening: never block on a credential prompt (fail fast instead), and refuse
    // the local/ext transports outright as defense-in-depth behind the scheme
    // allowlist in validate_repo_url. Submodules and tags are never fetched, so a
    // malicious repo cannot pull in additional remote URLs.
    command
        .env("GIT_TERMINAL_PROMPT", "0")
        .arg("-c")
        .arg("protocol.ext.allow=never")
        .arg("-c")
        .arg("protocol.file.allow=never")
        .arg("clone")
        .arg("--depth")
        .arg("1")
        .arg("--no-recurse-submodules")
        .arg("--no-tags");
    command
        .arg("--filter")
        .arg(format!("blob:limit={}", config.max_file_bytes));
    if let Some(git_ref) = request
        .target
        .git_ref
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        command.arg("--branch").arg(git_ref.trim());
    }
    command.arg(repo_url).arg(&repo_dir);
    match timeout(config.job_timeout, command.output()).await {
        Ok(Ok(output)) if output.status.success() => {
            scan_repo_files(config, &repo_dir, repo_url, corpus).await;
        }
        Ok(Ok(output)) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            corpus.findings.push(policy_finding(
                &format!("git clone failed: {}", clean_text(&stderr, 500)),
                "Confirm repository access and allowed prefixes.",
            ));
        }
        Ok(Err(error)) => corpus.findings.push(policy_finding(
            &format!("failed to start git clone: {error}"),
            "Check COMPLIANCE_GIT_BIN and runtime image git availability.",
        )),
        Err(_) => corpus.findings.push(policy_finding(
            "repository clone timed out",
            "Use a smaller repository or increase COMPLIANCE_JOB_TIMEOUT_SECONDS.",
        )),
    }
}

async fn scan_repo_files(
    config: &Config,
    repo_dir: &Path,
    repo_url: &str,
    corpus: &mut TextCorpus,
) {
    let mut scanned_files = 0usize;
    let mut bytes = 0usize;
    for entry in WalkDir::new(repo_dir)
        .max_depth(8)
        .into_iter()
        .filter_map(Result::ok)
    {
        let path = entry.path();
        if !entry.file_type().is_file()
            || path_has_git_dir(path)
            || !allowed_extension(config, path)
        {
            continue;
        }
        if scanned_files >= config.max_files {
            break;
        }
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if metadata.len() > config.max_file_bytes {
            continue;
        }
        if let Ok(raw) = fs::read(path).await {
            bytes += raw.len();
            scanned_files += 1;
            let source = path
                .strip_prefix(repo_dir)
                .unwrap_or(path)
                .to_string_lossy()
                .to_string();
            corpus
                .items
                .push((source, String::from_utf8_lossy(&raw).to_string()));
        }
    }
    corpus.artifacts.push(CollectedArtifact {
        kind: "codebase".to_string(),
        source: repo_url.to_string(),
        bytes,
        scanned_files,
        notes: vec![
            "repository cloned with shallow depth and blob size filter".to_string(),
            "only allowlisted source and config extensions were scanned".to_string(),
        ],
    });
}

fn path_has_git_dir(path: &Path) -> bool {
    path.components()
        .any(|component| component.as_os_str().to_string_lossy() == ".git")
}

fn allowed_extension(config: &Config, path: &Path) -> bool {
    let file_name = path
        .file_name()
        .map(|value| value.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    if file_name == "dockerfile" || file_name.ends_with(".dockerfile") {
        return true;
    }
    let Some(ext) = path
        .extension()
        .map(|value| normalize_key(&value.to_string_lossy()))
    else {
        return false;
    };
    config
        .allowed_file_extensions
        .iter()
        .any(|allowed| allowed == &ext)
}

fn infer_evidence_from_text(items: &[(String, String)]) -> Vec<EvidenceItem> {
    let mut evidence = Vec::new();
    let mut seen = BTreeSet::new();
    for (source, text) in items {
        let lower = text.to_ascii_lowercase();
        for control in CONTROL_CATALOG {
            for keyword in control.keywords {
                let keyword_lower = keyword.to_ascii_lowercase();
                if lower.contains(&keyword_lower)
                    && seen.insert(format!("{}:{source}:{keyword_lower}", control.id))
                {
                    evidence.push(EvidenceItem {
                        control_id: control.id.to_string(),
                        standard_id: None,
                        description: format!(
                            "Keyword evidence observed for '{}': {keyword}",
                            control.name
                        ),
                        artifact_ref: Some(source.clone()),
                        confidence: Some(0.55),
                        status: Some(EvidenceStatus::Observed),
                    });
                }
            }
        }
    }
    evidence
}

fn evaluate_standard(
    standard: &StandardDef,
    evidence: &[EvidenceItem],
    findings: &mut Vec<Finding>,
) -> StandardResult {
    let mut control_results = Vec::new();
    let mut satisfied = 0usize;
    let mut needs = 0usize;
    let mut failed = 0usize;
    for control_id in standard.controls {
        let control = control_by_id(control_id).unwrap_or_else(|| {
            panic!(
                "standard {} references unknown control {}",
                standard.id, control_id
            )
        });
        let result = evaluate_control(standard, control, evidence);
        match result.status {
            ControlStatus::Satisfied | ControlStatus::NotApplicable => satisfied += 1,
            ControlStatus::NeedsEvidence => {
                needs += 1;
                findings.push(missing_evidence_finding(standard, control));
            }
            ControlStatus::Failed => {
                failed += 1;
                findings.push(failed_control_finding(standard, control));
            }
        }
        control_results.push(result);
    }
    let controls_total = standard.controls.len();
    let score = if controls_total == 0 {
        0.0
    } else {
        (satisfied as f64 / controls_total as f64) * 100.0
    };
    let status = standard_status(score, failed, needs);
    StandardResult {
        standard_id: standard.id.to_string(),
        standard_name: standard.name.to_string(),
        category: standard.category.to_string(),
        jurisdiction: standard.jurisdiction.to_string(),
        score,
        status,
        controls_total,
        controls_satisfied: satisfied,
        controls_needs_evidence: needs,
        controls_failed: failed,
        control_results,
        regulatory_notice: "Readiness evidence only; external auditor, regulator, counsel, or certifying body sign-off is still required where applicable.".to_string(),
    }
}

fn evaluate_control(
    standard: &StandardDef,
    control: &ControlDef,
    evidence: &[EvidenceItem],
) -> ControlResult {
    let matched = evidence
        .iter()
        .filter(|item| evidence_matches(standard, control, item))
        .collect::<Vec<_>>();
    let status = if matched
        .iter()
        .any(|item| item.status == Some(EvidenceStatus::Gap))
    {
        ControlStatus::Failed
    } else if !matched.is_empty()
        && matched
            .iter()
            .all(|item| item.status == Some(EvidenceStatus::NotApplicable))
    {
        ControlStatus::NotApplicable
    } else if matched.iter().any(|item| {
        item.status.unwrap_or(EvidenceStatus::Observed) == EvidenceStatus::Observed
            && item.confidence.unwrap_or(0.5).is_finite()
            && item.confidence.unwrap_or(0.5) >= 0.35
    }) {
        ControlStatus::Satisfied
    } else {
        ControlStatus::NeedsEvidence
    };
    let evidence_refs = matched
        .iter()
        .filter_map(|item| item.artifact_ref.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let rationale = match status {
        ControlStatus::Satisfied => "At least one observed evidence item maps to this control.",
        ControlStatus::NeedsEvidence => "No observed evidence item maps to this required control.",
        ControlStatus::Failed => "A provided evidence item explicitly marks this control as a gap.",
        ControlStatus::NotApplicable => {
            "Evidence marks this control as not applicable for the target."
        }
    }
    .to_string();
    ControlResult {
        control_id: control.id.to_string(),
        name: control.name.to_string(),
        family: control.family.to_string(),
        status,
        evidence_count: matched.len(),
        evidence_refs,
        rationale,
    }
}

fn evidence_matches(standard: &StandardDef, control: &ControlDef, item: &EvidenceItem) -> bool {
    let control_key = normalize_key(&item.control_id).replace('-', "_");
    if control_key != control.id {
        return false;
    }
    let Some(evidence_standard) = item.standard_id.as_deref() else {
        return true;
    };
    standard_by_id_or_alias(evidence_standard).is_some_and(|candidate| candidate.id == standard.id)
}

fn unknown_or_gap_evidence_findings(evidence: &[EvidenceItem]) -> Vec<Finding> {
    let mut findings = Vec::new();
    for item in evidence {
        if item.status == Some(EvidenceStatus::Gap) {
            findings.push(Finding {
                severity: FindingSeverity::Error,
                standard_id: item.standard_id.clone(),
                control_id: Some(item.control_id.clone()),
                message: format!("Evidence marks a gap: {}", item.description),
                recommendation: "Close the stated gap or scope the control as not applicable with reviewer approval.".to_string(),
                evidence_required: vec!["remediation ticket".to_string(), "control owner approval".to_string()],
            });
        }
    }
    findings
}

fn heuristic_findings(request: &AuditRequest, items: &[(String, String)]) -> Vec<Finding> {
    let mut findings = Vec::new();
    for (source, text) in items {
        let lower = text.to_ascii_lowercase();
        for needle in [
            "password=",
            "api_key",
            "secret_key",
            "private_key",
            "allow_unauthenticated=true",
        ] {
            if lower.contains(needle) {
                findings.push(Finding {
                    severity: FindingSeverity::Error,
                    standard_id: None,
                    control_id: Some("access_control".to_string()),
                    message: format!("Potential sensitive token or auth bypass marker '{needle}' found in {source}."),
                    recommendation: "Remove secrets from artifacts/code, rotate exposed credentials, and document secret scanning coverage.".to_string(),
                    evidence_required: vec![
                        "secret scanning report".to_string(),
                        "credential rotation evidence".to_string(),
                    ],
                });
            }
        }
        if request.target.kind == AuditTargetKind::Network
            && lower.contains("0.0.0.0/0")
            && !lower.contains("exception approved")
        {
            findings.push(Finding {
                severity: FindingSeverity::Warning,
                standard_id: None,
                control_id: Some("network_security".to_string()),
                message: format!("Broad ingress or egress marker 0.0.0.0/0 found in {source}."),
                recommendation: "Attach firewall/security-group review evidence or reduce the exposed CIDR range.".to_string(),
                evidence_required: vec!["network rule review".to_string()],
            });
        }
    }
    findings
}

fn missing_evidence_finding(standard: &StandardDef, control: &ControlDef) -> Finding {
    Finding {
        severity: FindingSeverity::Warning,
        standard_id: Some(standard.id.to_string()),
        control_id: Some(control.id.to_string()),
        message: format!("{} needs evidence for {}.", standard.name, control.name),
        recommendation: "Attach a policy, ticket, scan report, runbook, log sample, architectural decision, or control-owner attestation mapped to this control.".to_string(),
        evidence_required: vec![
            control.name.to_string(),
            "control owner and review date".to_string(),
            "artifact reference".to_string(),
        ],
    }
}

fn failed_control_finding(standard: &StandardDef, control: &ControlDef) -> Finding {
    Finding {
        severity: FindingSeverity::Error,
        standard_id: Some(standard.id.to_string()),
        control_id: Some(control.id.to_string()),
        message: format!(
            "{} has an explicit gap for {}.",
            standard.name, control.name
        ),
        recommendation: "Remediate the gap or document accepted risk before claiming readiness."
            .to_string(),
        evidence_required: vec![
            "remediation evidence".to_string(),
            "risk acceptance record".to_string(),
        ],
    }
}

fn policy_finding(message: &str, recommendation: &str) -> Finding {
    Finding {
        severity: FindingSeverity::Warning,
        standard_id: None,
        control_id: None,
        message: message.to_string(),
        recommendation: recommendation.to_string(),
        evidence_required: vec![],
    }
}

fn standard_status(score: f64, failed: usize, needs: usize) -> ComplianceStatus {
    if failed == 0 && needs == 0 && score >= 99.9 {
        ComplianceStatus::EvidenceCompliant
    } else if failed > 0 && score < 70.0 {
        ComplianceStatus::NonCompliant
    } else if score >= 50.0 {
        ComplianceStatus::PartiallyCompliant
    } else {
        ComplianceStatus::NeedsReview
    }
}

fn overall_status(score: f64, results: &[StandardResult]) -> ComplianceStatus {
    if results.is_empty() {
        return ComplianceStatus::NeedsReview;
    }
    if results
        .iter()
        .all(|result| result.status == ComplianceStatus::EvidenceCompliant)
    {
        ComplianceStatus::EvidenceCompliant
    } else if results
        .iter()
        .any(|result| result.status == ComplianceStatus::NonCompliant)
        || score < 35.0
    {
        ComplianceStatus::NonCompliant
    } else if score >= 50.0 {
        ComplianceStatus::PartiallyCompliant
    } else {
        ComplianceStatus::NeedsReview
    }
}

fn target_kind_label(kind: AuditTargetKind) -> &'static str {
    match kind {
        AuditTargetKind::Artifact => "artifact",
        AuditTargetKind::Codebase => "codebase",
        AuditTargetKind::Network => "network",
        AuditTargetKind::System => "system",
    }
}

pub fn validate_repo_url(config: &Config, repo_url: &str) -> Result<(), String> {
    let value = repo_url.trim();
    if value.is_empty() {
        return Err("target.repoUrl must not be empty".to_string());
    }
    if value.len() > 2048 {
        return Err("target.repoUrl must be 2048 bytes or fewer".to_string());
    }
    if value.chars().any(char::is_control) {
        return Err("target.repoUrl must not contain control characters".to_string());
    }
    if !(value.starts_with("https://") || value.starts_with("ssh://") || value.starts_with("git@"))
    {
        return Err("target.repoUrl must use https://, ssh://, or git@".to_string());
    }
    if value.starts_with("https://") || value.starts_with("ssh://") {
        let url = Url::parse(value).map_err(|error| format!("invalid repoUrl: {error}"))?;
        if !config.allow_private_targets && is_private_or_local_url(&url) {
            return Err(
                "target.repoUrl points at a private, local, or in-cluster host".to_string(),
            );
        }
    } else if !config.allow_private_targets {
        // scp-style `git@host:path` skips URL parsing above, so apply the same
        // private/local-host guard by lifting the host into an ssh:// URL.
        if let Some(rest) = value.strip_prefix("git@") {
            let host = rest
                .split(':')
                .next()
                .unwrap_or("")
                .split('/')
                .next()
                .unwrap_or("");
            if host.is_empty() {
                return Err("target.repoUrl is missing a host".to_string());
            }
            if let Ok(url) = Url::parse(&format!("ssh://{host}")) {
                if is_private_or_local_url(&url) {
                    return Err(
                        "target.repoUrl points at a private, local, or in-cluster host".to_string(),
                    );
                }
            }
        }
    }
    if !config.allowed_repo_prefixes.is_empty()
        && !config
            .allowed_repo_prefixes
            .iter()
            .any(|prefix| value.starts_with(prefix))
    {
        return Err(
            "target.repoUrl is not allowed by COMPLIANCE_ALLOWED_REPO_PREFIXES".to_string(),
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{AuditOptions, AuditTarget, EvidenceItem};
    use std::path::PathBuf;

    fn test_config() -> Config {
        Config {
            host: "127.0.0.1".to_string(),
            port: 8118,
            work_root: PathBuf::from("/tmp/dd-compliance-rs-test"),
            server_auth_secret: Some("secret".to_string()),
            allow_unauthenticated: false,
            allow_external_fetch: false,
            allow_repo_clone: false,
            allow_private_targets: false,
            allowed_repo_prefixes: vec![],
            allowed_file_extensions: vec!["rs".to_string(), "md".to_string()],
            git_bin: "git".to_string(),
            job_timeout: std::time::Duration::from_secs(5),
            max_jobs: 20,
            max_concurrent_jobs: 2,
            max_http_body_bytes: 1024 * 1024,
            max_artifact_bytes: 1024 * 1024,
            max_files: 100,
            max_file_bytes: 1024 * 1024,
            max_findings_per_job: 200,
            max_concurrent_analyses: 4,
        }
    }

    #[tokio::test]
    async fn audit_scores_startup_stack_across_requested_standards() {
        let request = crate::models::example_request();
        let report = run_audit(
            Arc::new(test_config()),
            reqwest::Client::new(),
            request,
            "job-test".to_string(),
        )
        .await
        .expect("audit");
        assert_eq!(report.schema_version, SCHEMA_VERSION);
        assert!(report.standards.contains(&"soc-2".to_string()));
        assert!(report.standards.contains(&"gdpr".to_string()));
        assert!(report.score > 40.0);
        assert!(!report.standard_results.is_empty());
    }

    #[test]
    fn explicit_gap_fails_matching_control() {
        let standard = standard_by_id_or_alias("soc-2").unwrap();
        let mut findings = Vec::new();
        let evidence = vec![EvidenceItem {
            control_id: "access_control".to_string(),
            standard_id: Some("soc-2".to_string()),
            description: "No access reviews exist.".to_string(),
            artifact_ref: None,
            confidence: Some(1.0),
            status: Some(EvidenceStatus::Gap),
        }];
        let result = evaluate_standard(standard, &evidence, &mut findings);
        assert!(result.controls_failed >= 1);
        assert!(findings
            .iter()
            .any(|finding| finding.control_id.as_deref() == Some("access_control")));
    }

    #[test]
    fn validate_blocks_private_repo_by_default() {
        let config = test_config();
        let error = validate_repo_url(&config, "https://localhost/repo.git").unwrap_err();
        assert!(error.contains("private"));
    }

    #[test]
    fn validate_blocks_scp_syntax_private_host() {
        let config = test_config();
        // scp-style git@ URLs skip URL parsing; the host must still be SSRF-checked.
        let error = validate_repo_url(&config, "git@169.254.169.254:org/repo.git").unwrap_err();
        assert!(error.contains("private"), "got {error}");
        let error = validate_repo_url(&config, "git@10.0.0.5:org/repo.git").unwrap_err();
        assert!(error.contains("private"), "got {error}");
        // A public scp-style host remains acceptable.
        assert!(validate_repo_url(&config, "git@github.com:org/repo.git").is_ok());
    }

    #[test]
    fn validates_unknown_standard() {
        let mut request = AuditRequest {
            request_id: None,
            schema_version: Some(SCHEMA_VERSION.to_string()),
            standard_ids: Some(vec!["not-real".to_string()]),
            target: AuditTarget {
                kind: AuditTargetKind::Artifact,
                name: None,
                uri: None,
                repo_url: None,
                git_ref: None,
                inline_text: None,
                tags: vec![],
            },
            evidence: vec![],
            options: Some(AuditOptions {
                fail_on_unknown_standards: Some(true),
                ..AuditOptions::default()
            }),
        };
        assert!(validate_request(&test_config(), &request).is_err());
        request.options = Some(AuditOptions {
            fail_on_unknown_standards: Some(false),
            ..AuditOptions::default()
        });
        assert!(validate_request(&test_config(), &request).is_ok());
    }
}
