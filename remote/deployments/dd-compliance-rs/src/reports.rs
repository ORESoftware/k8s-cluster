use std::sync::{atomic::AtomicU64, Arc};

use base64::{engine::general_purpose, Engine as _};

use crate::{
    audit::run_audit,
    config::{Config, SCHEMA_VERSION},
    diagrams::generate_infrastructure_diagram,
    models::{
        DiagramRequest, DiagramSource, SystemReport, SystemReportRequest, VulnerabilityFinding,
        VulnerabilityScanReport, VulnerabilityScanRequest, VulnerabilitySeverity,
    },
    util::{clean_text, next_id, now_ms},
};

static SYSTEM_REPORT_AUDIT_IDS: AtomicU64 = AtomicU64::new(0);

pub async fn generate_system_report(
    config: Arc<Config>,
    http: reqwest::Client,
    request: SystemReportRequest,
) -> SystemReport {
    let request_id = request
        .request_id
        .clone()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| format!("system-report-{}", now_ms()));
    let title = request
        .title
        .clone()
        .or_else(|| {
            request
                .system_name
                .as_ref()
                .map(|name| format!("{name} compliance report"))
        })
        .unwrap_or_else(|| "Compliance system report".to_string());
    let options = request.options.clone().unwrap_or_default();
    let include_vulnerability_scan = options.include_vulnerability_scan.unwrap_or(true);
    let include_diagram = options.include_diagram.unwrap_or(true);
    let include_markdown = options.include_markdown.unwrap_or(true);
    let include_pdf = options.include_pdf.unwrap_or(true);

    let audit_report = if let Some(audit_request) = request.audit.clone() {
        run_audit(
            config.clone(),
            http.clone(),
            audit_request,
            next_id("system-report-audit", &SYSTEM_REPORT_AUDIT_IDS),
        )
        .await
        .ok()
    } else {
        None
    };

    let vulnerability_scan = if include_vulnerability_scan {
        Some(scan_vulnerabilities(
            config.as_ref(),
            VulnerabilityScanRequest {
                request_id: Some(format!("{request_id}-vuln")),
                title: Some(title.clone()),
                artifacts: request.artifacts.clone(),
                inline_text: request.inline_text.clone(),
            },
        ))
    } else {
        None
    };

    let diagram = if include_diagram {
        let diagram_request = request.diagram.clone().or_else(|| {
            if request.artifacts.is_empty() {
                None
            } else {
                Some(DiagramRequest {
                    request_id: Some(format!("{request_id}-diagram")),
                    title: Some(format!("{title} infrastructure")),
                    terraform: vec![],
                    gitops: request.artifacts.clone(),
                    live: vec![],
                    nodes: vec![],
                    edges: vec![],
                    options: Some(crate::models::DiagramOptions {
                        include_local_mermaid: Some(true),
                    }),
                })
            }
        });
        if let Some(diagram_request) = diagram_request {
            Some(generate_infrastructure_diagram(diagram_request).await)
        } else {
            None
        }
    } else {
        None
    };

    let markdown_body = render_markdown_report(
        &title,
        request.system_name.as_deref(),
        request.description.as_deref(),
        audit_report.as_ref(),
        vulnerability_scan.as_ref(),
        diagram.as_ref(),
    );
    let (pdf_base64, pdf_bytes) = if include_pdf {
        let bytes = markdown_to_pdf(&title, &markdown_body);
        (
            Some(general_purpose::STANDARD.encode(&bytes)),
            Some(bytes.len()),
        )
    } else {
        (None, None)
    };
    let ok = audit_report
        .as_ref()
        .map(|report| report.ok)
        .unwrap_or(true)
        && vulnerability_scan
            .as_ref()
            .map(|scan| scan.ok)
            .unwrap_or(true)
        && diagram.as_ref().map(|diagram| diagram.ok).unwrap_or(true);
    SystemReport {
        ok,
        request_id,
        schema_version: SCHEMA_VERSION.to_string(),
        title,
        markdown: include_markdown.then_some(markdown_body),
        pdf_base64,
        pdf_bytes,
        vulnerability_scan,
        diagram,
        generated_at_ms: now_ms(),
        notes: vec![
            "Markdown and PDF are generated from supplied evidence; they are not a substitute for legal, auditor, or regulator review.".to_string(),
            "Vulnerability scanning is bounded static analysis over provided artifacts and should be paired with dedicated SCA, SAST, DAST, and cloud posture tools.".to_string(),
        ],
    }
}

