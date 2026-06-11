//! Bounded behavioral analyzers: transaction fraud screening, automated-traffic
//! (bot) detection, and login anomaly detection. Each accepts a batch of
//! structured records and emits deterministic per-record risk findings plus an
//! aggregate score. Scoring is self-contained — no external reputation or
//! geolocation services are consulted; callers supply any reference data.

use std::collections::HashMap;

use crate::{
    config::{Config, SCHEMA_VERSION},
    models::{
        BotDetectionRequest, FraudDetectionRequest, LoginAnomalyRequest, RiskAnalysisReport,
        RiskFinding, VulnerabilitySeverity,
    },
    util::{clip, now_ms},
};

/// Maximum characters of a caller-controlled identifier echoed into a finding.
const MAX_SUBJECT_CHARS: usize = 200;

/// Resolve a record's caller-supplied id (sanitized) or fall back to its index.
fn subject_ref(id: &Option<String>, index: usize) -> String {
    id.as_deref()
        .map(|value| clip(value, MAX_SUBJECT_CHARS))
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| format!("event-{index}"))
}

/// True when `email`'s domain is, or is a subdomain of, a disposable domain.
/// A plain suffix check would wrongly flag e.g. `user@notmailinator.com`.
fn is_disposable_email(email: &str) -> bool {
    let Some(domain) = email.rsplit('@').next() else {
        return false;
    };
    DISPOSABLE_EMAIL_DOMAINS
        .iter()
        .any(|d| domain == *d || domain.ends_with(&format!(".{d}")))
}

const DISPOSABLE_EMAIL_DOMAINS: &[&str] = &[
    "mailinator.com",
    "guerrillamail.com",
    "10minutemail.com",
    "tempmail.com",
    "trashmail.com",
    "yopmail.com",
    "getnada.com",
    "sharklasers.com",
];

const BOT_USER_AGENT_MARKERS: &[&str] = &[
    "bot",
    "crawler",
    "spider",
    "curl/",
    "wget",
    "python-requests",
    "python-urllib",
    "go-http-client",
    "java/",
    "libwww",
    "okhttp",
    "scrapy",
    "headlesschrome",
    "phantomjs",
    "puppeteer",
    "selenium",
];

fn severity_for_score(score: u32) -> VulnerabilitySeverity {
    match score {
        0..=19 => VulnerabilitySeverity::Info,
        20..=39 => VulnerabilitySeverity::Low,
        40..=64 => VulnerabilitySeverity::Medium,
        65..=84 => VulnerabilitySeverity::High,
        _ => VulnerabilitySeverity::Critical,
    }
}

fn finalize(
    config: &Config,
    request_id: Option<String>,
    analysis_type: &str,
    id_prefix: &str,
    events_analyzed: usize,
    mut findings: Vec<RiskFinding>,
    mut notes: Vec<String>,
) -> RiskAnalysisReport {
    let request_id = request_id
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| format!("{id_prefix}-{}", now_ms()));
    findings.sort_by(|left, right| {
        right
            .score
            .cmp(&left.score)
            .then(left.subject_ref.cmp(&right.subject_ref))
            .then(left.id.cmp(&right.id))
    });
    // Peak risk is computed before any truncation so the headline score still
    // reflects the worst event even if low-score findings are dropped.
    let risk_score = findings.iter().map(|f| f.score).max().unwrap_or(0);
    if findings.len() > config.max_findings_per_job {
        findings.truncate(config.max_findings_per_job);
        notes.push(format!(
            "Findings truncated to the configured maximum of {}.",
            config.max_findings_per_job
        ));
    }
    let high_or_worse = findings
        .iter()
        .filter(|f| f.severity >= VulnerabilitySeverity::High)
        .count();
    let summary = format!(
        "{} finding(s) across {} event(s); peak risk {}; {} high-or-worse.",
        findings.len(),
        events_analyzed,
        risk_score,
        high_or_worse
    );
    RiskAnalysisReport {
        ok: high_or_worse == 0,
        request_id,
        analysis_type: analysis_type.to_string(),
        schema_version: SCHEMA_VERSION.to_string(),
        summary,
        events_analyzed,
        risk_score,
        findings,
        generated_at_ms: now_ms(),
        notes,
    }
}

