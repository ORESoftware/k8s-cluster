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
        .is_some_and(|value| value == secret)
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
    if prefixes.is_empty() || prefixes.iter().any(|prefix| value.starts_with(prefix)) {
        Ok(())
    } else {
        Err(format!("{name} is not allowed by {env_name}"))
    }
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
    if repo_url.starts_with("https://")
        || repo_url.starts_with("ssh://")
        || repo_url.starts_with("git@")
    {
        Ok(())
    } else {
        Err("repoUrl must use https://, ssh://, or git@".to_string())
    }
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

pub(crate) fn validate_analyze_request(config: &Config, request: &AnalyzeRequest) -> Result<(), String> {
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
            if git_ref.len() > 180 || git_ref.chars().any(|c| c.is_control() || c.is_whitespace()) {
                return Err("gitRef must be a single token of at most 180 chars".to_string());
            }
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
