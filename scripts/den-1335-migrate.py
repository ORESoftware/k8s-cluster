from pathlib import Path

path = Path("src/service.rs")
text = path.read_text()

old = "const SHARED_AUTH_INTROSPECTION_MAX_BYTES: usize = 64 * 1024;\n"
new = '''const SHARED_AUTH_INTROSPECTION_MAX_BYTES: usize = 64 * 1024;
const SHARED_AUTH_REQUIRED_ACR: &str = "urn:oresoftware:loa:2";
const SHARED_AUTH_MAX_AUTH_AGE_SECONDS: u64 = 15 * 60;
const SHARED_AUTH_CLOCK_SKEW_SECONDS: u64 = 60;
const SHARED_AUTH_MAX_AMR_METHODS: usize = 16;
const SHARED_AUTH_MAX_AMR_METHOD_BYTES: usize = 64;
'''
if old not in text:
    raise SystemExit("Shared Auth constants insertion point not found")
text = text.replace(old, new, 1)

old = '''#[derive(Debug, Deserialize)]
struct SharedAuthIntrospection {
    active: bool,
    #[serde(default)]
    sub: Option<String>,
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    email_verified: bool,
    #[serde(default = "default_auth_assurance_level")]
    aal: u8,
}

fn default_auth_assurance_level() -> u8 {
    1
}
'''
new = '''#[derive(Debug, Deserialize)]
struct SharedAuthIntrospection {
    active: bool,
    #[serde(default)]
    sub: Option<String>,
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    email_verified: bool,
    #[serde(default = "default_auth_assurance_level")]
    aal: u8,
    #[serde(default)]
    amr: Vec<String>,
    #[serde(default)]
    acr: Option<String>,
    #[serde(default)]
    iat: Option<u64>,
}

fn default_auth_assurance_level() -> u8 {
    1
}

fn validate_shared_auth_assurance(
    introspection: &SharedAuthIntrospection,
    required_aal: u8,
    now: u64,
) -> Result<(), ServiceError> {
    if introspection.aal < required_aal {
        return Err(ServiceError::MfaRequired);
    }
    if required_aal < 2 {
        return Ok(());
    }
    if introspection.acr.as_deref() != Some(SHARED_AUTH_REQUIRED_ACR)
        || introspection.amr.is_empty()
        || introspection.amr.len() > SHARED_AUTH_MAX_AMR_METHODS
    {
        return Err(ServiceError::MfaRequired);
    }

    let mut methods = Vec::with_capacity(introspection.amr.len());
    for raw in &introspection.amr {
        let method = raw.trim().to_ascii_lowercase();
        if method.is_empty()
            || method.len() > SHARED_AUTH_MAX_AMR_METHOD_BYTES
            || !method.bytes().all(|byte| {
                byte.is_ascii_lowercase()
                    || byte.is_ascii_digit()
                    || b"_:-./".contains(&byte)
            })
            || methods.contains(&method)
        {
            return Err(ServiceError::MfaRequired);
        }
        methods.push(method);
    }

    if methods
        .iter()
        .any(|method| matches!(method.as_str(), "pwd" | "password"))
    {
        return Err(ServiceError::MfaRequired);
    }
    let passwordless_primary = methods.iter().any(|method| {
        matches!(
            method.as_str(),
            "federated"
                | "email_otp"
                | "otp"
                | "magiclink"
                | "magic_link"
                | "email/signup"
        )
    });
    let strong_second = methods.iter().any(|method| {
        matches!(
            method.as_str(),
            "totp" | "sms_otp" | "passkey" | "webauthn"
        )
    });
    if !passwordless_primary || !strong_second {
        return Err(ServiceError::MfaRequired);
    }

    let issued_at = introspection.iat.ok_or(ServiceError::MfaRequired)?;
    if issued_at > now.saturating_add(SHARED_AUTH_CLOCK_SKEW_SECONDS)
        || now.saturating_sub(issued_at) > SHARED_AUTH_MAX_AUTH_AGE_SECONDS
    {
        return Err(ServiceError::MfaRequired);
    }
    Ok(())
}

#[cfg(test)]
mod shared_auth_assurance_tests {
    use super::*;

    const NOW: u64 = 2_000_000_000;

    fn claims(
        amr: &[&str],
        aal: u8,
        acr: Option<&str>,
        iat: Option<u64>,
    ) -> SharedAuthIntrospection {
        SharedAuthIntrospection {
            active: true,
            sub: Some(Uuid::nil().to_string()),
            email: None,
            email_verified: false,
            aal,
            amr: amr.iter().map(|method| (*method).to_string()).collect(),
            acr: acr.map(str::to_string),
            iat,
        }
    }

    #[test]
    fn accepts_passwordless_primary_with_approved_independent_second_factor() {
        for methods in [
            vec!["federated", "totp"],
            vec!["email_otp", "sms_otp"],
            vec!["federated", "passkey"],
        ] {
            let value = claims(
                &methods,
                2,
                Some(SHARED_AUTH_REQUIRED_ACR),
                Some(NOW - 30),
            );
            assert!(validate_shared_auth_assurance(&value, 2, NOW).is_ok());
        }
    }

    #[test]
    fn rejects_numeric_aal2_without_the_canonical_method_chain() {
        for methods in [
            vec![],
            vec!["federated"],
            vec!["email_otp"],
            vec!["pwd", "totp"],
            vec!["federated", "email_otp"],
            vec!["federated", "totp", "totp"],
        ] {
            let value = claims(
                &methods,
                2,
                Some(SHARED_AUTH_REQUIRED_ACR),
                Some(NOW - 30),
            );
            assert!(matches!(
                validate_shared_auth_assurance(&value, 2, NOW),
                Err(ServiceError::MfaRequired)
            ));
        }
    }

    #[test]
    fn rejects_missing_wrong_stale_or_future_assurance_context() {
        let missing_acr = claims(&["federated", "totp"], 2, None, Some(NOW));
        let stale = claims(
            &["federated", "totp"],
            2,
            Some(SHARED_AUTH_REQUIRED_ACR),
            Some(NOW - SHARED_AUTH_MAX_AUTH_AGE_SECONDS - 1),
        );
        let future = claims(
            &["federated", "totp"],
            2,
            Some(SHARED_AUTH_REQUIRED_ACR),
            Some(NOW + SHARED_AUTH_CLOCK_SKEW_SECONDS + 1),
        );
        let missing_iat = claims(
            &["federated", "totp"],
            2,
            Some(SHARED_AUTH_REQUIRED_ACR),
            None,
        );
        for value in [missing_acr, stale, future, missing_iat] {
            assert!(matches!(
                validate_shared_auth_assurance(&value, 2, NOW),
                Err(ServiceError::MfaRequired)
            ));
        }
    }
}
'''
if old not in text:
    raise SystemExit("SharedAuthIntrospection block not found")
text = text.replace(old, new, 1)

old = '''    if introspection.aal < config.required_aal {
        return Err(ServiceError::MfaRequired);
    }
'''
new = '''    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    validate_shared_auth_assurance(&introspection, config.required_aal, now)?;
'''
if old not in text:
    raise SystemExit("Shared Auth assurance check not found")
path.write_text(text.replace(old, new, 1))