/// Truncate an event batch to `config.max_files`; the flag reports whether any
/// events were dropped so callers can be told their batch was not fully analyzed.
fn cap_events<T>(config: &Config, mut events: Vec<T>) -> (Vec<T>, bool) {
    let truncated = events.len() > config.max_files;
    if truncated {
        events.truncate(config.max_files);
    }
    (events, truncated)
}

fn base_notes(note: &str, truncated: bool) -> Vec<String> {
    let mut notes = vec![note.to_string()];
    if truncated {
        notes.push(
            "Some submitted events were skipped because the per-request event limit was reached; resubmit in smaller batches for complete coverage."
                .to_string(),
        );
    }
    notes
}

// ---------------------------------------------------------------------------
// Fraud detection
// ---------------------------------------------------------------------------

pub fn detect_fraud(config: &Config, request: FraudDetectionRequest) -> RiskAnalysisReport {
    let high_amount = request.high_amount_threshold.unwrap_or(1000.0).max(0.0);
    let blocklist: Vec<String> = request
        .blocklisted_ips
        .iter()
        .take(config.max_files)
        .map(|ip| ip.trim().to_ascii_lowercase())
        .filter(|ip| !ip.is_empty())
        .collect();
    let (events, truncated) = cap_events(config, request.events);
    let events_analyzed = events.len();

    // Velocity: count events per email and per ip across the batch.
    let mut email_counts: HashMap<String, u32> = HashMap::new();
    let mut ip_counts: HashMap<String, u32> = HashMap::new();
    for event in &events {
        if let Some(email) = normalized(&event.email) {
            *email_counts.entry(email).or_default() += 1;
        }
        if let Some(ip) = normalized(&event.ip) {
            *ip_counts.entry(ip).or_default() += 1;
        }
    }

    let mut findings = Vec::new();
    for (index, event) in events.iter().enumerate() {
        let subject = subject_ref(&event.id, index);
        let mut score = 0u32;
        let mut reasons: Vec<String> = Vec::new();

        if let Some(amount) = event.amount {
            if amount >= high_amount {
                score += 25;
                reasons.push(format!("high-value amount {amount:.2}"));
            }
        }
        if let Some(age) = event.account_age_days {
            if age < 1.0 {
                score += 20;
                reasons.push("account is less than a day old".to_string());
            }
            if age < 1.0 && event.amount.unwrap_or(0.0) >= high_amount {
                score += 15;
                reasons.push("high-value purchase on a brand-new account".to_string());
            }
        }
        if mismatched(&event.ip_country, &event.billing_country) {
            score += 20;
            reasons.push("IP country differs from billing country".to_string());
        }
        if mismatched(&event.card_bin_country, &event.ip_country) {
            score += 15;
            reasons.push("card BIN country differs from IP country".to_string());
        }
        if let Some(email) = normalized(&event.email) {
            if is_disposable_email(&email) {
                score += 20;
                reasons.push("disposable email domain".to_string());
            }
            if email_counts.get(&email).copied().unwrap_or(0) >= 3 {
                score += 15;
                reasons.push("repeated transactions from the same email in this batch".to_string());
            }
        }
        if let Some(ip) = normalized(&event.ip) {
            if blocklist.contains(&ip) {
                score += 40;
                reasons.push("IP is on the caller-supplied blocklist".to_string());
            }
            if ip_counts.get(&ip).copied().unwrap_or(0) >= 4 {
                score += 15;
                reasons.push("high transaction velocity from the same IP".to_string());
            }
        }
        if event.prior_chargebacks.unwrap_or(0) > 0 {
            score += 15;
            reasons.push("account has prior chargebacks".to_string());
        }

        if score == 0 {
            continue;
        }
        let score = score.min(100);
        findings.push(RiskFinding {
            id: "fraud-risk".to_string(),
            severity: severity_for_score(score),
            category: "fraud".to_string(),
            subject_ref: subject,
            score,
            message: format!("Transaction fraud signals: {}.", reasons.join("; ")),
            recommendation: "Step up verification (3-DS, manual review, or hold) before settlement and feed the outcome back into your scoring model.".to_string(),
        });
    }

    finalize(
        config,
        request.request_id,
        "fraud",
        "fraud-detection",
        events_analyzed,
        findings,
        base_notes(
            "Heuristic rule scoring over submitted transactions; calibrate thresholds and combine with a trained model and device intelligence.",
            truncated,
        ),
    )
}

