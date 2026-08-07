use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use p256::SecretKey;
use p256::elliptic_curve::sec1::ToEncodedPoint;
use p256::pkcs8::{DecodePrivateKey, EncodePrivateKey};
use reqwest::header::RETRY_AFTER;
use reqwest::redirect::Policy as RedirectPolicy;
use reqwest::{Client, StatusCode};
use serde::Serialize;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use thiserror::Error;
use tokio::net::lookup_host;
use url::Url;

use crate::contracts::{
    ContractVersion, OutcomeClass, ProviderKind, PushJob, PushOutcome, PushPriority, PushTarget,
};
use crate::provider::{ProviderError, ProviderReadiness, PushProvider};
use crate::retry::{classify_http_status, parse_retry_after};
use crate::validation::validate_push_job;

const DEFAULT_TTL_SECONDS: u32 = 43_200;
const MAX_TTL_SECONDS: u32 = 28 * 24 * 60 * 60;
const VAPID_TOKEN_LIFETIME_SECONDS: u64 = 12 * 60 * 60;
const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
const MAX_PLAINTEXT_PAYLOAD_BYTES: usize = 3_072;
const DEFAULT_ALLOWED_HOSTS: &[&str] = &[
    "fcm.googleapis.com",
    "push.services.mozilla.com",
    "notify.windows.com",
    "push.apple.com",
];

#[derive(Debug, Error)]
pub enum WebPushConfigError {
    #[error("Web Push configuration field '{0}' is required")]
    MissingField(&'static str),

    #[error("VAPID subject must be a mailto or HTTPS URI")]
    InvalidSubject,

    #[error("VAPID private key must be a valid P-256 PKCS#8 or SEC1 PEM key")]
    InvalidPrivateKey,

    #[error("Web Push TTL exceeds the 28-day service ceiling")]
    InvalidTtl,

    #[error("Web Push allowlist contains an invalid DNS suffix")]
    InvalidAllowedHost,

    #[error("Web Push HTTP client could not be constructed")]
    HttpClient(#[source] reqwest::Error),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WebPushHostPolicy {
    Allowlist(Vec<String>),
    AnyPublic,
}

impl Default for WebPushHostPolicy {
    fn default() -> Self {
        Self::strict_default()
    }
}

impl WebPushHostPolicy {
    pub fn strict_default() -> Self {
        Self::Allowlist(
            DEFAULT_ALLOWED_HOSTS
                .iter()
                .map(|host| (*host).to_owned())
                .collect(),
        )
    }

    pub fn allowlist<I, S>(hosts: I) -> Result<Self, WebPushConfigError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut normalized = Vec::new();
        for host in hosts {
            let host = normalize_allowed_host(host.as_ref())?;
            if !normalized.contains(&host) {
                normalized.push(host);
            }
        }
        if normalized.is_empty() {
            return Err(WebPushConfigError::InvalidAllowedHost);
        }
        Ok(Self::Allowlist(normalized))
    }

    pub const fn any_public() -> Self {
        Self::AnyPublic
    }

    pub fn allowed_hosts(&self) -> Option<&[String]> {
        match self {
            Self::Allowlist(hosts) => Some(hosts),
            Self::AnyPublic => None,
        }
    }
}

fn normalize_allowed_host(value: &str) -> Result<String, WebPushConfigError> {
    let host = value.trim().trim_start_matches('.').to_ascii_lowercase();
    if host.is_empty()
        || host.len() > 253
        || host.starts_with('-')
        || host.ends_with('-')
        || host.starts_with('.')
        || host.ends_with('.')
        || !host.contains('.')
        || host
            .chars()
            .any(|character| !(character.is_ascii_alphanumeric() || matches!(character, '-' | '.')))
    {
        return Err(WebPushConfigError::InvalidAllowedHost);
    }
    Ok(host)
}

#[derive(Clone)]
pub struct WebPushConfig {
    vapid_signing_key: Arc<EncodingKey>,
    vapid_public_key: String,
    vapid_subject: String,
    default_ttl_seconds: u32,
    host_policy: WebPushHostPolicy,
    request_timeout: Duration,
}

impl WebPushConfig {
    pub fn new(
        vapid_private_key_pem: impl Into<String>,
        vapid_subject: impl Into<String>,
        host_policy: WebPushHostPolicy,
    ) -> Result<Self, WebPushConfigError> {
        let vapid_private_key_pem =
            required(vapid_private_key_pem.into(), "vapid_private_key_pem")?;
        let vapid_subject = required(vapid_subject.into(), "vapid_subject")?;
        validate_vapid_subject(&vapid_subject)?;
        let (vapid_signing_key, vapid_public_key) =
            parse_vapid_private_key(&vapid_private_key_pem)?;
        Ok(Self {
            vapid_signing_key: Arc::new(vapid_signing_key),
            vapid_public_key,
            vapid_subject,
            default_ttl_seconds: DEFAULT_TTL_SECONDS,
            host_policy,
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
        })
    }

