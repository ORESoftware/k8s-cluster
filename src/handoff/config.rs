use std::collections::{HashMap, HashSet};

use serde::Deserialize;
use url::Url;

use crate::config::AppConfig;

use super::MAX_AUTH_VALUE_BYTES;

#[derive(Clone)]
pub(super) struct BrowserClient {
    pub client_id: String,
    pub supabase_project: String,
    pub redirect_uris: HashSet<String>,
    pub return_paths: HashSet<String>,
    pub client_secret: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BrowserClientConfig {
    client_id: String,
    supabase_project: String,
    redirect_uris: Vec<String>,
    return_paths: Vec<String>,
    client_secret_env: String,
}

pub(super) fn load_clients(
    config: &AppConfig,
    raw_clients: &str,
) -> anyhow::Result<HashMap<String, BrowserClient>> {
    let inputs: Vec<BrowserClientConfig> = serde_json::from_str(raw_clients)
        .map_err(|error| anyhow::anyhow!("AUTH_BROWSER_CLIENTS must be a JSON array: {error}"))?;
    if inputs.is_empty() {
        anyhow::bail!("AUTH_BROWSER_CLIENTS must contain at least one client");
    }

    let project_names = config
        .projects
        .iter()
        .map(|project| project.name.as_str())
        .collect::<HashSet<_>>();
    let mut clients = HashMap::with_capacity(inputs.len());
    for input in inputs {
        validate_identifier("client_id", &input.client_id)?;
        validate_identifier("supabase_project", &input.supabase_project)?;
        if !project_names.contains(input.supabase_project.as_str()) {
            anyhow::bail!(
                "browser client {} references unknown Supabase project {}",
                input.client_id,
                input.supabase_project
            );
        }
        if input.redirect_uris.is_empty() || input.return_paths.is_empty() {
            anyhow::bail!(
                "browser client {} requires redirect_uris and return_paths",
                input.client_id
            );
        }
        let redirect_uris = input
            .redirect_uris
            .into_iter()
            .map(validate_redirect_uri)
            .collect::<anyhow::Result<HashSet<_>>>()?;
        let return_paths = input
            .return_paths
            .into_iter()
            .map(validate_return_path)
            .collect::<anyhow::Result<HashSet<_>>>()?;
        let client_secret = std::env::var(&input.client_secret_env).map_err(|_| {
            anyhow::anyhow!(
                "browser client {} secret environment variable {} is missing",
                input.client_id,
                input.client_secret_env
            )
        })?;
        if client_secret.as_bytes().len() < 32 {
            anyhow::bail!(
                "browser client {} secret must contain at least 32 bytes",
                input.client_id
            );
        }
        let client = BrowserClient {
            client_id: input.client_id.clone(),
            supabase_project: input.supabase_project,
            redirect_uris,
            return_paths,
            client_secret,
        };
        if clients.insert(input.client_id.clone(), client).is_some() {
            anyhow::bail!("duplicate browser client_id {}", input.client_id);
        }
    }
    Ok(clients)
}

fn validate_identifier(label: &str, value: &str) -> anyhow::Result<()> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        anyhow::bail!("{label} must be 1-128 URL-safe identifier characters");
    }
    Ok(())
}

fn validate_redirect_uri(value: String) -> anyhow::Result<String> {
    let parsed = Url::parse(&value)?;
    let loopback = parsed
        .host_str()
        .is_some_and(|host| matches!(host, "localhost" | "127.0.0.1" | "::1"));
    if !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.fragment().is_some()
        || parsed.query().is_some()
        || !matches!(parsed.scheme(), "https" | "http")
        || (parsed.scheme() == "http" && !loopback)
    {
        anyhow::bail!("redirect_uri must be an exact HTTPS URL (HTTP only for loopback)");
    }
    Ok(value)
}

fn validate_return_path(value: String) -> anyhow::Result<String> {
    if value.is_empty()
        || value.len() > MAX_AUTH_VALUE_BYTES
        || !value.starts_with('/')
        || value.starts_with("//")
        || value.contains('#')
    {
        anyhow::bail!("return path must be a local absolute path without a fragment");
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::validate_return_path;

    #[test]
    fn return_paths_cannot_escape_origin() {
        assert!(validate_return_path("/u/quote".into()).is_ok());
        assert!(validate_return_path("//evil.example/quote".into()).is_err());
        assert!(validate_return_path("https://evil.example".into()).is_err());
    }
}