// ---------------------------------------------------------------------------
// Bot detection
// ---------------------------------------------------------------------------

pub fn detect_bots(config: &Config, request: BotDetectionRequest) -> RiskAnalysisReport {
    let rate_threshold = request.rate_threshold_per_min.unwrap_or(120.0).max(0.0);
    let honeypots: Vec<String> = request
        .honeypot_paths
        .iter()
        .take(config.max_files)
        .map(|path| path.trim().to_ascii_lowercase())
        .filter(|path| !path.is_empty())
        .collect();
    let (events, truncated) = cap_events(config, request.events);
    let events_analyzed = events.len();

    let mut findings = Vec::new();
    for (index, event) in events.iter().enumerate() {
        let subject = subject_ref(&event.id, index);
        let mut score = 0u32;
        let mut reasons: Vec<String> = Vec::new();

        match normalized(&event.user_agent) {
            None => {
                score += 35;
                reasons.push("missing User-Agent".to_string());
            }
            Some(ua) => {
                if let Some(marker) = BOT_USER_AGENT_MARKERS
                    .iter()
                    .find(|marker| ua.contains(**marker))
                {
                    score += 40;
                    reasons.push(format!("automated client User-Agent (`{marker}`)"));
                }
            }
        }
        if let Some(rate) = event.requests_per_min {
            if rate >= rate_threshold {
                score += 30;
                reasons.push(format!("request rate {rate:.0}/min exceeds threshold {rate_threshold:.0}"));
            }
        }
        if let Some(asn) = normalized(&event.asn_type) {
            if matches!(asn.as_str(), "hosting" | "datacenter" | "cloud" | "vpn" | "proxy") {
                score += 20;
                reasons.push(format!("traffic originates from {asn} infrastructure"));
            }
        }
        let headers: Vec<String> = event
            .headers_present
            .iter()
            .map(|h| h.trim().to_ascii_lowercase())
            .collect();
        if !headers.is_empty() {
            for required in ["accept", "accept-language"] {
                if !headers.iter().any(|h| h == required) {
                    score += 10;
                    reasons.push(format!("missing `{required}` header"));
                }
            }
        }
        if let Some(path) = normalized(&event.path) {
            if honeypots.iter().any(|hp| path == *hp) {
                score += 45;
                reasons.push("request hit a honeypot path".to_string());
            }
        }

        if score == 0 {
            continue;
        }
        let score = score.min(100);
        findings.push(RiskFinding {
            id: "bot-traffic".to_string(),
            severity: severity_for_score(score),
            category: "bot".to_string(),
            subject_ref: subject,
            score,
            message: format!("Automated-traffic signals: {}.", reasons.join("; ")),
            recommendation: "Apply rate limiting, challenge (CAPTCHA/JS), or block at the edge; verify against allowlisted good bots first.".to_string(),
        });
    }

    finalize(
        config,
        request.request_id,
        "bot",
        "bot-detection",
        events_analyzed,
        findings,
        base_notes(
            "Heuristic signal scoring over submitted request records; combine with edge WAF telemetry, fingerprinting, and a verified-bot allowlist.",
            truncated,
        ),
    )
}

// ---------------------------------------------------------------------------
// Login anomaly detection
// ---------------------------------------------------------------------------

