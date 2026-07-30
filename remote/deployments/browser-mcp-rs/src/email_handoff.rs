use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    time::{SystemTime, UNIX_EPOCH},
};
use url::{Host, Url};

const MAX_HANDOFF_AGE_SECONDS: i64 = 15 * 60;
const MAX_REFERENCE_LENGTH: usize = 512;
const MAX_URL_LENGTH: usize = 4_096;

const ALLOWED_RISK_SIGNALS: &[&str] = &[
    "sender_reply_to_mismatch",
    "lookalike_domain",
    "artificial_urgency",
    "requests_credentials",
    "requests_remote_access",
    "requests_crypto_or_gift_card",
    "requests_payment_or_bank",
    "requests_ssn_or_tax_id",
    "requests_identity_document_upload",
    "unexpected_attachment",
];

const HARD_DENY_RISK_SIGNALS: &[&str] = &[
    "lookalike_domain",
    "requests_credentials",
    "requests_remote_access",
    "requests_crypto_or_gift_card",
    "requests_payment_or_bank",
    "requests_ssn_or_tax_id",
    "requests_identity_document_upload",
];

const SOFT_REVIEW_RISK_SIGNALS: &[&str] = &[
    "sender_reply_to_mismatch",
    "artificial_urgency",
    "unexpected_attachment",
];

const BLOCKED_EMAIL_AND_LOGIN_HOSTS: &[&str] = &[
    "gmail.com",
    "mail.google.com",
    "accounts.google.com",
    "outlook.com",
    "office.com",
    "login.microsoftonline.com",
    "login.live.com",
    "bit.ly",
    "t.co",
    "tinyurl.com",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HandoffError {
    pub code: &'static str,
    pub message: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GmailHandoffAudit {
    pub mailbox_alias: String,
    pub message_ref: String,
    pub thread_ref: String,
    pub sender_domain: String,
    pub reply_to_domain: Option<String>,
    pub target_host: String,
    pub workflow_id: String,
    pub risk_signals: Vec<String>,
    pub expires_at_unix: i64,
}

fn handoff_error(code: &'static str, message: &'static str) -> HandoffError {
    HandoffError { code, message }
}

fn required_string<'a>(
    object: &'a serde_json::Map<String, Value>,
    name: &str,
    max_len: usize,
) -> Result<&'a str, HandoffError> {
    let value = object.get(name).and_then(Value::as_str).ok_or_else(|| {
        handoff_error(
            "invalid_source_context",
            "required Gmail handoff field is missing",
        )
    })?;
    if value.is_empty()
        || value.len() > max_len
        || value.chars().any(|character| character.is_control())
    {
        return Err(handoff_error(
            "invalid_source_context",
            "Gmail handoff field has an invalid length or control character",
        ));
    }
    Ok(value)
}

fn required_i64(object: &serde_json::Map<String, Value>, name: &str) -> Result<i64, HandoffError> {
    object.get(name).and_then(Value::as_i64).ok_or_else(|| {
        handoff_error(
            "invalid_source_context",
            "required Gmail handoff timestamp is missing",
        )
    })
}

fn require_true(
    object: &serde_json::Map<String, Value>,
    name: &str,
    code: &'static str,
    message: &'static str,
) -> Result<(), HandoffError> {
    if object.get(name).and_then(Value::as_bool) == Some(true) {
        Ok(())
    } else {
        Err(handoff_error(code, message))
    }
}

fn valid_domain(domain: &str) -> bool {
    domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && !domain.contains("..")
        && !domain.starts_with("xn--")
        && !domain.contains(".xn--")
        && domain
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'.' || byte == b'-')
}

fn normalize_domain(domain: &str) -> Result<String, HandoffError> {
    let normalized = domain.trim().to_ascii_lowercase();
    if !valid_domain(&normalized) {
        return Err(handoff_error(
            "invalid_source_context",
            "Gmail handoff domains must be plain ASCII hostnames",
        ));
    }
    Ok(normalized)
}

fn domain_within_ceiling(domain: &str, ceiling: &[String]) -> bool {
    ceiling.iter().any(|allowed| {
        let allowed = allowed.to_ascii_lowercase();
        domain == allowed || domain.ends_with(&format!(".{allowed}"))
    })
}

fn blocked_email_or_login_host(host: &str) -> bool {
    BLOCKED_EMAIL_AND_LOGIN_HOSTS
        .iter()
        .any(|blocked| host == *blocked || host.ends_with(&format!(".{blocked}")))
}

