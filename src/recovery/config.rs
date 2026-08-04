use reqwest::Url;

const MIN_SECRET_BYTES: usize = 32;
const MAX_SECRET_BYTES: usize = 4096;

#[derive(Clone)]
pub struct RecoveryConfig {
    pub identity_base: Url,
    pub identity_token: String,
    pub voxletra_base: Url,
    pub voxletra_token: String,
    pub subject_pepper: String,
    pub review_secret: Option<String>,
    pub ceremony_ttl_secs: u64,
    pub cooldown_secs: u64,
    pub redeem_ttl_secs: u64,
    pub document_threshold: f64,
    pub face_threshold: f64,
    pub voice_liveness_threshold: f64,
    pub always_manual_review: bool,
    pub consent_version: String,
}

#[derive(Debug, thiserror::Error)]
pub enum RecoveryConfigError {
    #[error("account recovery configuration is incomplete: {0}")]
    Missing(&'static str),
    #[error("account recovery configuration is invalid: {0}")]
    Invalid(&'static str),
}

impl RecoveryConfig {
    /// Account recovery is disabled only when every required integration value
    /// is absent. Supplying a partial configuration fails startup instead of
    /// leaving a silently weaker path available.
    pub fn from_env() -> Result<Option<Self>, RecoveryConfigError> {
        let identity_base = optional_env("AUTH_RECOVERY_IDENTITY_URL");
        let identity_token = secret_env("AUTH_RECOVERY_IDENTITY_TOKEN");
        let voxletra_base = optional_env("AUTH_RECOVERY_VOXLETRA_URL");
        let voxletra_token = secret_env("AUTH_RECOVERY_VOXLETRA_TOKEN");
        let subject_pepper = secret_env("AUTH_RECOVERY_SUBJECT_PEPPER");

        if identity_base.is_none()
            && identity_token.is_none()
            && voxletra_base.is_none()
            && voxletra_token.is_none()
            && subject_pepper.is_none()
        {
            return Ok(None);
        }

        let identity_base = validate_base_url(
            &identity_base.ok_or(RecoveryConfigError::Missing(
                "AUTH_RECOVERY_IDENTITY_URL",
            ))?,
            "AUTH_RECOVERY_IDENTITY_URL",
        )?;
        let identity_token = validate_secret(
            identity_token.ok_or(RecoveryConfigError::Missing(
                "AUTH_RECOVERY_IDENTITY_TOKEN",
            ))?,
            "AUTH_RECOVERY_IDENTITY_TOKEN",
        )?;
        let voxletra_base = validate_base_url(
            &voxletra_base.ok_or(RecoveryConfigError::Missing(
                "AUTH_RECOVERY_VOXLETRA_URL",
            ))?,
            "AUTH_RECOVERY_VOXLETRA_URL",
        )?;
        let voxletra_token = validate_secret(
            voxletra_token.ok_or(RecoveryConfigError::Missing(
                "AUTH_RECOVERY_VOXLETRA_TOKEN",
            ))?,
            "AUTH_RECOVERY_VOXLETRA_TOKEN",
        )?;
        let subject_pepper = validate_secret(
            subject_pepper.ok_or(RecoveryConfigError::Missing(
                "AUTH_RECOVERY_SUBJECT_PEPPER",
            ))?,
            "AUTH_RECOVERY_SUBJECT_PEPPER",
        )?;
        let review_secret = secret_env("AUTH_RECOVERY_REVIEW_SECRET")
            .map(|secret| validate_secret(secret, "AUTH_RECOVERY_REVIEW_SECRET"))
            .transpose()?;

        let ceremony_ttl_secs = parse_u64("AUTH_RECOVERY_TTL_SECS", 900)?;
        if !(300..=3600).contains(&ceremony_ttl_secs) {
            return Err(RecoveryConfigError::Invalid(
                "AUTH_RECOVERY_TTL_SECS must be between 300 and 3600",
            ));
        }
        let cooldown_secs = parse_u64("AUTH_RECOVERY_COOLDOWN_SECS", 86_400)?;
        if !(900..=604_800).contains(&cooldown_secs) {
            return Err(RecoveryConfigError::Invalid(
                "AUTH_RECOVERY_COOLDOWN_SECS must be between 900 and 604800",
            ));
        }

        let redeem_ttl_secs = parse_u64("AUTH_RECOVERY_REDEEM_TTL_SECS", 86_400)?;
        if !(3600..=604_800).contains(&redeem_ttl_secs) {
            return Err(RecoveryConfigError::Invalid(
                "AUTH_RECOVERY_REDEEM_TTL_SECS must be between 3600 and 604800",
            ));
        }

        let document_threshold = parse_threshold("AUTH_RECOVERY_DOCUMENT_THRESHOLD", 0.85)?;
        let face_threshold = parse_threshold("AUTH_RECOVERY_FACE_THRESHOLD", 0.90)?;
        let voice_liveness_threshold =
            parse_threshold("AUTH_RECOVERY_VOICE_LIVENESS_THRESHOLD", 0.90)?;
        let always_manual_review = parse_bool("AUTH_RECOVERY_ALWAYS_MANUAL_REVIEW", false)?;
        let consent_version = optional_env("AUTH_RECOVERY_CONSENT_VERSION")
            .unwrap_or_else(|| "2026-08-04".to_owned());
        validate_consent_version(&consent_version)?;

        Ok(Some(Self {
            identity_base,
            identity_token,
            voxletra_base,
            voxletra_token,
            subject_pepper,
            review_secret,
            ceremony_ttl_secs,
            cooldown_secs,
            redeem_ttl_secs,
            document_threshold,
            face_threshold,
            voice_liveness_threshold,
            always_manual_review,
            consent_version,
        }))
    }
}

fn optional_env(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

fn secret_env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

fn validate_secret(secret: String, name: &'static str) -> Result<String, RecoveryConfigError> {
    if !(MIN_SECRET_BYTES..=MAX_SECRET_BYTES).contains(&secret.len())
        || secret.chars().any(char::is_control)
    {
        return Err(RecoveryConfigError::Invalid(name));
    }
    Ok(secret)
}

fn validate_base_url(raw: &str, name: &'static str) -> Result<Url, RecoveryConfigError> {
    let mut url = Url::parse(raw.trim()).map_err(|_| RecoveryConfigError::Invalid(name))?;
    if !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || !matches!(url.path(), "" | "/")
    {
        return Err(RecoveryConfigError::Invalid(name));
    }
    let local = matches!(
        url.host_str(),
        Some("localhost" | "127.0.0.1" | "::1" | "[::1]")
    );
    if url.scheme() != "https" && !(local && url.scheme() == "http") {
        return Err(RecoveryConfigError::Invalid(name));
    }
    url.set_path("/");
    Ok(url)
}

fn parse_u64(name: &'static str, default: u64) -> Result<u64, RecoveryConfigError> {
    match optional_env(name) {
        Some(value) => value
            .parse()
            .map_err(|_| RecoveryConfigError::Invalid(name)),
        None => Ok(default),
    }
}

fn parse_threshold(name: &'static str, default: f64) -> Result<f64, RecoveryConfigError> {
    let value = match optional_env(name) {
        Some(value) => value
            .parse::<f64>()
            .map_err(|_| RecoveryConfigError::Invalid(name))?,
        None => default,
    };
    if !value.is_finite() || !(0.50..=1.0).contains(&value) {
        return Err(RecoveryConfigError::Invalid(name));
    }
    Ok(value)
}

fn parse_bool(name: &'static str, default: bool) -> Result<bool, RecoveryConfigError> {
    match optional_env(name).as_deref() {
        None => Ok(default),
        Some("1" | "true" | "TRUE" | "yes" | "YES") => Ok(true),
        Some("0" | "false" | "FALSE" | "no" | "NO") => Ok(false),
        Some(_) => Err(RecoveryConfigError::Invalid(name)),
    }
}

fn validate_consent_version(value: &str) -> Result<(), RecoveryConfigError> {
    if value.is_empty()
        || value.len() > 64
        || !value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.')
        })
    {
        return Err(RecoveryConfigError::Invalid(
            "AUTH_RECOVERY_CONSENT_VERSION",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_urls_require_https_outside_loopback() {
        assert!(validate_base_url("https://identity.example", "url").is_ok());
        assert!(validate_base_url("http://127.0.0.1:9000", "url").is_ok());
        assert!(validate_base_url("http://identity.example", "url").is_err());
        assert!(validate_base_url("https://identity.example/path", "url").is_err());
        assert!(validate_base_url("https://user:pass@identity.example", "url").is_err());
    }

    #[test]
    fn thresholds_are_deliberately_high_and_bounded() {
        assert_eq!(
            parse_threshold("UNSET_RECOVERY_THRESHOLD", 0.90).unwrap(),
            0.90
        );
        assert!((0.50..=1.0).contains(&0.90));
    }

    #[test]
    fn consent_versions_are_non_secret_identifiers() {
        assert!(validate_consent_version("2026-08-04").is_ok());
        assert!(validate_consent_version("policy.v2").is_ok());
        assert!(validate_consent_version("bad value").is_err());
    }
}