    pub fn with_default_ttl(mut self, ttl_seconds: u32) -> Result<Self, WebPushConfigError> {
        if ttl_seconds > MAX_TTL_SECONDS {
            return Err(WebPushConfigError::InvalidTtl);
        }
        self.default_ttl_seconds = ttl_seconds;
        Ok(self)
    }

    pub fn with_request_timeout(mut self, timeout: Duration) -> Self {
        self.request_timeout = timeout;
        self
    }

    pub fn host_policy(&self) -> &WebPushHostPolicy {
        &self.host_policy
    }
}

fn parse_vapid_private_key(
    private_key_pem: &str,
) -> Result<(EncodingKey, String), WebPushConfigError> {
    let secret_key = SecretKey::from_pkcs8_pem(private_key_pem)
        .or_else(|_| SecretKey::from_sec1_pem(private_key_pem))
        .map_err(|_| WebPushConfigError::InvalidPrivateKey)?;
    let private_key_der = secret_key
        .to_pkcs8_der()
        .map_err(|_| WebPushConfigError::InvalidPrivateKey)?;
    let signing_key = EncodingKey::from_ec_der(private_key_der.as_bytes());
    let public_key = secret_key.public_key().to_encoded_point(false);
    let public_key = URL_SAFE_NO_PAD.encode(public_key.as_bytes());
    Ok((signing_key, public_key))
}

fn required(value: String, field: &'static str) -> Result<String, WebPushConfigError> {
    let value = value.trim().to_owned();
    if value.is_empty() {
        Err(WebPushConfigError::MissingField(field))
    } else {
        Ok(value)
    }
}

fn validate_vapid_subject(subject: &str) -> Result<(), WebPushConfigError> {
    let url = Url::parse(subject).map_err(|_| WebPushConfigError::InvalidSubject)?;
    let valid = match url.scheme() {
        "mailto" => subject.strip_prefix("mailto:").is_some_and(|address| {
            address.contains('@') && !address.chars().any(char::is_whitespace)
        }),
        "https" => {
            url.host_str().is_some()
                && url.username().is_empty()
                && url.password().is_none()
                && url.fragment().is_none()
        }
        _ => false,
    };
    if valid {
        Ok(())
    } else {
        Err(WebPushConfigError::InvalidSubject)
    }
}

#[derive(Clone)]
pub struct WebPushProvider {
    config: WebPushConfig,
    client: Client,
}

impl WebPushProvider {
    pub fn new(config: WebPushConfig) -> Result<Self, WebPushConfigError> {
        let client = Client::builder()
            .redirect(RedirectPolicy::none())
            .timeout(config.request_timeout)
            .build()
            .map_err(WebPushConfigError::HttpClient)?;
        Ok(Self::with_client(config, client))
    }

    pub fn with_client(config: WebPushConfig, client: Client) -> Self {
        Self { config, client }
    }

