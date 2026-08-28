use std::{
    collections::BTreeSet,
    fs,
    path::{Component, Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

pub const ROUTER_SERVICE_NAME: &str = "gha-executor-router";
pub const MAX_EXECUTORS_DEFAULT: usize = 8;
pub const MAX_REQUEST_BYTES_DEFAULT: usize = 64 * 1024;
pub const MAX_ERROR_CHARS_DEFAULT: usize = 512;
pub const MIN_SECRET_BYTES: usize = 32;
pub const MAX_SECRET_BYTES: usize = 4096;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    Aws,
    Hetzner,
}

impl Provider {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Aws => "aws",
            Self::Hetzner => "hetzner",
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExecutorSpec {
    pub id: String,
    pub provider: Provider,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub auth_path: Option<PathBuf>,
}

#[derive(Clone, Debug)]
pub struct Executor {
    pub id: String,
    pub provider: Provider,
    pub base_url: String,
    pub auth: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidatedBuildRequest {
    pub request_id: String,
    pub repository: String,
    pub revision: String,
    pub profile: String,
}

pub fn parse_executor_specs(raw: &str, max_executors: usize) -> Result<Vec<ExecutorSpec>, String> {
    if max_executors == 0 {
        return Err("maximum executor count must be positive".to_string());
    }
    let specs: Vec<ExecutorSpec> = serde_json::from_str(raw)
        .map_err(|error| format!("GHA_EXECUTOR_ROUTER_EXECUTORS_JSON is invalid: {error}"))?;
    if specs.is_empty() {
        return Err("at least one executor entry is required".to_string());
    }
    if specs.len() > max_executors {
        return Err(format!(
            "executor configuration has {} entries; maximum is {max_executors}",
            specs.len()
        ));
    }

    let mut ids = BTreeSet::new();
    let mut enabled_urls = BTreeSet::new();
    let mut enabled_paths = BTreeSet::new();
    for (index, spec) in specs.iter().enumerate() {
        let label = format!("executors[{index}]");
        validate_executor_id(&spec.id)
            .map_err(|error| format!("{label}.id is invalid: {error}"))?;
        if !ids.insert(spec.id.clone()) {
            return Err(format!("duplicate executor id: {}", spec.id));
        }
        if spec.enabled {
            let url = spec
                .url
                .as_deref()
                .ok_or_else(|| format!("{label}.url is required when enabled=true"))?;
            let normalized = normalize_executor_url(url)
                .map_err(|error| format!("{label}.url is invalid: {error}"))?;
            if !enabled_urls.insert(normalized) {
                return Err(format!("enabled executors must use unique URLs: {url}"));
            }
            let path = spec
                .auth_path
                .as_ref()
                .ok_or_else(|| format!("{label}.authPath is required when enabled=true"))?;
            if !path.is_absolute() {
                return Err(format!("{label}.authPath must be absolute"));
            }
            reject_unsafe_path(path, &format!("{label}.authPath"))?;
            if !enabled_paths.insert(path.clone()) {
                return Err(format!(
                    "enabled executors must use distinct authentication files: {}",
                    path.display()
                ));
            }
        } else if spec.url.is_some() || spec.auth_path.is_some() {
            return Err(format!(
                "{label}: disabled executors must omit url and authPath so dormant endpoints and credentials cannot drift"
            ));
        }
    }
    Ok(specs)
}

pub fn materialize_executors(
    specs: &[ExecutorSpec],
    secret_root: &Path,
) -> Result<Vec<Executor>, String> {
    validate_secret_root(secret_root)?;
    let canonical_root = fs::canonicalize(secret_root).map_err(|error| {
        format!(
            "executor secret root {} is unavailable: {error}",
            secret_root.display()
        )
    })?;
    let mut executors = Vec::new();
    for spec in specs.iter().filter(|spec| spec.enabled) {
        let auth_path = spec
            .auth_path
            .as_ref()
            .expect("enabled executor auth path validated");
        if auth_path.parent() != Some(secret_root) {
            return Err(format!(
                "executor {} authPath must be a direct child of {}",
                spec.id,
                secret_root.display()
            ));
        }
        let canonical_path = fs::canonicalize(auth_path).map_err(|error| {
            format!(
                "executor {} authentication file {} is unavailable: {error}",
                spec.id,
                auth_path.display()
            )
        })?;
        if !canonical_path.starts_with(&canonical_root) {
            return Err(format!(
                "executor {} authentication file escapes the configured secret root",
                spec.id
            ));
        }
        if !canonical_path
            .metadata()
            .map_err(|error| {
                format!(
                    "executor {} authentication metadata is unavailable: {error}",
                    spec.id
                )
            })?
            .is_file()
        {
            return Err(format!(
                "executor {} authentication path is not a regular file",
                spec.id
            ));
        }
        let raw = fs::read_to_string(&canonical_path).map_err(|error| {
            format!(
                "executor {} authentication file could not be read: {error}",
                spec.id
            )
        })?;
        let auth = raw.trim().to_string();
        if auth.len() < MIN_SECRET_BYTES {
            return Err(format!(
                "executor {} authentication secret must contain at least {MIN_SECRET_BYTES} bytes",
                spec.id
            ));
        }
        if auth.len() > MAX_SECRET_BYTES
            || auth.as_bytes().contains(&0)
            || auth.contains('\n')
            || auth.contains('\r')
        {
            return Err(format!(
                "executor {} authentication secret exceeds the bounded secret contract",
                spec.id
            ));
        }
        executors.push(Executor {
            id: spec.id.clone(),
            provider: spec.provider,
            base_url: normalize_executor_url(
                spec.url.as_deref().expect("enabled executor URL validated"),
            )?,
            auth,
        });
    }
    Ok(executors)
}

pub fn validate_secret_root(path: &Path) -> Result<(), String> {
    if !path.is_absolute() {
        return Err("executor secret root must be absolute".to_string());
    }
    reject_unsafe_path(path, "executor secret root")?;
    if path == Path::new("/") {
        return Err("executor secret root must not be the filesystem root".to_string());
    }
    Ok(())
}

fn reject_unsafe_path(path: &Path, label: &str) -> Result<(), String> {
    if path
        .components()
        .any(|component| matches!(component, Component::ParentDir | Component::CurDir))
    {
        return Err(format!("{label} must not contain traversal components"));
    }
    Ok(())
}

pub fn normalize_executor_url(input: &str) -> Result<String, String> {
    let value = input.trim();
    if value.is_empty() || value.len() > 512 {
        return Err("URL must contain between 1 and 512 characters".to_string());
    }
    if value.chars().any(char::is_whitespace) {
        return Err("URL must not contain whitespace".to_string());
    }
    if value.contains('@') {
        return Err("URL must not embed credentials".to_string());
    }
    if value.contains('?') || value.contains('#') {
        return Err("URL must not contain a query string or fragment".to_string());
    }
    let (scheme, rest) = value
        .split_once("://")
        .ok_or_else(|| "URL must use http:// or https://".to_string())?;
    if !matches!(scheme, "http" | "https") {
        return Err("URL must use http:// or https://".to_string());
    }
    let (authority, path) = rest
        .split_once('/')
        .map(|(authority, path)| (authority, format!("/{path}")))
        .unwrap_or((rest, String::new()));
    if authority.is_empty() {
        return Err("URL authority is empty".to_string());
    }
    if !path.is_empty() && path != "/" {
        return Err("URL must be an origin without a path".to_string());
    }
    let host = authority_host(authority)?;
    if scheme == "http"
        && host != "localhost"
        && host != "127.0.0.1"
        && host != "::1"
        && !host.ends_with(".svc.cluster.local")
    {
        return Err(
            "plain HTTP is allowed only for loopback or in-cluster .svc.cluster.local origins"
                .to_string(),
        );
    }
    Ok(format!("{scheme}://{authority}"))
}

fn authority_host(authority: &str) -> Result<String, String> {
    if authority.starts_with('[') {
        let close = authority
            .find(']')
            .ok_or_else(|| "IPv6 authority is missing ']'".to_string())?;
        let host = &authority[1..close];
        let suffix = &authority[close + 1..];
        if !suffix.is_empty() {
            let port = suffix
                .strip_prefix(':')
                .ok_or_else(|| "IPv6 authority suffix must be a port".to_string())?;
            validate_port(port)?;
        }
        if host != "::1" {
            return Err("only IPv6 loopback is accepted in bracket notation".to_string());
        }
        return Ok(host.to_string());
    }
    let mut parts = authority.split(':');
    let host = parts.next().unwrap_or_default().to_ascii_lowercase();
    let port = parts.next();
    if parts.next().is_some() {
        return Err("unbracketed IPv6 authorities are not accepted".to_string());
    }
    if let Some(port) = port {
        validate_port(port)?;
    }
    if host.is_empty()
        || !host
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '.' | '-'))
        || host.starts_with('.')
        || host.ends_with('.')
        || host.contains("..")
    {
        return Err("URL host is invalid".to_string());
    }
    Ok(host)
}

fn validate_port(port: &str) -> Result<(), String> {
    let value = port
        .parse::<u16>()
        .map_err(|_| "URL port must be an integer between 1 and 65535".to_string())?;
    if value == 0 {
        return Err("URL port must be positive".to_string());
    }
    Ok(())
}

pub fn validate_build_request(value: &Value) -> Result<ValidatedBuildRequest, String> {
    let object = value
        .as_object()
        .ok_or_else(|| "build request must be a JSON object".to_string())?;
    let allowed = [
        "schemaVersion",
        "jobKind",
        "repoUrl",
        "gitRef",
        "profile",
        "requestId",
    ]
    .into_iter()
    .collect::<BTreeSet<_>>();
    let unknown = object
        .keys()
        .filter(|key| !allowed.contains(key.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    if !unknown.is_empty() {
        return Err(format!(
            "executor router accepts fixed run-profile requests only; unsupported fields: {}",
            unknown.join(", ")
        ));
    }
    if object.get("schemaVersion").and_then(Value::as_str) != Some("build-server.v1") {
        return Err("schemaVersion must be build-server.v1".to_string());
    }
    if object.get("jobKind").and_then(Value::as_str) != Some("run-profile") {
        return Err("jobKind must be run-profile".to_string());
    }
    let repo_url = required_string(object.get("repoUrl"), "repoUrl")?;
    let repository = validate_repo_url(repo_url)?;
    let revision = required_string(object.get("gitRef"), "gitRef")?;
    if !is_full_commit_sha(revision) {
        return Err("gitRef must be a lowercase 40-hex commit SHA".to_string());
    }
    let profile = required_string(object.get("profile"), "profile")?;
    if !valid_profile(profile) {
        return Err("profile must use lowercase letters, digits, and hyphens".to_string());
    }
    let request_id = required_string(object.get("requestId"), "requestId")?;
    if !valid_request_id(request_id) {
        return Err(
            "requestId must be 1-256 characters using letters, digits, '.', ':', '_', or '-'"
                .to_string(),
        );
    }
    Ok(ValidatedBuildRequest {
        request_id: request_id.to_string(),
        repository,
        revision: revision.to_string(),
        profile: profile.to_string(),
    })
}

fn required_string<'a>(value: Option<&'a Value>, label: &str) -> Result<&'a str, String> {
    value
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("{label} must be a non-empty string"))
}