fn parse_external_url(
    raw: &str,
    allowed_domains: &[String],
) -> Result<(Url, String), HandoffError> {
    if raw.len() > MAX_URL_LENGTH {
        return Err(handoff_error(
            "invalid_source_context",
            "approved external URL is too long",
        ));
    }
    let url = Url::parse(raw).map_err(|_| {
        handoff_error(
            "invalid_source_context",
            "approved external URL is malformed",
        )
    })?;
    if url.scheme() != "https" {
        return Err(handoff_error(
            "domain_not_allowed",
            "Gmail handoff navigation requires HTTPS",
        ));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(handoff_error(
            "domain_not_allowed",
            "URL credentials are forbidden in Gmail handoffs",
        ));
    }
    if url.port().is_some_and(|port| port != 443) {
        return Err(handoff_error(
            "domain_not_allowed",
            "Gmail handoff navigation permits only the default HTTPS port",
        ));
    }
    let host = match url.host() {
        Some(Host::Domain(domain)) => normalize_domain(domain)?,
        Some(Host::Ipv4(_)) | Some(Host::Ipv6(_)) | None => {
            return Err(handoff_error(
                "domain_not_allowed",
                "IP literals and hostless URLs are forbidden in Gmail handoffs",
            ));
        }
    };
    if blocked_email_or_login_host(&host) {
        return Err(handoff_error(
            "domain_not_allowed",
            "webmail, identity login, and generic short-link hosts are forbidden",
        ));
    }
    if allowed_domains.is_empty() || !domain_within_ceiling(&host, allowed_domains) {
        return Err(handoff_error(
            "domain_not_allowed",
            "approved external URL is outside the selected workflow profile",
        ));
    }
    Ok((url, host))
}

fn parse_risk_signals(
    object: &serde_json::Map<String, Value>,
) -> Result<Vec<String>, HandoffError> {
    let values = object
        .get("risk_signals")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            handoff_error(
                "risk_assessment_incomplete",
                "risk_signals must be supplied",
            )
        })?;
    if values.len() > ALLOWED_RISK_SIGNALS.len() {
        return Err(handoff_error(
            "risk_assessment_incomplete",
            "too many Gmail risk signals were supplied",
        ));
    }
    let mut signals = BTreeSet::new();
    for value in values {
        let signal = value.as_str().ok_or_else(|| {
            handoff_error("risk_assessment_incomplete", "risk signals must be strings")
        })?;
        if !ALLOWED_RISK_SIGNALS.contains(&signal) || !signals.insert(signal.to_string()) {
            return Err(handoff_error(
                "risk_assessment_incomplete",
                "risk signals contain an unknown or duplicate value",
            ));
        }
    }
    Ok(signals.into_iter().collect())
}

fn navigation_urls(actions: Option<&Value>) -> Result<Vec<String>, HandoffError> {
    let actions = actions
        .and_then(Value::as_array)
        .ok_or_else(|| handoff_error("invalid_source_context", "browser actions are missing"))?;
    let mut urls = Vec::new();
    for action in actions {
        let action = action.as_object().ok_or_else(|| {
            handoff_error("invalid_source_context", "browser action must be an object")
        })?;
        let action_type = action
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let raw = match action_type {
            "start" => action.get("initial_url").and_then(Value::as_str),
            "goto" => action.get("url").and_then(Value::as_str),
            _ => None,
        };
        if let Some(raw) = raw {
            urls.push(raw.to_string());
        }
    }
    if urls.is_empty() {
        return Err(handoff_error(
            "invalid_source_context",
            "a Gmail handoff must include an approved navigation action",
        ));
    }
    Ok(urls)
}

pub fn hash_reference(namespace: &str, value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(namespace.as_bytes());
    hasher.update([0]);
    hasher.update(value.as_bytes());
    let digest = hasher.finalize();
    let hex = digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    format!("sha256:{hex}")
}