    async fn send_message(&self, job: &PushJob) -> Result<PushOutcome, ProviderError> {
        validate_job(job)?;
        let (endpoint, p256dh, auth) = match &job.target {
            PushTarget::WebPush {
                endpoint,
                p256dh,
                auth,
            } => (endpoint.as_str(), p256dh.as_str(), auth.as_str()),
            _ => {
                return Err(invalid_payload(
                    "Web Push provider received a non-Web-Push target",
                    "provider_target_mismatch",
                ));
            }
        };

        let endpoint = validate_endpoint(endpoint, &self.config.host_policy)?;
        if matches!(self.config.host_policy, WebPushHostPolicy::AnyPublic) {
            verify_public_dns(&endpoint).await?;
        }

        let payload = build_payload(job)?;
        let encrypted_payload = encrypt_subscription_payload(p256dh, auth, &payload)?;
        let authorization = vapid_authorization(&self.config, &endpoint, unix_now()?)?;
        let ttl = job
            .options
            .ttl_seconds
            .unwrap_or(self.config.default_ttl_seconds);
        let mut request = self
            .client
            .post(endpoint)
            .header("Authorization", authorization)
            .header("Content-Encoding", "aes128gcm")
            .header("Content-Type", "application/octet-stream")
            .header("TTL", ttl.to_string())
            .header(
                "Urgency",
                match job.options.priority {
                    PushPriority::Normal => "normal",
                    PushPriority::High => "high",
                },
            );
        if let Some(collapse_key) = &job.options.collapse_key {
            request = request.header("Topic", topic_for_collapse_key(collapse_key));
        }

        let response = request
            .body(encrypted_payload)
            .send()
            .await
            .map_err(classify_transport_error)?;
        classify_response(job, response.status(), response.headers().get(RETRY_AFTER))
    }
}

#[async_trait]
impl PushProvider for WebPushProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::WebPush
    }

    fn readiness(&self) -> ProviderReadiness {
        ProviderReadiness::ready()
    }

    async fn send(&self, job: &PushJob) -> Result<PushOutcome, ProviderError> {
        self.send_message(job).await
    }
}

#[derive(Serialize)]
struct VapidClaims<'a> {
    aud: &'a str,
    exp: u64,
    sub: &'a str,
}

fn vapid_authorization(
    config: &WebPushConfig,
    endpoint: &Url,
    now: u64,
) -> Result<String, ProviderError> {
    let audience = endpoint.origin().ascii_serialization();
    let claims = VapidClaims {
        aud: &audience,
        exp: now.saturating_add(VAPID_TOKEN_LIFETIME_SECONDS),
        sub: &config.vapid_subject,
    };
    let token = encode(
        &Header::new(Algorithm::ES256),
        &claims,
        config.vapid_signing_key.as_ref(),
    )
    .map_err(|_| ProviderError::not_configured("VAPID ES256 token could not be created"))?;
    Ok(format!("vapid t={token}, k={}", config.vapid_public_key))
}

fn unix_now() -> Result<u64, ProviderError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|_| ProviderError::internal("system clock is before the Unix epoch"))
}

fn encrypt_subscription_payload(
    p256dh: &str,
    auth: &str,
    payload: &[u8],
) -> Result<Vec<u8>, ProviderError> {
    let p256dh = decode_subscription_component(p256dh, "p256dh")?;
    let auth = decode_subscription_component(auth, "auth")?;
    if p256dh.len() != 65 || p256dh.first().copied() != Some(0x04) {
        return Err(invalid_payload(
            "Web Push p256dh must be an uncompressed P-256 public key",
            "invalid_p256dh",
        ));
    }
    if auth.len() != 16 {
        return Err(invalid_payload(
            "Web Push auth secret must decode to 16 bytes",
            "invalid_auth_secret",
        ));
    }
    ece::encrypt(&p256dh, &auth, payload).map_err(|_| {
        invalid_payload(
            "Web Push subscription keys or payload could not be encrypted",
            "encryption_failed",
        )
    })
}

fn decode_subscription_component(
    value: &str,
    name: &'static str,
) -> Result<Vec<u8>, ProviderError> {
    URL_SAFE_NO_PAD.decode(value).map_err(|_| {
        invalid_payload(
            format!("Web Push {name} is not unpadded URL-safe base64"),
            "invalid_subscription_key",
        )
    })
}

fn validate_job(job: &PushJob) -> Result<(), ProviderError> {
    validate_push_job(job).map_err(|errors| {
        invalid_payload(
            errors
                .into_iter()
                .map(|error| error.to_string())
                .collect::<Vec<_>>()
                .join("; "),
            "contract_validation_failed",
        )
    })?;
    if job.provider != ProviderKind::WebPush || !matches!(job.target, PushTarget::WebPush { .. }) {
        return Err(invalid_payload(
            "Web Push job provider and target must both be web_push",
            "provider_target_mismatch",
        ));
    }
    if job.options.dry_run {
        return Err(invalid_payload(
            "Web Push does not provide a validate-only delivery mode",
            "dry_run_unsupported",
        ));
    }
    Ok(())
}