fn validate_repo_url(value: &str) -> Result<String, String> {
    if value.contains('@')
        || value.contains('?')
        || value.contains('#')
        || value.contains(char::is_whitespace)
    {
        return Err(
            "repoUrl must not contain credentials, query, fragment, or whitespace".to_string(),
        );
    }
    let path = value
        .strip_prefix("https://github.com/")
        .and_then(|path| path.strip_suffix(".git"))
        .ok_or_else(|| "repoUrl must be https://github.com/<owner>/<repo>.git".to_string())?;
    let mut parts = path.split('/');
    let owner = parts.next().unwrap_or_default();
    let repo = parts.next().unwrap_or_default();
    if parts.next().is_some() || !valid_repo_component(owner) || !valid_repo_component(repo) {
        return Err("repoUrl owner/repository identity is invalid".to_string());
    }
    Ok(format!("{owner}/{repo}"))
}

fn valid_repo_component(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 100
        && value != "."
        && value != ".."
        && value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-')
        })
}

pub fn is_full_commit_sha(value: &str) -> bool {
    value.len() == 40
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn valid_profile(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value.bytes().enumerate().all(|(index, byte)| {
            byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || (byte == b'-' && index > 0 && index + 1 < value.len())
        })
}

fn valid_request_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b':' | b'_' | b'-'))
}