pub fn scan_vulnerabilities(
    config: &Config,
    request: VulnerabilityScanRequest,
) -> VulnerabilityScanReport {
    let request_id = request
        .request_id
        .clone()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| format!("vuln-scan-{}", now_ms()));
    let mut findings = Vec::new();
    let mut scanned_bytes = 0usize;
    if let Some(text) = request.inline_text.as_deref() {
        let cleaned = clean_text(text, config.max_artifact_bytes);
        scanned_bytes += cleaned.len();
        scan_text("inlineText", &cleaned, &mut findings);
    }
    for artifact in request.artifacts {
        let name = artifact.name.unwrap_or_else(|| "artifact".to_string());
        let cleaned = clean_text(&artifact.content, config.max_artifact_bytes);
        scanned_bytes += cleaned.len();
        scan_text(&name, &cleaned, &mut findings);
    }
    findings.sort_by(|left, right| {
        right
            .severity
            .cmp(&left.severity)
            .then(left.id.cmp(&right.id))
    });
    findings.dedup_by(|left, right| left.id == right.id && left.evidence_ref == right.evidence_ref);
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
    VulnerabilityScanReport {
        ok: high_or_worse == 0,
        request_id,
        schema_version: SCHEMA_VERSION.to_string(),
        summary,
        scanned_bytes,
        findings,
        generated_at_ms: now_ms(),
        notes: vec![
            "Static heuristic scan only; submit SBOMs, dependency manifests, IaC, Kubernetes YAML, and scanner outputs for stronger evidence.".to_string(),
        ],
    }
}

fn scan_text(evidence_ref: &str, text: &str, findings: &mut Vec<VulnerabilityFinding>) {
    let lower = text.to_ascii_lowercase();
    let checks = [
        (
            "secret-material",
            VulnerabilitySeverity::Critical,
            "secrets",
            ["akia", "begin rsa private key", "private_key", "secret_access_key"].as_slice(),
            "Potential secret or private key material appears in submitted evidence.",
            "Rotate exposed credentials, remove them from source/history, and move secrets to AWS Secrets Manager or External Secrets.",
        ),
        (
            "plaintext-secret",
            VulnerabilitySeverity::High,
            "secrets",
            ["password=", "password:", "token=", "api_key", "apikey"].as_slice(),
            "Potential plaintext password, token, or API key marker appears in submitted evidence.",
            "Replace inline credentials with secret references and verify the value was not committed.",
        ),
        (
            "public-network-exposure",
            VulnerabilitySeverity::High,
            "network",
            ["0.0.0.0/0", "::/0", "nodeport", "loadbalancer"].as_slice(),
            "Potential broad public network exposure appears in infrastructure evidence.",
            "Restrict ingress with least-privilege CIDRs, gateway auth, security groups, and NetworkPolicy.",
        ),
        (
            "privileged-workload",
            VulnerabilitySeverity::High,
            "kubernetes",
            ["privileged: true", "allowprivilegeescalation: true", "runasuser: 0"].as_slice(),
            "Potential privileged Kubernetes workload configuration appears in evidence.",
            "Run as non-root, drop Linux capabilities, disable privilege escalation, and use RuntimeDefault seccomp.",
        ),
        (
            "weak-workload-hardening",
            VulnerabilitySeverity::Medium,
            "kubernetes",
            ["readonlyrootfilesystem: false", "automountserviceaccounttoken: true"].as_slice(),
            "Workload hardening appears weaker than the cluster production posture.",
            "Use read-only root filesystems where possible and disable service account token automount unless required.",
        ),
        (
            "insecure-transport",
            VulnerabilitySeverity::Medium,
            "transport",
            ["tls_insecure", "insecure_skip_verify", "ssl_verify=false", "http://"].as_slice(),
            "Potential insecure transport or disabled TLS verification appears in evidence.",
            "Prefer HTTPS/TLS with verification enabled; document any internal plaintext exception and isolate it with NetworkPolicy.",
        ),
    ];
    for (id, severity, category, needles, message, recommendation) in checks {
        if needles.iter().any(|needle| lower.contains(needle)) {
            findings.push(VulnerabilityFinding {
                id: id.to_string(),
                severity,
                category: category.to_string(),
                evidence_ref: evidence_ref.to_string(),
                message: message.to_string(),
                recommendation: recommendation.to_string(),
            });
        }
    }
}

