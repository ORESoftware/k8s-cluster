use std::env;

use axum::http::HeaderMap;
use serde::Serialize;
use serde_json::{Map, Value};

const MAX_USER_ID_BYTES: usize = 160;
const MAX_ROLE_BYTES: usize = 64;
const MAX_ROLES: usize = 20;
const MAX_METADATA_BYTES: usize = 4 * 1024;
const MAX_METADATA_KEYS: usize = 32;
const MAX_METADATA_KEY_BYTES: usize = 64;
const MIN_PROXY_SECRET_BYTES: usize = 32;

const VERIFIED_HEADER: &str = "x-shared-auth-verified";
const PROXY_SECRET_HEADER: &str = "x-streempilot-auth-proxy";
const USER_ID_HEADER: &str = "x-auth-user-id";
const ROLES_HEADER: &str = "x-auth-roles";
const AAL_HEADER: &str = "x-auth-aal";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SignalAuthMode {
    Disabled,
    Optional,
    Required,
}

impl SignalAuthMode {
    fn parse(value: Option<String>) -> Result<Self, String> {
        match value
            .as_deref()
            .map(str::trim)
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            None | Some("") | Some("optional") => Ok(Self::Optional),
            Some("disabled") | Some("off") | Some("none") => Ok(Self::Disabled),
            Some("required") | Some("on") => Ok(Self::Required),
            Some(other) => Err(format!(
                "WEBRTC_AUTH_MODE must be disabled, optional, or required; got {other:?}"
            )),
        }
    }
}

#[derive(Clone, Debug)]
pub struct SignalAuthConfig {
    pub mode: SignalAuthMode,
    proxy_secret: Option<String>,
}

impl SignalAuthConfig {
    pub fn from_env() -> Result<Self, String> {
        let mode = SignalAuthMode::parse(env::var("WEBRTC_AUTH_MODE").ok())?;
        let proxy_secret = env::var("WEBRTC_AUTH_PROXY_SECRET")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());

        if proxy_secret
            .as_ref()
            .is_some_and(|secret| secret.len() < MIN_PROXY_SECRET_BYTES)
        {
            return Err(format!(
                "WEBRTC_AUTH_PROXY_SECRET must be at least {MIN_PROXY_SECRET_BYTES} bytes"
            ));
        }
        if mode == SignalAuthMode::Required && proxy_secret.is_none() {
            return Err(
                "WEBRTC_AUTH_MODE=required also requires WEBRTC_AUTH_PROXY_SECRET".to_string(),
            );
        }

        Ok(Self { mode, proxy_secret })
    }

    #[cfg(test)]
    pub fn disabled() -> Self {
        Self {
            mode: SignalAuthMode::Disabled,
            proxy_secret: None,
        }
    }

    #[cfg(test)]
    fn for_test(mode: SignalAuthMode, proxy_secret: Option<&str>) -> Self {
        Self {
            mode,
            proxy_secret: proxy_secret.map(str::to_string),
        }
    }

    /// Extract the identity produced by shared-auth at the reverse-proxy
    /// boundary. The service never trusts client-provided x-auth-* headers on
    /// their own: a successful auth subrequest must set both the verified bit
    /// and the private proxy-to-service secret.
    pub fn identity_from_headers(
        &self,
        headers: &HeaderMap,
    ) -> Result<Option<PeerIdentity>, SignalAuthError> {
        if self.mode == SignalAuthMode::Disabled {
            return Ok(None);
        }

        let has_auth_headers = headers.contains_key(USER_ID_HEADER)
            || headers.contains_key(ROLES_HEADER)
            || headers.contains_key(AAL_HEADER)
            || headers.contains_key(VERIFIED_HEADER)
            || headers.contains_key(PROXY_SECRET_HEADER);

        if !has_auth_headers {
            return if self.mode == SignalAuthMode::Required {
                Err(SignalAuthError::AuthenticationRequired)
            } else {
                Ok(None)
            };
        }

        let configured_secret = self
            .proxy_secret
            .as_deref()
            .ok_or(SignalAuthError::UntrustedProxy)?;
        let supplied_secret = header_text(headers, PROXY_SECRET_HEADER)
            .ok_or(SignalAuthError::UntrustedProxy)?;
        if !constant_time_eq(configured_secret.as_bytes(), supplied_secret.as_bytes()) {
            return Err(SignalAuthError::UntrustedProxy);
        }
        if header_text(headers, VERIFIED_HEADER) != Some("1") {
            return Err(SignalAuthError::AuthenticationRequired);
        }

        let user_id = header_text(headers, USER_ID_HEADER)
            .filter(|value| valid_bounded_text(value, MAX_USER_ID_BYTES))
            .ok_or(SignalAuthError::InvalidIdentity)?
            .to_string();
        let roles = header_text(headers, ROLES_HEADER)
            .map(parse_roles)
            .unwrap_or_default();
        let aal = header_text(headers, AAL_HEADER)
            .and_then(|value| value.parse::<u8>().ok())
            .filter(|value| *value <= 3);

        Ok(Some(PeerIdentity {
            user_id,
            roles,
            aal,
        }))
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PeerIdentity {
    pub user_id: String,
    pub roles: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aal: Option<u8>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SignalAuthError {
    AuthenticationRequired,
    UntrustedProxy,
    InvalidIdentity,
}

impl SignalAuthError {
    pub fn public_message(self) -> &'static str {
        match self {
            Self::AuthenticationRequired => "shared-auth verification is required",
            Self::UntrustedProxy => "untrusted authentication proxy",
            Self::InvalidIdentity => "shared-auth returned an invalid identity",
        }
    }
}

/// Client metadata is presentation-only. Reserved identity/auth keys are
/// removed so a hello frame can never masquerade as the gateway-authenticated
/// identity serialized separately on `PeerSummary.identity`.
pub fn sanitize_client_metadata(value: Value) -> Result<Value, &'static str> {
    if value.is_null() {
        return Ok(Value::Null);
    }
    let Value::Object(object) = value else {
        return Err("metadata must be a JSON object");
    };
    if object.len() > MAX_METADATA_KEYS {
        return Err("metadata has too many keys");
    }

    let mut clean = Map::new();
    for (key, value) in object {
        if key.is_empty()
            || key.len() > MAX_METADATA_KEY_BYTES
            || key.chars().any(char::is_control)
        {
            return Err("metadata contains an invalid key");
        }
        if is_reserved_metadata_key(&key) {
            continue;
        }
        clean.insert(key, value);
    }

    let sanitized = Value::Object(clean);
    let encoded = serde_json::to_vec(&sanitized).map_err(|_| "metadata is not serializable")?;
    if encoded.len() > MAX_METADATA_BYTES {
        return Err("metadata exceeds 4096 bytes");
    }
    Ok(sanitized)
}