pub fn detect_login_anomalies(
    config: &Config,
    request: LoginAnomalyRequest,
) -> RiskAnalysisReport {
    let max_kph = request.max_travel_kph.unwrap_or(900.0).max(1.0);
    let (events, truncated) = cap_events(config, request.events);
    let events_analyzed = events.len();

    // Group successful logins per user to establish a per-user baseline and
    // detect impossible travel / new device / new geo against prior events.
    let mut per_user: HashMap<String, Vec<usize>> = HashMap::new();
    for (index, event) in events.iter().enumerate() {
        if let Some(user) = normalized(&event.user) {
            per_user.entry(user).or_default().push(index);
        }
    }
    // Credential-stuffing: distinct users failing from the same IP.
    let mut ip_failed_users: HashMap<String, std::collections::HashSet<String>> = HashMap::new();
    for event in &events {
        if event.success == Some(false) {
            if let (Some(ip), Some(user)) = (normalized(&event.ip), normalized(&event.user)) {
                ip_failed_users.entry(ip).or_default().insert(user);
            }
        }
    }

    let mut findings = Vec::new();
    for (_user, mut indices) in per_user {
        indices.sort_by_key(|&i| events[i].timestamp_ms.unwrap_or(0));
        let mut seen_countries: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut seen_devices: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut prior_success: Option<usize> = None;
        for &index in &indices {
            let event = &events[index];
            let subject = subject_ref(&event.id, index);
            let mut score = 0u32;
            let mut reasons: Vec<String> = Vec::new();

            let country = normalized(&event.country);
            let device = normalized(&event.device_id);
            let is_first = seen_countries.is_empty() && seen_devices.is_empty();

            if let Some(ref c) = country {
                if !is_first && !seen_countries.contains(c) {
                    score += 25;
                    reasons.push(format!("first login from country {c} for this user"));
                }
            }
            if let Some(ref d) = device {
                if !is_first && !seen_devices.contains(d) {
                    score += 20;
                    reasons.push("login from a previously unseen device".to_string());
                }
            }

            // Impossible travel against the prior successful login.
            if event.success != Some(false) {
                if let Some(prev_index) = prior_success {
                    if let Some(kph) = travel_speed_kph(&events[prev_index], event) {
                        if kph > max_kph {
                            score += 45;
                            let detail = if kph.is_finite() {
                                format!("~{kph:.0} km/h exceeds {max_kph:.0} km/h")
                            } else {
                                "two locations at the same instant".to_string()
                            };
                            reasons.push(format!("impossible travel: {detail}"));
                        }
                    } else if country_changed(&events[prev_index], event) {
                        // No coordinates, but country flipped between adjacent logins.
                        if adjacent_within_minutes(&events[prev_index], event, 60) {
                            score += 30;
                            reasons.push("country changed between logins less than an hour apart".to_string());
                        }
                    }
                }
            }

            if event.failed_attempts.unwrap_or(0) >= 5 {
                score += 30;
                reasons.push(format!(
                    "{} failed attempts preceding this login",
                    event.failed_attempts.unwrap_or(0)
                ));
            }
            if event.success == Some(true) && event.mfa_used == Some(false) {
                score += 10;
                reasons.push("successful login without MFA".to_string());
            }
            if let Some(ip) = normalized(&event.ip) {
                if ip_failed_users
                    .get(&ip)
                    .map(|users| users.len() >= 5)
                    .unwrap_or(false)
                {
                    score += 35;
                    reasons.push("part of credential-stuffing: many distinct users failing from this IP".to_string());
                }
            }

            if let Some(c) = country {
                seen_countries.insert(c);
            }
            if let Some(d) = device {
                seen_devices.insert(d);
            }
            if event.success != Some(false) {
                prior_success = Some(index);
            }

            if score == 0 {
                continue;
            }
            let score = score.min(100);
            findings.push(RiskFinding {
                id: "login-anomaly".to_string(),
                severity: severity_for_score(score),
                category: "login".to_string(),
                subject_ref: subject,
                score,
                message: format!("Login anomaly signals: {}.", reasons.join("; ")),
                recommendation: "Trigger step-up auth or session revocation, notify the account owner, and review related sessions.".to_string(),
            });
        }
    }

    finalize(
        config,
        request.request_id,
        "login-anomaly",
        "login-anomaly",
        events_analyzed,
        findings,
        base_notes(
            "Heuristic anomaly scoring over submitted login events; combine with a per-user behavioral baseline and authoritative IP geolocation.",
            truncated,
        ),
    )
}