pub fn validate_executor_id(value: &str) -> Result<(), String> {
    if value.is_empty() || value.len() > 32 {
        return Err("executor id must contain between 1 and 32 characters".to_string());
    }
    let bytes = value.as_bytes();
    if !bytes[0].is_ascii_lowercase() && !bytes[0].is_ascii_digit() {
        return Err("executor id must start with a lowercase letter or digit".to_string());
    }
    if !bytes[bytes.len() - 1].is_ascii_lowercase() && !bytes[bytes.len() - 1].is_ascii_digit() {
        return Err("executor id must end with a lowercase letter or digit".to_string());
    }
    if !bytes
        .iter()
        .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-')
    {
        return Err("executor id must use lowercase letters, digits, and hyphens".to_string());
    }
    Ok(())
}

pub fn namespace_build_id(executor_id: &str, upstream_id: &str) -> Result<String, String> {
    validate_executor_id(executor_id)?;
    if !valid_upstream_build_id(upstream_id) {
        return Err(
            "upstream build id must be 1-128 characters using letters, digits, '.', '_', or '-'"
                .to_string(),
        );
    }
    Ok(format!("{executor_id}~{upstream_id}"))
}

pub fn parse_namespaced_build_id(value: &str) -> Result<(&str, &str), String> {
    let (executor_id, upstream_id) = value
        .split_once('~')
        .ok_or_else(|| "build id is missing the executor namespace".to_string())?;
    if upstream_id.contains('~') {
        return Err("build id contains more than one executor namespace".to_string());
    }
    validate_executor_id(executor_id)?;
    if !valid_upstream_build_id(upstream_id) {
        return Err("upstream build id is invalid".to_string());
    }
    Ok((executor_id, upstream_id))
}