fn validate_endpoint(endpoint: &str, policy: &WebPushHostPolicy) -> Result<Url, ProviderError> {
    let url = Url::parse(endpoint)
        .map_err(|_| invalid_endpoint("Web Push endpoint is not a valid URL"))?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || url.port().is_some_and(|port| port != 443)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err(invalid_endpoint(
            "Web Push endpoint must use HTTPS port 443 without credentials or fragments",
        ));
    }

    let host = url
        .host_str()
        .ok_or_else(|| invalid_endpoint("Web Push endpoint is missing a host"))?
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_ascii_lowercase();
    if host == "localhost" || host.ends_with(".localhost") {
        return Err(blocked_endpoint("Web Push endpoint host is local"));
    }
    if let Ok(address) = host.parse::<IpAddr>()
        && ip_is_blocked(address)
    {
        return Err(blocked_endpoint(
            "Web Push endpoint points at a non-public address",
        ));
    }

    if let WebPushHostPolicy::Allowlist(allowed_hosts) = policy
        && !allowed_hosts
            .iter()
            .any(|allowed| host_matches_suffix(&host, allowed))
    {
        return Err(blocked_endpoint(
            "Web Push endpoint host is not in the configured push-service allowlist",
        ));
    }
    Ok(url)
}

fn host_matches_suffix(host: &str, allowed: &str) -> bool {
    host == allowed
        || host
            .strip_suffix(allowed)
            .is_some_and(|prefix| prefix.ends_with('.'))
}

async fn verify_public_dns(endpoint: &Url) -> Result<(), ProviderError> {
    let host = endpoint
        .host_str()
        .ok_or_else(|| invalid_endpoint("Web Push endpoint is missing a host"))?
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_owned();
    if host.parse::<IpAddr>().is_ok() {
        return Ok(());
    }
    let port = endpoint.port_or_known_default().unwrap_or(443);
    let addresses = lookup_host(format!("{host}:{port}"))
        .await
        .map_err(|_| {
            ProviderError::delivery(
                OutcomeClass::TransientProviderFailure,
                "Web Push endpoint host could not be resolved",
                None,
                Some("dns_resolution_failed".to_owned()),
            )
        })?
        .map(|address| address.ip())
        .collect::<Vec<_>>();
    if addresses.is_empty() {
        return Err(ProviderError::delivery(
            OutcomeClass::TransientProviderFailure,
            "Web Push endpoint host returned no addresses",
            None,
            Some("dns_no_addresses".to_owned()),
        ));
    }
    if addresses.iter().copied().any(ip_is_blocked) {
        return Err(blocked_endpoint(
            "Web Push endpoint resolves to a non-public address",
        ));
    }
    Ok(())
}

fn ip_is_blocked(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => ipv4_is_blocked(address),
        IpAddr::V6(address) => ipv6_is_blocked(address),
    }
}

fn ipv4_is_blocked(address: Ipv4Addr) -> bool {
    let octets = address.octets();
    address.is_private()
        || address.is_loopback()
        || address.is_link_local()
        || address.is_unspecified()
        || address.is_broadcast()
        || address.is_documentation()
        || address.is_multicast()
        || octets[0] == 0
        || (octets[0] == 100 && (octets[1] & 0xc0) == 64)
        || (octets[0] == 192 && octets[1] == 0 && octets[2] == 0)
        || (octets[0] == 198 && matches!(octets[1], 18 | 19))
        || octets[0] >= 240
}

fn ipv6_is_blocked(address: Ipv6Addr) -> bool {
    let segments = address.segments();
    address.is_loopback()
        || address.is_unspecified()
        || address.is_multicast()
        || (segments[0] & 0xfe00) == 0xfc00
        || (segments[0] & 0xffc0) == 0xfe80
        || (segments[0] & 0xffc0) == 0xfec0
        || (segments[0] == 0x2001 && segments[1] == 0x0db8)
        || address.to_ipv4_mapped().is_some_and(ipv4_is_blocked)
}