fn travel_speed_kph(
    prev: &crate::models::LoginEvent,
    next: &crate::models::LoginEvent,
) -> Option<f64> {
    let (lat1, lon1) = (prev.latitude?, prev.longitude?);
    let (lat2, lon2) = (next.latitude?, next.longitude?);
    let t1 = prev.timestamp_ms?;
    let t2 = next.timestamp_ms?;
    let hours = (t2.max(t1) - t2.min(t1)) as f64 / 3_600_000.0;
    let distance = haversine_km(lat1, lon1, lat2, lon2);
    if hours <= 0.0 {
        // Same timestamp but a non-trivial distance is itself impossible.
        return if distance > 1.0 { Some(f64::INFINITY) } else { None };
    }
    Some(distance / hours)
}

fn haversine_km(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    const R: f64 = 6371.0;
    let (rlat1, rlat2) = (lat1.to_radians(), lat2.to_radians());
    let dlat = (lat2 - lat1).to_radians();
    let dlon = (lon2 - lon1).to_radians();
    let a = (dlat / 2.0).sin().powi(2)
        + rlat1.cos() * rlat2.cos() * (dlon / 2.0).sin().powi(2);
    // Clamp before asin: floating-point error on near-antipodal points can push
    // sqrt(a) marginally above 1.0, which would make asin return NaN and silently
    // suppress an impossible-travel finding.
    2.0 * R * a.sqrt().clamp(0.0, 1.0).asin()
}

fn country_changed(prev: &crate::models::LoginEvent, next: &crate::models::LoginEvent) -> bool {
    mismatched(&prev.country, &next.country)
}

fn adjacent_within_minutes(
    prev: &crate::models::LoginEvent,
    next: &crate::models::LoginEvent,
    minutes: u128,
) -> bool {
    match (prev.timestamp_ms, next.timestamp_ms) {
        (Some(a), Some(b)) => (a.max(b) - a.min(b)) <= minutes * 60_000,
        _ => false,
    }
}

fn normalized(value: &Option<String>) -> Option<String> {
    value
        .as_ref()
        .map(|v| v.trim().to_ascii_lowercase())
        .filter(|v| !v.is_empty())
}