fn valid_upstream_build_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

pub fn digest_eq(left: &str, right: &str) -> bool {
    let left = Sha256::digest(left.as_bytes());
    let right = Sha256::digest(right.as_bytes());
    left.as_slice().ct_eq(right.as_slice()).into()
}

pub fn bounded_text(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn unique_temp_root() -> PathBuf {
        std::env::temp_dir().join(format!(
            "gha-executor-router-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ))
    }

    #[test]
    fn parses_ordered_enabled_and_disabled_executor_contracts() {
        let specs = parse_executor_specs(
            r#"[
              {"id":"aws-primary","provider":"aws","enabled":true,"url":"http://dd-build-server.default.svc.cluster.local:8100","authPath":"/var/run/secrets/router/aws-auth"},
              {"id":"hetzner-secondary","provider":"hetzner","enabled":false}
            ]"#,
            8,
        )
        .unwrap();
        assert_eq!(specs.len(), 2);
        assert_eq!(specs[0].provider, Provider::Aws);
        assert!(specs[0].enabled);
        assert_eq!(specs[1].provider, Provider::Hetzner);
        assert!(!specs[1].enabled);
    }

    #[test]
    fn rejects_duplicate_ids_urls_and_secret_paths() {
        for raw in [
            r#"[
              {"id":"aws","provider":"aws","enabled":true,"url":"https://one.example","authPath":"/secrets/a"},
              {"id":"aws","provider":"hetzner","enabled":true,"url":"https://two.example","authPath":"/secrets/b"}
            ]"#,
            r#"[
              {"id":"aws","provider":"aws","enabled":true,"url":"https://one.example","authPath":"/secrets/a"},
              {"id":"hetzner","provider":"hetzner","enabled":true,"url":"https://one.example/","authPath":"/secrets/b"}
            ]"#,
            r#"[
              {"id":"aws","provider":"aws","enabled":true,"url":"https://one.example","authPath":"/secrets/a"},
              {"id":"hetzner","provider":"hetzner","enabled":true,"url":"https://two.example","authPath":"/secrets/a"}
            ]"#,
        ] {
            assert!(parse_executor_specs(raw, 8).is_err(), "accepted {raw}");
        }
    }

    #[test]
    fn disabled_entries_cannot_hide_dormant_endpoint_or_auth_state() {
        assert!(parse_executor_specs(
            r#"[{"id":"aws","provider":"aws","enabled":false,"url":"https://one.example"}]"#,
            8,
        )
        .is_err());
        assert!(parse_executor_specs(
            r#"[{"id":"aws","provider":"aws","enabled":false,"authPath":"/secrets/a"}]"#,
            8,
        )
        .is_err());
    }

    #[test]
    fn url_policy_allows_cluster_loopback_and_https_origins_only() {
        for value in [
            "http://localhost:8100",
            "http://127.0.0.1:8100",
            "http://[::1]:8100",
            "http://dd-build-server.default.svc.cluster.local:8100",
            "https://ci.example.com",
        ] {
            assert!(normalize_executor_url(value).is_ok(), "rejected {value}");
        }
        for value in [
            "http://ci.example.com",
            "https://user:pass@ci.example.com",
            "https://ci.example.com/path",
            "https://ci.example.com?token=x",
            "https://ci.example.com#fragment",
            "file:///tmp/socket",
            "https://ci..example.com",
        ] {
            assert!(normalize_executor_url(value).is_err(), "accepted {value}");
        }
    }

    #[test]
    fn materializes_only_direct_bounded_secret_files() {
        let root = unique_temp_root();
        fs::create_dir_all(&root).unwrap();
        let secret = root.join("aws-auth");
        fs::write(&secret, "a".repeat(MIN_SECRET_BYTES)).unwrap();
        let specs = vec![ExecutorSpec {
            id: "aws-primary".into(),
            provider: Provider::Aws,
            enabled: true,
            url: Some("http://127.0.0.1:8100".into()),
            auth_path: Some(secret.clone()),
        }];
        let executors = materialize_executors(&specs, &root).unwrap();
        assert_eq!(executors.len(), 1);
        assert_eq!(executors[0].auth, "a".repeat(MIN_SECRET_BYTES));

        fs::write(&secret, format!("{}\n{}", "a".repeat(16), "b".repeat(16))).unwrap();
        assert!(materialize_executors(&specs, &root).is_err());
        fs::write(&secret, "a".repeat(MIN_SECRET_BYTES)).unwrap();

        let nested = root.join("nested");
        fs::create_dir_all(&nested).unwrap();
        let nested_secret = nested.join("auth");
        fs::write(&nested_secret, "b".repeat(MIN_SECRET_BYTES)).unwrap();
        let mut nested_specs = specs;
        nested_specs[0].auth_path = Some(nested_secret);
        assert!(materialize_executors(&nested_specs, &root).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn validates_only_fixed_profile_immutable_build_requests() {
        let value = json!({
            "schemaVersion": "build-server.v1",
            "jobKind": "run-profile",
            "repoUrl": "https://github.com/ORESoftware/k8s-cluster.git",
            "gitRef": "a".repeat(40),
            "profile": "rust-verify",
            "requestId": "gha-clone:plan-1:rust"
        });
        let validated = validate_build_request(&value).unwrap();
        assert_eq!(validated.repository, "ORESoftware/k8s-cluster");
        assert_eq!(validated.profile, "rust-verify");

        for field in ["image", "deploy", "buildArgs", "dockerfile", "executor"] {
            let mut invalid = value.clone();
            invalid
                .as_object_mut()
                .unwrap()
                .insert(field.to_string(), json!("caller-controlled"));
            assert!(
                validate_build_request(&invalid).is_err(),
                "accepted {field}"
            );
        }
    }

    #[test]
    fn rejects_mutable_refs_and_secret_shaped_repo_urls() {
        let base = json!({
            "schemaVersion": "build-server.v1",
            "jobKind": "run-profile",
            "repoUrl": "https://github.com/owner/repo.git",
            "gitRef": "a".repeat(40),
            "profile": "rust-verify",
            "requestId": "request-1"
        });
        for revision in ["main", "v1.2.3", &"A".repeat(40)] {
            let mut value = base.clone();
            value["gitRef"] = json!(revision);
            assert!(validate_build_request(&value).is_err());
        }
        let mut value = base;
        value["repoUrl"] = json!("https://token@github.com/owner/repo.git");
        assert!(validate_build_request(&value).is_err());
    }

    #[test]
    fn build_ids_are_executor_namespaced_and_unambiguous() {
        let value =
            namespace_build_id("aws-primary", "550e8400-e29b-41d4-a716-446655440000").unwrap();
        assert_eq!(value, "aws-primary~550e8400-e29b-41d4-a716-446655440000");
        assert_eq!(
            parse_namespaced_build_id(&value).unwrap(),
            ("aws-primary", "550e8400-e29b-41d4-a716-446655440000")
        );
        assert!(parse_namespaced_build_id("550e8400-e29b").is_err());
        assert!(parse_namespaced_build_id("aws~bad~id").is_err());
        assert!(namespace_build_id("AWS", "id").is_err());
    }

    #[test]
    fn authentication_comparison_is_content_exact() {
        assert!(digest_eq("same", "same"));
        assert!(!digest_eq("same", "different"));
        assert!(!digest_eq("same", "same\0"));
    }
}
