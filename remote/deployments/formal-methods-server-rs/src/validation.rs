use std::{
    path::{Component, Path, PathBuf},
    sync::atomic::Ordering,
};

use axum::{
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;
use subtle::ConstantTimeEq;

use crate::{
    state::{AppState, Config},
    types::AnalyzeRequest,
    SCHEMA_VERSION,
};

fn request_is_authorized(headers: &HeaderMap, secret: &str) -> bool {
    headers
        .get("x-server-auth")
        .or_else(|| headers.get("x-formal-methods-auth"))
        .or_else(|| headers.get("x-agent-auth"))
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.as_bytes().ct_eq(secret.as_bytes()).into())
}

pub(crate) fn require_auth(headers: &HeaderMap, state: &AppState) -> Result<(), Response> {
    let Some(secret) = state.config.server_auth_secret.as_deref() else {
        state.counters.rejected.fetch_add(1, Ordering::Relaxed);
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "SERVER_AUTH_SECRET is not configured" })),
        )
            .into_response());
    };
    if !request_is_authorized(headers, secret) {
        state.counters.rejected.fetch_add(1, Ordering::Relaxed);
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(json!({
                "error": "unauthorized",
                "errMessage": "missing required formal-methods server auth header",
            })),
        )
            .into_response());
    }
    Ok(())
}

pub(crate) fn clean_optional(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

pub(crate) fn ensure_allowed_prefix(
    name: &str,
    value: &str,
    prefixes: &[String],
    env_name: &str,
) -> Result<(), String> {
    let allowed = prefixes.is_empty()
        || prefixes.iter().any(|prefix| {
            let prefix = prefix.trim();
            value == prefix
                || value.strip_prefix(prefix).is_some_and(|remainder| {
                    prefix.ends_with(['/', ':']) || remainder.starts_with(['/', ':'])
                })
        });
    if allowed {
        Ok(())
    } else {
        Err(format!("{name} is not allowed by {env_name}"))
    }
}

fn repo_path_is_safe(path: &str) -> bool {
    let path = path.trim_matches('/');
    !path.is_empty()
        && path.len() <= 512
        && path.split('/').all(|part| {
            !part.is_empty()
                && part != "."
                && part != ".."
                && !part.chars().any(|character| {
                    character.is_control()
                        || character.is_whitespace()
                        || matches!(character, '\\' | '?' | '#')
                })
        })
}

pub(crate) fn validate_repo_url(repo_url: &str) -> Result<(), String> {
    let repo_url = repo_url.trim();
    if repo_url.is_empty() {
        return Err("repoUrl is required".to_string());
    }
    if repo_url.len() > 2048 {
        return Err("repoUrl must be 2048 characters or fewer".to_string());
    }
    if repo_url.chars().any(char::is_control) {
        return Err("repoUrl must not contain control characters".to_string());
    }
    let lower_repo_url = repo_url.to_ascii_lowercase();
    if lower_repo_url.contains("/../")
        || lower_repo_url.contains("/./")
        || lower_repo_url.contains("%2e")
    {
        return Err("repoUrl repository path must be normalized".to_string());
    }
    if let Some(scp_style) = repo_url.strip_prefix("git@") {
        let Some((host, path)) = scp_style.split_once(':') else {
            return Err("git@ repoUrl must include a host and repository path".to_string());
        };
        if host.is_empty()
            || !host
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-'))
            || !repo_path_is_safe(path)
        {
            return Err("git@ repoUrl has an invalid host or repository path".to_string());
        }
        return Ok(());
    }

    let parsed = reqwest::Url::parse(repo_url)
        .map_err(|_| "repoUrl must be a valid https:// or ssh:// URL".to_string())?;
    if !matches!(parsed.scheme(), "https" | "ssh") {
        return Err("repoUrl must use https://, ssh://, or git@".to_string());
    }
    if parsed.host_str().is_none()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
        || !repo_path_is_safe(parsed.path())
    {
        return Err("repoUrl must contain only a host and repository path".to_string());
    }
    if parsed.scheme() == "https" && (!parsed.username().is_empty() || parsed.password().is_some())
    {
        return Err("https repoUrl must not embed credentials".to_string());
    }
    if parsed.scheme() == "ssh"
        && ((!parsed.username().is_empty() && parsed.username() != "git")
            || parsed.password().is_some())
    {
        return Err("ssh repoUrl may only use the git username and no password".to_string());
    }
    Ok(())
}

pub(crate) fn validate_git_ref(git_ref: &str) -> Result<(), String> {
    if git_ref.is_empty() || git_ref.len() > 180 {
        return Err("gitRef must contain between 1 and 180 bytes".to_string());
    }
    if git_ref.starts_with('-')
        || git_ref.starts_with('/')
        || git_ref.ends_with('/')
        || git_ref.ends_with('.')
        || git_ref.contains("..")
        || git_ref.contains("//")
        || git_ref.contains("@{")
        || git_ref.ends_with(".lock")
        || !git_ref
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'/'))
    {
        return Err("gitRef is not a safe branch, tag, or commit name".to_string());
    }
    Ok(())
}