fn render_markdown_report(
    title: &str,
    system_name: Option<&str>,
    description: Option<&str>,
    audit: Option<&crate::models::AuditReport>,
    scan: Option<&VulnerabilityScanReport>,
    diagram: Option<&crate::models::DiagramReport>,
) -> String {
    let mut out = String::new();
    out.push_str("# ");
    out.push_str(title);
    out.push_str("\n\n");
    if let Some(system_name) = system_name {
        out.push_str("- System: ");
        out.push_str(system_name);
        out.push('\n');
    }
    if let Some(description) = description {
        out.push_str("- Description: ");
        out.push_str(description);
        out.push('\n');
    }
    out.push_str("- Generated by: dd-compliance-rs\n\n");
    if let Some(audit) = audit {
        out.push_str("## Compliance Summary\n\n");
        out.push_str(&format!(
            "- Status: {:?}\n- Score: {:.1}%\n- Standards: {}\n- Findings: {}\n\n",
            audit.status,
            audit.score,
            audit.standards.join(", "),
            audit.findings.len()
        ));
    }
    if let Some(scan) = scan {
        out.push_str("## Vulnerability Scan\n\n");
        out.push_str(&format!("- {}\n\n", scan.summary));
        for finding in &scan.findings {
            out.push_str(&format!(
                "- {:?}: {} ({})\n  Recommendation: {}\n",
                finding.severity, finding.message, finding.evidence_ref, finding.recommendation
            ));
        }
        out.push('\n');
    }
    if let Some(diagram) = diagram {
        out.push_str("## Infrastructure Diagram\n\n");
        out.push_str(&format!("- {}\n\n", diagram.summary));
        if let Some(artifact) = diagram
            .diagrams
            .iter()
            .find(|item| item.format == "mermaid")
        {
            out.push_str("```mermaid\n");
            out.push_str(&artifact.content);
            out.push_str("```\n\n");
        }
    }
    out.push_str("## Notes\n\n");
    out.push_str("- Automated readiness evidence only; review with qualified security, legal, and compliance owners.\n");
    out
}

fn markdown_to_pdf(title: &str, markdown: &str) -> Vec<u8> {
    let mut lines = vec![title.to_string()];
    lines.extend(
        markdown
            .lines()
            .map(|line| line.replace('#', "").trim().to_string()),
    );
    let mut content = String::from("BT\n/F1 10 Tf\n50 780 Td\n12 TL\n");
    for line in wrap_lines(&lines.join("\n"), 92).into_iter().take(58) {
        content.push('(');
        content.push_str(&pdf_escape(&line));
        content.push_str(") Tj\nT*\n");
    }
    content.push_str("ET\n");
    let objects = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R >>".to_string(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
        format!("<< /Length {} >>\nstream\n{}endstream", content.len(), content),
    ];
    let mut pdf = Vec::new();
    pdf.extend_from_slice(b"%PDF-1.4\n");
    let mut offsets = Vec::new();
    for (index, object) in objects.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.extend_from_slice(format!("{} 0 obj\n{}\nendobj\n", index + 1, object).as_bytes());
    }
    let xref = pdf.len();
    pdf.extend_from_slice(
        format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes(),
    );
    for offset in offsets {
        pdf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    pdf.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{}\n%%EOF\n",
            objects.len() + 1,
            xref
        )
        .as_bytes(),
    );
    pdf
}

fn wrap_lines(value: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    for raw in value.lines() {
        let mut line = String::new();
        for word in raw.split_whitespace() {
            if !line.is_empty() && line.len() + word.len() + 1 > width {
                lines.push(line);
                line = String::new();
            }
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(word);
        }
        if line.is_empty() {
            lines.push(String::new());
        } else {
            lines.push(line);
        }
    }
    lines
}

fn pdf_escape(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii() && !ch.is_control() {
                ch
            } else {
                ' '
            }
        })
        .collect::<String>()
        .replace('\\', "\\\\")
        .replace('(', "\\(")
        .replace(')', "\\)")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use std::path::PathBuf;

    fn test_config() -> Config {
        Config {
            host: "127.0.0.1".to_string(),
            port: 8118,
            work_root: PathBuf::from("/tmp/dd-compliance-rs-report-test"),
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
        }
    }

    #[test]
    fn vulnerability_scan_flags_dangerous_infra_evidence() {
        let report = scan_vulnerabilities(
            &test_config(),
            VulnerabilityScanRequest {
                request_id: None,
                title: None,
                artifacts: vec![DiagramSource {
                    name: Some("deployment.yaml".to_string()),
                    content: "allowPrivilegeEscalation: true\ncidr_blocks = [\"0.0.0.0/0\"]"
                        .to_string(),
                }],
                inline_text: None,
            },
        );
        assert!(!report.ok);
        assert!(report
            .findings
            .iter()
            .any(|item| item.id == "privileged-workload"));
        assert!(report
            .findings
            .iter()
            .any(|item| item.id == "public-network-exposure"));
    }

    #[test]
    fn markdown_pdf_output_is_valid_pdf_bytes() {
        let bytes = markdown_to_pdf("Title", "# Title\n\nA report line.");
        assert!(bytes.starts_with(b"%PDF-1.4"));
        assert!(bytes.ends_with(b"%%EOF\n"));
    }
}
