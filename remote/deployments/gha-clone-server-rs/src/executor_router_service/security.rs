use super::*;

pub(super) fn require_auth(headers: &HeaderMap, state: &AppState) -> Result<(), Box<Response>> {
    let primary = headers.get_all("x-build-server-auth");
    let alternate = headers.get_all("x-server-auth");
    let mut values = primary.iter().chain(alternate.iter());
    let Some(value) = values.next() else {
        return unauthorized(state);
    };
    if values.next().is_some() {
        return unauthorized(state);
    }
    let presented = match value.to_str() {
        Ok(value) => value,
        Err(_) => return unauthorized(state),
    };
    if digest_eq(presented, &state.config.inbound_auth) {
        Ok(())
    } else {
        unauthorized(state)
    }
}

fn unauthorized(state: &AppState) -> Result<(), Box<Response>> {
    state.metrics.rejected_total.fetch_add(1, Ordering::Relaxed);
    Err(Box::new(
        (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "unauthorized" })),
        )
            .into_response(),
    ))
}

pub(super) fn read_secret(path: &Path, root: &Path, label: &str) -> Result<String, String> {
    if !path.is_absolute() || path.parent() != Some(root) {
        return Err(format!(
            "{label} path must be an absolute direct child of {}",
            root.display()
        ));
    }
    let canonical_root = fs::canonicalize(root)
        .map_err(|error| format!("secret root {} is unavailable: {error}", root.display()))?;
    let canonical_path = fs::canonicalize(path)
        .map_err(|error| format!("{label} file {} is unavailable: {error}", path.display()))?;
    if !canonical_path.starts_with(&canonical_root) {
        return Err(format!("{label} file escapes the configured secret root"));
    }
    let raw = fs::read_to_string(&canonical_path)
        .map_err(|error| format!("{label} file could not be read: {error}"))?;
    let value = raw.trim().to_string();
    if value.len() < MIN_SECRET_BYTES
        || value.len() > MAX_SECRET_BYTES
        || value.as_bytes().contains(&0)
        || value.contains('\n')
        || value.contains('\r')
    {
        return Err(format!(
            "{label} must contain between {MIN_SECRET_BYTES} and {MAX_SECRET_BYTES} non-NUL bytes"
        ));
    }
    Ok(value)
}

pub(super) fn env_optional(name: &str) -> Option<String> {
    env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

pub(super) fn env_required(name: &str) -> Result<String, String> {
    env_optional(name).ok_or_else(|| format!("{name} is required"))
}

pub(super) fn env_bool(name: &str, default: bool) -> Result<bool, String> {
    match env_optional(name) {
        Some(value) => match value.to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => Ok(true),
            "0" | "false" | "no" | "off" => Ok(false),
            _ => Err(format!("{name} must be true or false")),
        },
        None => Ok(default),
    }
}

pub(super) fn env_u16(name: &str, default: u16) -> Result<u16, String> {
    env_optional(name)
        .map(|value| {
            value
                .parse::<u16>()
                .map_err(|error| format!("{name} is invalid: {error}"))
                .and_then(|value| {
                    if value == 0 {
                        Err(format!("{name} must be positive"))
                    } else {
                        Ok(value)
                    }
                })
        })
        .transpose()
        .map(|value| value.unwrap_or(default))
}

pub(super) fn env_u64(name: &str, default: u64) -> Result<u64, String> {
    env_optional(name)
        .map(|value| {
            value
                .parse::<u64>()
                .map_err(|error| format!("{name} is invalid: {error}"))
                .and_then(|value| {
                    if value == 0 {
                        Err(format!("{name} must be positive"))
                    } else {
                        Ok(value)
                    }
                })
        })
        .transpose()
        .map(|value| value.unwrap_or(default))
}

pub(super) fn env_usize(name: &str, default: usize) -> Result<usize, String> {
    env_optional(name)
        .map(|value| {
            value
                .parse::<usize>()
                .map_err(|error| format!("{name} is invalid: {error}"))
                .and_then(|value| {
                    if value == 0 {
                        Err(format!("{name} must be positive"))
                    } else {
                        Ok(value)
                    }
                })
        })
        .transpose()
        .map(|value| value.unwrap_or(default))
}

pub(super) async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("install Ctrl-C handler");
    };
    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("install SIGTERM handler")
            .recv()
            .await;
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}