fn is_reserved_metadata_key(key: &str) -> bool {
    matches!(
        key.to_ascii_lowercase().as_str(),
        "auth"
            | "authenticated"
            | "identity"
            | "userid"
            | "user_id"
            | "roles"
            | "aal"
            | "verified"
            | "sharedauth"
            | "shared_auth"
    )
}

fn header_text<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn parse_roles(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|role| valid_role(role))
        .take(MAX_ROLES)
        .map(str::to_string)
        .collect()
}

fn valid_role(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_ROLE_BYTES
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b":._-".contains(&byte))
}

fn valid_bounded_text(value: &str, max_bytes: usize) -> bool {
    !value.is_empty() && value.len() <= max_bytes && !value.chars().any(char::is_control)
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut difference = 0_u8;
    for (left, right) in left.iter().zip(right) {
        difference |= left ^ right;
    }
    difference == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;
    use serde_json::json;

    const SECRET: &str = "0123456789abcdef0123456789abcdef";

    fn verified_headers() -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(VERIFIED_HEADER, HeaderValue::from_static("1"));
        headers.insert(PROXY_SECRET_HEADER, HeaderValue::from_static(SECRET));
        headers.insert(USER_ID_HEADER, HeaderValue::from_static("user-123"));
        headers.insert(
            ROLES_HEADER,
            HeaderValue::from_static("host, producer,invalid role"),
        );
        headers.insert(AAL_HEADER, HeaderValue::from_static("2"));
        headers
    }

    #[test]
    fn optional_mode_allows_a_guest_without_auth_headers() {
        let config = SignalAuthConfig::for_test(SignalAuthMode::Optional, Some(SECRET));
        assert_eq!(config.identity_from_headers(&HeaderMap::new()), Ok(None));
    }

    #[test]
    fn required_mode_rejects_a_guest_without_auth_headers() {
        let config = SignalAuthConfig::for_test(SignalAuthMode::Required, Some(SECRET));
        assert_eq!(
            config.identity_from_headers(&HeaderMap::new()),
            Err(SignalAuthError::AuthenticationRequired)
        );
    }

    #[test]
    fn verified_proxy_headers_create_a_bounded_identity() {
        let config = SignalAuthConfig::for_test(SignalAuthMode::Required, Some(SECRET));
        let identity = config
            .identity_from_headers(&verified_headers())
            .unwrap()
            .unwrap();
        assert_eq!(identity.user_id, "user-123");
        assert_eq!(identity.roles, vec!["host", "producer"]);
        assert_eq!(identity.aal, Some(2));
    }

    #[test]
    fn spoofed_identity_without_proxy_secret_is_rejected() {
        let config = SignalAuthConfig::for_test(SignalAuthMode::Optional, Some(SECRET));
        let mut headers = HeaderMap::new();
        headers.insert(USER_ID_HEADER, HeaderValue::from_static("attacker"));
        assert_eq!(
            config.identity_from_headers(&headers),
            Err(SignalAuthError::UntrustedProxy)
        );
    }

    #[test]
    fn hello_metadata_cannot_override_verified_identity() {
        let sanitized = sanitize_client_metadata(json!({
            "displayName": "Producer Jane",
            "role": "guest",
            "broadcastId": "broadcast-1",
            "identity": {"userId": "attacker"},
            "roles": ["admin"],
            "aal": 3,
            "verified": true
        }))
        .unwrap();
        assert_eq!(sanitized["displayName"], "Producer Jane");
        assert_eq!(sanitized["role"], "guest");
        assert!(sanitized.get("identity").is_none());
        assert!(sanitized.get("roles").is_none());
        assert!(sanitized.get("aal").is_none());
        assert!(sanitized.get("verified").is_none());
    }

    #[test]
    fn metadata_rejects_non_objects_and_oversized_values() {
        assert!(sanitize_client_metadata(json!(["not", "an", "object"])).is_err());
        assert!(sanitize_client_metadata(json!({"note": "x".repeat(5000)})).is_err());
    }
}
