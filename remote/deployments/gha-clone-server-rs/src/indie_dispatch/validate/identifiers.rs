const MAX_IDENTIFIER_BYTES: usize = 128;
const MAX_PROFILE_NAME_BYTES: usize = 64;

pub(super) fn validate_request_id(value: &str) -> Result<(), String> {
    let Some(component) = value.strip_prefix("gha:") else {
        return Err("requestId must use gha:<48 lowercase hex>".to_string());
    };
    if component.len() != 48 || !is_lower_hex(component) {
        return Err("requestId must use gha:<48 lowercase hex>".to_string());
    }
    Ok(())
}

pub(super) fn validate_digest(name: &str, value: &str) -> Result<(), String> {
    let Some(component) = value.strip_prefix("sha256:") else {
        return Err(format!("{name} must use sha256:<64 lowercase hex>"));
    };
    if component.len() != 64 || !is_lower_hex(component) {
        return Err(format!("{name} must use sha256:<64 lowercase hex>"));
    }
    Ok(())
}

pub(super) fn validate_commit_sha(value: &str) -> Result<(), String> {
    if value.len() != 40 || !is_lower_hex(value) {
        return Err("commitSha must be exactly 40 lowercase hexadecimal characters".to_string());
    }
    Ok(())
}

pub(super) fn validate_base_job_id(name: &str, value: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > MAX_IDENTIFIER_BYTES
        || !value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-'))
    {
        return Err(format!(
            "{name} must contain only ASCII letters, digits, '_' or '-' and be 1-{MAX_IDENTIFIER_BYTES} bytes"
        ));
    }
    Ok(())
}

pub(super) fn validate_instance_id(name: &str, value: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > MAX_IDENTIFIER_BYTES
        || !value.chars().all(|character| {
            character.is_ascii_alphanumeric()
                || matches!(character, '_' | '-' | '[' | ']' | '.' | ',')
        })
    {
        return Err(format!(
            "{name} contains unsupported characters or exceeds {MAX_IDENTIFIER_BYTES} bytes"
        ));
    }
    Ok(())
}

pub(super) fn validate_profile_name(value: &str) -> Result<(), String> {
    if value.is_empty() || value.len() > MAX_PROFILE_NAME_BYTES {
        return Err(format!("profile must be 1-{MAX_PROFILE_NAME_BYTES} bytes"));
    }
    let mut characters = value.chars();
    let Some(first) = characters.next() else {
        return Err("profile must not be empty".to_string());
    };
    if !first.is_ascii_lowercase() && !first.is_ascii_digit() {
        return Err("profile must start with a lowercase letter or digit".to_string());
    }
    if !characters.all(|character| {
        character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
    }) {
        return Err("profile may contain lowercase letters, digits, and '-' only".to_string());
    }
    Ok(())
}

fn is_lower_hex(value: &str) -> bool {
    value
        .bytes()
        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