pub(crate) fn validate_relative_path(name: &str, value: &str) -> Result<PathBuf, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(format!("{name} must not be empty"));
    }
    if trimmed.len() > 240 {
        return Err(format!("{name} must be 240 characters or fewer"));
    }
    let path = Path::new(trimmed);
    if path.is_absolute() {
        return Err(format!("{name} must be relative to the repository root"));
    }
    let mut clean = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(value) => clean.push(value),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(format!("{name} must stay inside the repository root"));
            }
        }
    }
    if clean.as_os_str().is_empty() {
        clean.push(".");
    }
    Ok(clean)
}

pub(crate) fn validate_analyze_request(
    config: &Config,
    request: &AnalyzeRequest,
) -> Result<(), String> {
    if let Some(schema_version) = clean_optional(request.schema_version.as_deref()) {
        if schema_version != SCHEMA_VERSION {
            return Err(format!("schemaVersion must be {SCHEMA_VERSION}"));
        }
    }
    let has_repo = clean_optional(request.repo_url.as_deref()).is_some();
    let has_inline = clean_optional(request.inline_source.as_deref()).is_some();
    if !has_repo && !has_inline {
        return Err("either repoUrl or inlineSource is required".to_string());
    }
    if has_repo && has_inline {
        return Err("only one of repoUrl or inlineSource may be set".to_string());
    }
    if has_repo {
        let repo_url = request.repo_url.as_deref().unwrap().trim();
        validate_repo_url(repo_url)?;
        ensure_allowed_prefix(
            "repoUrl",
            repo_url,
            &config.allowed_repo_prefixes,
            "FORMAL_METHODS_ALLOWED_REPO_PREFIXES",
        )?;
        if let Some(git_ref) = clean_optional(request.git_ref.as_deref()) {
            validate_git_ref(&git_ref)?;
        }
        if let Some(paths) = request.paths.as_ref() {
            if paths.len() > 64 {
                return Err("paths must contain at most 64 entries".to_string());
            }
            for path in paths {
                validate_relative_path("paths[]", path)?;
            }
        }
    }
    if has_inline {
        let source = request.inline_source.as_deref().unwrap();
        if source.len() > config.max_inline_source_bytes {
            return Err(format!(
                "inlineSource must be {} bytes or fewer",
                config.max_inline_source_bytes
            ));
        }
        if let Some(name) = clean_optional(request.inline_filename.as_deref()) {
            validate_relative_path("inlineFilename", &name)?;
        }
    }
    if let Some(languages) = request.languages.as_ref() {
        if languages.len() > 32 {
            return Err("languages must contain at most 32 entries".to_string());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repository_urls_reject_credentials_and_non_network_schemes() {
        assert!(validate_repo_url("https://github.com/ORESoftware/repo.git").is_ok());
        assert!(validate_repo_url("ssh://git@github.com/ORESoftware/repo.git").is_ok());
        assert!(validate_repo_url("git@github.com:ORESoftware/repo.git").is_ok());
        assert!(validate_repo_url("https://token@github.com/ORESoftware/repo.git").is_err());
        assert!(validate_repo_url("file:///etc/passwd").is_err());
        assert!(validate_repo_url("https://github.com/../admin").is_err());
    }

    #[test]
    fn allowed_prefixes_require_a_url_boundary() {
        let prefixes = vec!["https://github.com/ORESoftware".to_string()];
        assert!(ensure_allowed_prefix(
            "repoUrl",
            "https://github.com/ORESoftware/repo.git",
            &prefixes,
            "TEST"
        )
        .is_ok());
        assert!(ensure_allowed_prefix(
            "repoUrl",
            "https://github.com/ORESoftware-evil/repo.git",
            &prefixes,
            "TEST"
        )
        .is_err());
    }

    #[test]
    fn git_refs_reject_option_and_ref_syntax_injection() {
        for valid in ["main", "release/v1.2.3", "0123456789abcdef"] {
            assert!(validate_git_ref(valid).is_ok(), "{valid}");
        }
        for invalid in [
            "--upload-pack=evil",
            "../main",
            "refs//heads/main",
            "main@{1}",
            "x.lock",
        ] {
            assert!(validate_git_ref(invalid).is_err(), "{invalid}");
        }
    }
}