pub fn validate_gmail_handoff(
    context: &Value,
    workflow_id: &str,
    allowed_domains: &[String],
    actions: Option<&Value>,
) -> Result<GmailHandoffAudit, HandoffError> {
    let object = context.as_object().ok_or_else(|| {
        handoff_error("invalid_source_context", "source_context must be an object")
    })?;
    if required_string(object, "kind", 32)? != "gmail" {
        return Err(handoff_error(
            "invalid_source_context",
            "only Gmail source_context is supported",
        ));
    }

    let mailbox_alias = required_string(object, "mailbox_alias", 32)?;
    if !matches!(mailbox_alias, "personal" | "fiducia") {
        return Err(handoff_error(
            "invalid_source_context",
            "mailbox_alias must be personal or fiducia",
        ));
    }

    require_true(
        object,
        "risk_assessment_complete",
        "risk_assessment_incomplete",
        "Gmail link risk assessment must be completed before navigation",
    )?;
    require_true(
        object,
        "user_approved_open_external_link",
        "external_link_approval_required",
        "opening an email-derived external link requires explicit user approval",
    )?;

    let message_id = required_string(object, "message_id", MAX_REFERENCE_LENGTH)?;
    let thread_id = required_string(object, "thread_id", MAX_REFERENCE_LENGTH)?;
    let sender_domain = normalize_domain(required_string(object, "sender_domain", 253)?)?;
    let reply_to_domain = match object.get("reply_to_domain") {
        None | Some(Value::Null) => None,
        Some(value) => Some(normalize_domain(value.as_str().ok_or_else(|| {
            handoff_error(
                "invalid_source_context",
                "reply_to_domain must be a hostname",
            )
        })?)?),
    };

    let issued_at_unix = required_i64(object, "issued_at_unix")?;
    let expires_at_unix = required_i64(object, "expires_at_unix")?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| handoff_error("invalid_source_context", "system clock is unavailable"))?
        .as_secs() as i64;
    if issued_at_unix > now + 60
        || issued_at_unix < now - MAX_HANDOFF_AGE_SECONDS
        || expires_at_unix <= issued_at_unix
        || expires_at_unix > issued_at_unix + MAX_HANDOFF_AGE_SECONDS
        || expires_at_unix < now
    {
        return Err(handoff_error(
            "source_context_expired",
            "Gmail handoff provenance is expired or outside its bounded lifetime",
        ));
    }

    let risk_signals = parse_risk_signals(object)?;
    let risk_set = risk_signals
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let actual_reply_to_mismatch = reply_to_domain
        .as_deref()
        .is_some_and(|reply_to| reply_to != sender_domain);
    if actual_reply_to_mismatch != risk_set.contains("sender_reply_to_mismatch") {
        return Err(handoff_error(
            "risk_assessment_incomplete",
            "sender/reply-to mismatch signal does not match supplied domains",
        ));
    }
    if HARD_DENY_RISK_SIGNALS
        .iter()
        .any(|signal| risk_set.contains(signal))
    {
        return Err(handoff_error(
            "email_link_risk_denied",
            "Gmail handoff contains a fail-closed phishing or sensitive-data signal",
        ));
    }
    let has_soft_risk = SOFT_REVIEW_RISK_SIGNALS
        .iter()
        .any(|signal| risk_set.contains(signal));
    if has_soft_risk
        && object
            .get("user_confirmed_risk_review")
            .and_then(Value::as_bool)
            != Some(true)
    {
        return Err(handoff_error(
            "email_link_review_required",
            "soft Gmail risk signals require explicit reviewed-link confirmation",
        ));
    }

    let approved_raw = required_string(object, "approved_external_url", MAX_URL_LENGTH)?;
    let (approved_url, target_host) = parse_external_url(approved_raw, allowed_domains)?;
    let navigations = navigation_urls(actions)?;
    for raw in &navigations {
        parse_external_url(raw, allowed_domains)?;
    }
    let first_navigation = Url::parse(&navigations[0]).map_err(|_| {
        handoff_error(
            "invalid_source_context",
            "first browser navigation URL is malformed",
        )
    })?;
    if first_navigation != approved_url {
        return Err(handoff_error(
            "external_link_approval_required",
            "first browser navigation does not match the user-approved email link",
        ));
    }

    Ok(GmailHandoffAudit {
        mailbox_alias: mailbox_alias.to_string(),
        message_ref: hash_reference(&format!("gmail-message:{mailbox_alias}"), message_id),
        thread_ref: hash_reference(&format!("gmail-thread:{mailbox_alias}"), thread_id),
        sender_domain,
        reply_to_domain,
        target_host,
        workflow_id: workflow_id.to_string(),
        risk_signals,
        expires_at_unix,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn now() -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_secs() as i64
    }

    fn safe_context(url: &str) -> Value {
        let issued = now();
        json!({
            "kind": "gmail",
            "mailbox_alias": "fiducia",
            "message_id": "message-raw-id",
            "thread_id": "thread-raw-id",
            "sender_domain": "example.com",
            "risk_assessment_complete": true,
            "risk_signals": [],
            "user_approved_open_external_link": true,
            "approved_external_url": url,
            "issued_at_unix": issued,
            "expires_at_unix": issued + 600
        })
    }

    fn actions(url: &str) -> Value {
        json!([{"type": "start", "initial_url": url}])
    }

    fn ceiling() -> Vec<String> {
        vec!["greenhouse.io".to_string()]
    }

    #[test]
    fn accepts_safe_approved_link_and_hashes_identifiers() {
        let url = "https://boards.greenhouse.io/example/jobs/123";
        let audit = validate_gmail_handoff(
            &safe_context(url),
            "platform-jobs",
            &ceiling(),
            Some(&actions(url)),
        )
        .expect("safe handoff");
        assert_eq!(audit.target_host, "boards.greenhouse.io");
        assert!(audit.message_ref.starts_with("sha256:"));
        assert!(audit.thread_ref.starts_with("sha256:"));
        assert!(!audit.message_ref.contains("message-raw-id"));
        assert!(!audit.thread_ref.contains("thread-raw-id"));
    }

    #[test]
    fn rejects_off_profile_and_webmail_links() {
        let off_profile = "https://evil.example/apply";
        assert_eq!(
            validate_gmail_handoff(
                &safe_context(off_profile),
                "platform-jobs",
                &ceiling(),
                Some(&actions(off_profile)),
            )
            .expect_err("off profile")
            .code,
            "domain_not_allowed"
        );

        let gmail = "https://mail.google.com/mail/u/0/";
        assert_eq!(
            validate_gmail_handoff(
                &safe_context(gmail),
                "unsafe-test",
                &["mail.google.com".to_string()],
                Some(&actions(gmail)),
            )
            .expect_err("webmail")
            .code,
            "domain_not_allowed"
        );
    }

    #[test]
    fn rejects_credentials_ip_literals_and_unapproved_navigation() {
        for url in [
            "https://user:pass@greenhouse.io/apply",
            "https://127.0.0.1/apply",
        ] {
            assert_eq!(
                validate_gmail_handoff(
                    &safe_context(url),
                    "platform-jobs",
                    &ceiling(),
                    Some(&actions(url)),
                )
                .expect_err("unsafe URL")
                .code,
                "domain_not_allowed"
            );
        }

        let approved = "https://greenhouse.io/jobs/1";
        let different = "https://greenhouse.io/jobs/2";
        assert_eq!(
            validate_gmail_handoff(
                &safe_context(approved),
                "platform-jobs",
                &ceiling(),
                Some(&actions(different)),
            )
            .expect_err("unapproved navigation")
            .code,
            "external_link_approval_required"
        );
    }

    #[test]
    fn hard_risk_signals_fail_closed() {
        let url = "https://greenhouse.io/jobs/1";
        let mut context = safe_context(url);
        context["risk_signals"] = json!(["requests_ssn_or_tax_id"]);
        assert_eq!(
            validate_gmail_handoff(&context, "platform-jobs", &ceiling(), Some(&actions(url)),)
                .expect_err("hard risk")
                .code,
            "email_link_risk_denied"
        );
    }

    #[test]
    fn soft_risk_requires_review_and_mismatch_signal_consistency() {
        let url = "https://greenhouse.io/jobs/1";
        let mut context = safe_context(url);
        context["reply_to_domain"] = json!("other.example");
        context["risk_signals"] = json!(["sender_reply_to_mismatch"]);
        assert_eq!(
            validate_gmail_handoff(&context, "platform-jobs", &ceiling(), Some(&actions(url)),)
                .expect_err("review")
                .code,
            "email_link_review_required"
        );
        context["user_confirmed_risk_review"] = json!(true);
        validate_gmail_handoff(&context, "platform-jobs", &ceiling(), Some(&actions(url)))
            .expect("reviewed mismatch");
    }

    #[test]
    fn expired_provenance_is_rejected() {
        let url = "https://greenhouse.io/jobs/1";
        let mut context = safe_context(url);
        let issued = now() - 2_000;
        context["issued_at_unix"] = json!(issued);
        context["expires_at_unix"] = json!(issued + 600);
        assert_eq!(
            validate_gmail_handoff(&context, "platform-jobs", &ceiling(), Some(&actions(url)),)
                .expect_err("expired")
                .code,
            "source_context_expired"
        );
    }
}