fn build_payload(job: &PushJob) -> Result<Vec<u8>, ProviderError> {
    let mut document = Map::new();
    if let Some(title) = &job.notification.title {
        document.insert("title".to_owned(), Value::String(title.clone()));
    }
    if let Some(body) = &job.notification.body {
        document.insert("body".to_owned(), Value::String(body.clone()));
    }
    if let Some(image_url) = &job.notification.image_url {
        document.insert("image_url".to_owned(), Value::String(image_url.clone()));
    }
    if !job.notification.data.is_empty() {
        document.insert(
            "data".to_owned(),
            Value::Object(
                job.notification
                    .data
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect(),
            ),
        );
    }
    let payload = serde_json::to_vec(&Value::Object(document))
        .map_err(|_| ProviderError::internal("Web Push payload could not be serialized"))?;
    if payload.len() > MAX_PLAINTEXT_PAYLOAD_BYTES {
        return Err(invalid_payload(
            format!("Web Push plaintext payload exceeds {MAX_PLAINTEXT_PAYLOAD_BYTES} bytes"),
            "payload_too_large",
        ));
    }
    Ok(payload)
}

fn topic_for_collapse_key(collapse_key: &str) -> String {
    let digest = Sha256::digest(collapse_key.as_bytes());
    hex::encode(&digest[..16])
}

fn classify_transport_error(error: reqwest::Error) -> ProviderError {
    let code = if error.is_timeout() {
        "transport_timeout"
    } else if error.is_connect() {
        "transport_connect"
    } else {
        "transport_error"
    };
    ProviderError::delivery(
        OutcomeClass::TransientProviderFailure,
        "Web Push request failed before a provider response was received",
        None,
        Some(code.to_owned()),
    )
}

fn classify_response(
    job: &PushJob,
    status: StatusCode,
    retry_after_header: Option<&reqwest::header::HeaderValue>,
) -> Result<PushOutcome, ProviderError> {
    if status.is_success() {
        return Ok(PushOutcome::accepted(job));
    }
    let retry_after = retry_after_header
        .and_then(|value| value.to_str().ok())
        .and_then(|value| parse_retry_after(value, SystemTime::now()));
    let class = match status {
        StatusCode::NOT_FOUND | StatusCode::GONE => OutcomeClass::InvalidToken,
        StatusCode::PAYLOAD_TOO_LARGE | StatusCode::BAD_REQUEST => OutcomeClass::InvalidPayload,
        StatusCode::TOO_MANY_REQUESTS => OutcomeClass::Throttled,
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => OutcomeClass::PermanentProviderFailure,
        _ => {
            let decision = classify_http_status(status.as_u16(), retry_after);
            if decision.retryable {
                OutcomeClass::TransientProviderFailure
            } else {
                OutcomeClass::PermanentProviderFailure
            }
        }
    };
    Ok(PushOutcome {
        version: ContractVersion::V1,
        job_id: job.job_id.clone(),
        provider: ProviderKind::WebPush,
        target_fingerprint: job.target.fingerprint(),
        class,
        provider_code: Some(format!("http_{}", status.as_u16())),
        retry_after_ms: retry_after.map(duration_to_millis),
        safe_detail: Some(
            match class {
                OutcomeClass::InvalidToken => "Web Push subscription is expired or unavailable",
                OutcomeClass::InvalidPayload => "Web Push service rejected the request payload",
                OutcomeClass::Throttled => "Web Push service throttled the request",
                OutcomeClass::TransientProviderFailure => {
                    "Web Push service returned a retryable failure"
                }
                _ => "Web Push service permanently rejected the request",
            }
            .to_owned(),
        ),
    })
}

fn duration_to_millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

fn invalid_endpoint(detail: impl AsRef<str>) -> ProviderError {
    ProviderError::delivery(
        OutcomeClass::InvalidPayload,
        detail,
        None,
        Some("invalid_endpoint".to_owned()),
    )
}

fn blocked_endpoint(detail: impl AsRef<str>) -> ProviderError {
    ProviderError::delivery(
        OutcomeClass::InvalidPayload,
        detail,
        None,
        Some("endpoint_policy_rejected".to_owned()),
    )
}

fn invalid_payload(detail: impl AsRef<str>, code: &'static str) -> ProviderError {
    ProviderError::delivery(
        OutcomeClass::InvalidPayload,
        detail,
        None,
        Some(code.to_owned()),
    )
}