fn mismatched(left: &Option<String>, right: &Option<String>) -> bool {
    match (normalized(left), normalized(right)) {
        (Some(a), Some(b)) => a != b,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{BotEvent, FraudEvent, LoginEvent};
    use std::path::PathBuf;

    fn test_config() -> Config {
        Config {
            host: "127.0.0.1".to_string(),
            port: 8118,
            work_root: PathBuf::from("/tmp/dd-compliance-rs-behavior-test"),
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

    #[test]
    fn fraud_flags_disposable_email_and_geo_mismatch() {
        let report = detect_fraud(
            &test_config(),
            FraudDetectionRequest {
                request_id: None,
                title: None,
                events: vec![FraudEvent {
                    id: Some("t1".to_string()),
                    amount: Some(5000.0),
                    currency: None,
                    email: Some("x@mailinator.com".to_string()),
                    ip: Some("1.2.3.4".to_string()),
                    ip_country: Some("RU".to_string()),
                    billing_country: Some("US".to_string()),
                    card_bin_country: None,
                    account_age_days: Some(0.2),
                    prior_chargebacks: Some(2),
                    timestamp_ms: None,
                }],
                high_amount_threshold: Some(1000.0),
                blocklisted_ips: vec![],
            },
        );
        assert!(!report.ok);
        assert_eq!(report.analysis_type, "fraud");
        assert!(report.risk_score >= 65);
    }

    #[test]
    fn bot_flags_automated_user_agent_and_rate() {
        let report = detect_bots(
            &test_config(),
            BotDetectionRequest {
                request_id: None,
                title: None,
                events: vec![BotEvent {
                    id: Some("r1".to_string()),
                    ip: Some("9.9.9.9".to_string()),
                    user_agent: Some("python-requests/2.31".to_string()),
                    path: Some("/login".to_string()),
                    method: Some("POST".to_string()),
                    requests_per_min: Some(900.0),
                    asn_type: Some("hosting".to_string()),
                    headers_present: vec!["host".to_string()],
                }],
                rate_threshold_per_min: Some(120.0),
                honeypot_paths: vec![],
            },
        );
        assert!(!report.ok);
        assert!(report.findings.iter().any(|f| f.category == "bot"));
    }

    #[test]
    fn disposable_email_matches_domain_and_subdomain_only() {
        assert!(is_disposable_email("x@mailinator.com"));
        assert!(is_disposable_email("x@smtp.mailinator.com"));
        // A registrable domain that merely ends with the same letters must not match.
        assert!(!is_disposable_email("x@notmailinator.com"));
        assert!(!is_disposable_email("x@example.com"));
    }

    #[test]
    fn long_caller_subject_id_is_clipped() {
        let report = detect_bots(
            &test_config(),
            BotDetectionRequest {
                request_id: None,
                title: None,
                events: vec![BotEvent {
                    id: Some("A".repeat(5000)),
                    ip: None,
                    user_agent: Some("python-requests/2.31".to_string()),
                    path: None,
                    method: None,
                    requests_per_min: None,
                    asn_type: None,
                    headers_present: vec![],
                }],
                rate_threshold_per_min: Some(120.0),
                honeypot_paths: vec![],
            },
        );
        assert_eq!(report.findings.len(), 1);
        assert!(report.findings[0].subject_ref.chars().count() <= MAX_SUBJECT_CHARS + 1);
    }

    #[test]
    fn event_batch_is_capped_and_reported() {
        let config = test_config(); // max_files = 100
        let events: Vec<FraudEvent> = (0..(config.max_files + 25))
            .map(|i| FraudEvent {
                id: Some(format!("t{i}")),
                amount: Some(5.0),
                currency: None,
                email: None,
                ip: None,
                ip_country: None,
                billing_country: None,
                card_bin_country: None,
                account_age_days: None,
                prior_chargebacks: None,
                timestamp_ms: None,
            })
            .collect();
        let report = detect_fraud(
            &config,
            FraudDetectionRequest {
                request_id: None,
                title: None,
                events,
                high_amount_threshold: Some(1000.0),
                blocklisted_ips: vec![],
            },
        );
        assert_eq!(report.events_analyzed, config.max_files);
        assert!(report
            .notes
            .iter()
            .any(|n| n.contains("per-request event limit")));
    }

    #[test]
    fn login_flags_impossible_travel() {
        let report = detect_login_anomalies(
            &test_config(),
            LoginAnomalyRequest {
                request_id: None,
                title: None,
                events: vec![
                    LoginEvent {
                        id: Some("l1".to_string()),
                        user: Some("alice".to_string()),
                        ip: Some("1.1.1.1".to_string()),
                        country: Some("US".to_string()),
                        latitude: Some(40.71),
                        longitude: Some(-74.0),
                        timestamp_ms: Some(1_000_000_000_000),
                        success: Some(true),
                        device_id: Some("a".to_string()),
                        mfa_used: Some(true),
                        failed_attempts: Some(0),
                    },
                    LoginEvent {
                        id: Some("l2".to_string()),
                        user: Some("alice".to_string()),
                        ip: Some("2.2.2.2".to_string()),
                        country: Some("SG".to_string()),
                        latitude: Some(1.35),
                        longitude: Some(103.8),
                        timestamp_ms: Some(1_000_000_600_000),
                        success: Some(true),
                        device_id: Some("z".to_string()),
                        mfa_used: Some(false),
                        failed_attempts: Some(0),
                    },
                ],
                max_travel_kph: Some(900.0),
            },
        );
        assert!(!report.ok);
        assert!(report
            .findings
            .iter()
            .any(|f| f.message.contains("impossible travel")));
    }
}