pub fn redact_web_push_endpoint(endpoint: &str) -> String {
    let Ok(url) = Url::parse(endpoint) else {
        return "web-push-subscription".to_owned();
    };
    let Some(host) = url.host_str() else {
        return "web-push-subscription".to_owned();
    };
    match url.port() {
        Some(port) => format!("{}://{host}:{port}/…", url.scheme()),
        None => format!("{}://{host}/…", url.scheme()),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use p256::pkcs8::{EncodePrivateKey, LineEnding};
    use rand_core::OsRng;
    use serde_json::json;

    use super::*;
    use crate::contracts::{Notification, PushOptions, TraceMetadata};

    fn web_push_job(endpoint: &str) -> PushJob {
        PushJob {
            version: ContractVersion::V1,
            job_id: "job-web-1".to_owned(),
            tenant_id: "tenant-1".to_owned(),
            application_id: "app-1".to_owned(),
            idempotency_key: "event-web-1".to_owned(),
            provider: ProviderKind::WebPush,
            target: PushTarget::WebPush {
                endpoint: endpoint.to_owned(),
                p256dh: "fixture-p256dh".to_owned(),
                auth: "fixture-auth".to_owned(),
            },
            notification: Notification {
                title: Some("Hello".to_owned()),
                body: Some("World".to_owned()),
                image_url: Some("https://cdn.example.invalid/image.jpg".to_owned()),
                data: BTreeMap::from([("count".to_owned(), json!(3))]),
            },
            options: PushOptions {
                ttl_seconds: Some(120),
                priority: PushPriority::High,
                collapse_key: Some("inbox".to_owned()),
                dry_run: false,
            },
            trace: TraceMetadata::default(),
        }
    }

    fn generated_config(policy: WebPushHostPolicy) -> WebPushConfig {
        let key = SecretKey::random(&mut OsRng);
        let pem = key
            .to_pkcs8_pem(LineEnding::LF)
            .expect("encode test VAPID key");
        WebPushConfig::new(pem.as_str(), "mailto:push@example.invalid", policy)
            .expect("test config")
    }

    #[test]
    fn default_allowlist_matches_exact_hosts_and_real_subdomains_only() {
        let policy = WebPushHostPolicy::strict_default();
        let allowed = policy.allowed_hosts().expect("allowlist");
        assert!(allowed.iter().any(|host| host == "fcm.googleapis.com"));
        assert!(host_matches_suffix(
            "updates.push.services.mozilla.com",
            "push.services.mozilla.com"
        ));
        assert!(!host_matches_suffix(
            "push.services.mozilla.com.attacker.invalid",
            "push.services.mozilla.com"
        ));
        assert!(!host_matches_suffix(
            "evilpush.services.mozilla.com",
            "push.services.mozilla.com"
        ));
    }

    #[test]
    fn rejects_unsafe_endpoint_shapes_and_hosts() {
        let policy = WebPushHostPolicy::strict_default();
        for endpoint in [
            "http://fcm.googleapis.com/send/capability",
            "https://user:password@fcm.googleapis.com/send/capability",
            "https://fcm.googleapis.com:8443/send/capability",
            "https://localhost/send/capability",
            "https://127.0.0.1/send/capability",
            "https://169.254.169.254/metadata",
            "https://10.0.0.1/send/capability",
            "https://fcm.googleapis.com/send/capability#fragment",
            "https://fcm.googleapis.com.attacker.invalid/send/capability",
        ] {
            assert!(validate_endpoint(endpoint, &policy).is_err(), "{endpoint}");
        }
    }

    #[test]
    fn blocks_special_ipv4_ipv6_and_mapped_addresses() {
        for address in [
            "0.0.0.1",
            "100.64.0.1",
            "192.0.0.10",
            "198.18.0.1",
            "224.0.0.1",
            "240.0.0.1",
            "::1",
            "fc00::1",
            "fe80::1",
            "2001:db8::1",
            "::ffff:127.0.0.1",
        ] {
            let address = address.parse::<IpAddr>().expect("IP fixture");
            assert!(ip_is_blocked(address), "{address}");
        }
        assert!(!ip_is_blocked("8.8.8.8".parse().expect("public IPv4")));
        assert!(!ip_is_blocked(
            "2606:4700:4700::1111".parse().expect("public IPv6")
        ));
    }

    #[test]
    fn validates_subjects_keys_allowlists_payloads_topics_and_redaction() {
        assert!(validate_vapid_subject("mailto:push@example.invalid").is_ok());
        assert!(validate_vapid_subject("https://example.invalid/contact").is_ok());
        assert!(validate_vapid_subject("http://example.invalid/contact").is_err());
        assert!(WebPushHostPolicy::allowlist(["push.example.invalid"]).is_ok());
        assert!(WebPushHostPolicy::allowlist(["localhost"]).is_err());
        assert!(matches!(
            WebPushConfig::new(
                "not-a-real-vapid-key",
                "mailto:push@example.invalid",
                WebPushHostPolicy::strict_default()
            ),
            Err(WebPushConfigError::InvalidPrivateKey)
        ));

        let config = generated_config(WebPushHostPolicy::strict_default());
        let endpoint = Url::parse("https://fcm.googleapis.com/send/capability").expect("URL");
        let authorization = vapid_authorization(&config, &endpoint, 1_000).expect("VAPID token");
        assert!(authorization.starts_with("vapid t="));
        assert!(authorization.contains(", k="));

        let job = web_push_job(endpoint.as_str());
        let payload = build_payload(&job).expect("payload");
        let document: Value = serde_json::from_slice(&payload).expect("JSON payload");
        assert_eq!(document["title"], "Hello");
        assert_eq!(document["data"]["count"], 3);
        assert_eq!(topic_for_collapse_key("inbox").len(), 32);
        assert_eq!(
            redact_web_push_endpoint("https://fcm.googleapis.com/send/capability?secret=value"),
            "https://fcm.googleapis.com/…"
        );
    }

    #[test]
    fn encrypts_subscription_payload() {
        let (receiver_key, auth_secret) =
            ece::generate_keypair_and_auth_secret().expect("receiver key material");
        let receiver_public_key = receiver_key
            .pub_as_raw()
            .expect("encode receiver public key");
        let p256dh = URL_SAFE_NO_PAD.encode(&receiver_public_key);
        let auth = URL_SAFE_NO_PAD.encode(auth_secret);
        let plaintext = b"web push fixture";
        let encrypted =
            encrypt_subscription_payload(&p256dh, &auth, plaintext).expect("encrypt payload");
        assert!(encrypted.len() > plaintext.len());
    }

    #[test]
    fn rejects_invalid_subscription_key_material() {
        let error = encrypt_subscription_payload("not-base64", "also-not-base64", b"fixture")
            .expect_err("invalid subscription keys must fail");
        assert!(matches!(
            error,
            ProviderError::Delivery {
                class: OutcomeClass::InvalidPayload,
                ..
            }
        ));
    }

    #[test]
    fn classifies_provider_responses_without_target_details() {
        let job = web_push_job("https://fcm.googleapis.com/send/capability");
        for (status, expected) in [
            (StatusCode::CREATED, OutcomeClass::Accepted),
            (StatusCode::GONE, OutcomeClass::InvalidToken),
            (StatusCode::PAYLOAD_TOO_LARGE, OutcomeClass::InvalidPayload),
            (StatusCode::TOO_MANY_REQUESTS, OutcomeClass::Throttled),
            (
                StatusCode::SERVICE_UNAVAILABLE,
                OutcomeClass::TransientProviderFailure,
            ),
            (
                StatusCode::FORBIDDEN,
                OutcomeClass::PermanentProviderFailure,
            ),
        ] {
            let outcome = classify_response(&job, status, None).expect("outcome");
            assert_eq!(outcome.class, expected);
            assert!(
                !outcome
                    .safe_detail
                    .as_deref()
                    .unwrap_or_default()
                    .contains("capability")
            );
        }
    }

    #[tokio::test]
    async fn rejects_private_endpoint_before_crypto_or_network_work() {
        let provider = WebPushProvider::new(generated_config(WebPushHostPolicy::any_public()))
            .expect("provider");
        let error = provider
            .send(&web_push_job("https://127.0.0.1/send/capability"))
            .await
            .expect_err("private endpoint must fail");
        assert!(matches!(
            error,
            ProviderError::Delivery {
                class: OutcomeClass::InvalidPayload,
                ..
            }
        ));
    }
}
