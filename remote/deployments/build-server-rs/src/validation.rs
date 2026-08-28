use std::{
    collections::BTreeMap,
    path::{Component, Path, PathBuf},
};

use crate::config::{profile_policy, Config};
use crate::ecr::{ecr_image, EcrImage};
use crate::profiles;
use crate::types::{BuildRequest, DeployRequest};

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
    // Fail closed: an empty allowlist denies everything rather than allowing
    // any repo/image. A dropped or typo'd env var must never silently reopen
    // the server to arbitrary clone/build targets.
    if prefixes.is_empty() {
        return Err(format!(
            "{name} is rejected because {env_name} is empty; configure an explicit allowlist"
        ));
    }
    if prefixes.iter().any(|prefix| value.starts_with(prefix)) {
        Ok(())
    } else {
        Err(format!("{name} is not allowed by {env_name}"))
    }
}

pub(crate) fn validate_no_whitespace(
    name: &str,
    value: &str,
    max_len: usize,
) -> Result<(), String> {
    if value.trim().is_empty() {
        return Err(format!("{name} must not be empty"));
    }
    if value.len() > max_len {
        return Err(format!("{name} must be {max_len} characters or fewer"));
    }
    if value.chars().any(char::is_whitespace) {
        return Err(format!("{name} must not contain whitespace"));
    }
    if value.chars().any(char::is_control) {
        return Err(format!("{name} must not contain control characters"));
    }
    Ok(())
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

pub(crate) fn has_explicit_image_version(image: &str) -> bool {
    let last_path = image.rsplit('/').next().unwrap_or(image);
    image.contains('@') || last_path.contains(':')
}

pub(crate) fn validate_image(
    config: &Config,
    image: &str,
    push: bool,
) -> Result<Option<EcrImage>, String> {
    validate_no_whitespace("image", image, 512)?;
    // A leading dash would be parsed by nerdctl as a flag in the `-t <image>`
    // and `push <image>` positions; reject it before it reaches argv.
    if image.trim_start().starts_with('-') {
        return Err("image must not start with '-'".to_string());
    }
    if !has_explicit_image_version(image) {
        return Err("image must include an explicit tag or digest".to_string());
    }
    ensure_allowed_prefix(
        "image",
        image,
        &config.allowed_image_prefixes,
        "BUILD_SERVER_ALLOWED_IMAGE_PREFIXES",
    )?;
    let ecr = ecr_image(image);
    if push && config.ecr_login_enabled && ecr.is_none() {
        return Err(
            "push currently requires an Amazon ECR image when ECR login is enabled".to_string(),
        );
    }
    Ok(ecr)
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
            Component::Normal(value) => {
                let part = value
                    .to_str()
                    .ok_or_else(|| format!("{name} must be valid UTF-8"))?;
                // Reject characters that are structural in a nerdctl `--mount`
                // spec (`,` `=` `:`) or otherwise unsafe, so a path component
                // can never inject an extra mount field (e.g. a second `src=`).
                if part
                    .chars()
                    .any(|ch| matches!(ch, ',' | '=' | ':' | '\0') || ch.is_control())
                {
                    return Err(format!("{name} contains unsupported characters"));
                }
                clean.push(value);
            }
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

pub(crate) fn validate_build_args(
    build_args: &Option<BTreeMap<String, String>>,
) -> Result<(), String> {
    let Some(build_args) = build_args else {
        return Ok(());
    };
    if build_args.len() > 32 {
        return Err("buildArgs can contain at most 32 entries".to_string());
    }
    for (key, value) in build_args {
        if key.is_empty() || key.len() > 80 {
            return Err("build arg keys must be 1-80 characters".to_string());
        }
        let upper_key = key.to_ascii_uppercase();
        if ["SECRET", "PASSWORD", "TOKEN", "CREDENTIAL", "PRIVATE_KEY"]
            .iter()
            .any(|part| upper_key.contains(part))
        {
            return Err(format!(
                "build arg key {key:?} looks secret-like; use registry/repo credentials, not Docker build args"
            ));
        }
        if !key
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.'))
        {
            return Err(format!(
                "build arg key {key:?} contains unsupported characters"
            ));
        }
        if value.len() > 1024 || value.chars().any(char::is_control) {
            return Err(format!(
                "build arg {key:?} must be printable and 1024 characters or fewer"
            ));
        }
    }
    Ok(())
}

pub(crate) fn validate_namespace(config: &Config, namespace: &str) -> Result<(), String> {
    validate_no_whitespace("deploy.namespace", namespace, 63)?;
    if !config.allowed_namespaces.contains(namespace) {
        return Err(format!(
            "namespace {namespace:?} is not allowed by BUILD_SERVER_ALLOWED_NAMESPACES"
        ));
    }
    Ok(())
}

pub(crate) fn validate_rollout_resource(value: &str) -> Result<String, String> {
    let value = value.trim();
    validate_no_whitespace("deploy.rollout", value, 160)?;
    if value.contains("..") {
        return Err("deploy.rollout must not contain '..'".to_string());
    }
    // A leading dash would be parsed by kubectl as a flag (--kubeconfig=…,
    // --server=…) rather than a positional resource. Reject before shaping.
    if value.starts_with('-') {
        return Err("deploy.rollout must not start with '-'".to_string());
    }
    let resource = if value.contains('/') {
        value.to_string()
    } else {
        format!("deployment/{value}")
    };
    // Accept only a single `TYPE/NAME` positional with conservative charsets.
    // Flags, hosts, kubeconfig paths, and extra slashes must never reach the
    // kubectl argv here.
    let (kind, name) = resource
        .split_once('/')
        .ok_or_else(|| "deploy.rollout must be TYPE/NAME".to_string())?;
    let kind_ok = !kind.is_empty()
        && kind
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || matches!(ch, '.' | '-'));
    let name_ok = !name.is_empty()
        && name.len() <= 253
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-'));
    if !kind_ok || !name_ok {
        return Err("deploy.rollout must be a valid TYPE/NAME resource".to_string());
    }
    Ok(resource)
}

pub(crate) fn validate_deploy(
    config: &Config,
    deploy: &Option<DeployRequest>,
) -> Result<(), String> {
    let Some(deploy) = deploy else {
        return Ok(());
    };
    match deploy.kind.as_str() {
        "kustomize" | "manifest" | "none" => {}
        _ => return Err("deploy.kind must be one of: kustomize, manifest, none".to_string()),
    }
    if deploy.kind == "none" {
        return Ok(());
    }
    if !config.deploy_enabled {
        return Err("deploy is disabled by BUILD_SERVER_DEPLOY_ENABLED=false".to_string());
    }
    validate_relative_path("deploy.path", &deploy.path)?;
    let namespace = deploy.namespace.as_deref().unwrap_or("default");
    validate_namespace(config, namespace)?;
    if let Some(rollout) = deploy.rollout.as_deref() {
        validate_rollout_resource(rollout)?;
    }
    Ok(())
}

pub(crate) fn validate_build_request(
    config: &Config,
    request: &BuildRequest,
) -> Result<(), String> {
    if let Some(schema_version) = clean_optional(request.schema_version.as_deref()) {
        if schema_version != "build-server.v1" {
            return Err("schemaVersion must be build-server.v1".to_string());
        }
    }
    let job_kind =
        clean_optional(request.job_kind.as_deref()).unwrap_or_else(|| "build-image".to_string());
    if !matches!(
        job_kind.as_str(),
        "build-image" | "build-and-deploy" | "run-profile"
    ) {
        return Err("jobKind must be build-image, build-and-deploy, or run-profile".to_string());
    }
    validate_repo_url(&request.repo_url)?;
    ensure_allowed_prefix(
        "repoUrl",
        &request.repo_url,
        &config.allowed_repo_prefixes,
        "BUILD_SERVER_ALLOWED_REPO_PREFIXES",
    )?;
    if job_kind == "run-profile" {
        let profile = clean_optional(request.profile.as_deref())
            .ok_or_else(|| "profile is required for jobKind=run-profile".to_string())?;
        if profiles::find(&profile).is_none() || !config.allowed_profiles.contains(&profile) {
            return Err(format!(
                "profile {profile:?} is not allowed by BUILD_SERVER_ALLOWED_PROFILES"
            ));
        }
        profile_policy::ensure_repository_profile_allowed(
            &request.repo_url,
            &profile,
            &config.allowed_profile_repo_prefixes,
        )?;
        if !request.image.trim().is_empty() {
            return Err("image must be omitted for jobKind=run-profile".to_string());
        }
        if request.push.unwrap_or(false)
            || request.deploy.is_some()
            || request.build_args.is_some()
            || request.dockerfile.is_some()
        {
            return Err(
                "run-profile does not accept image, push, deploy, buildArgs, or dockerfile"
                    .to_string(),
            );
        }
    } else {
        if request.profile.is_some() {
            return Err("profile is only valid for jobKind=run-profile".to_string());
        }
        validate_image(config, &request.image, request.push.unwrap_or(false))?;
    }
    if let Some(git_ref) = clean_optional(request.git_ref.as_deref()) {
        validate_no_whitespace("gitRef", &git_ref, 180)?;
    }
    validate_relative_path("contextDir", request.context_dir.as_deref().unwrap_or("."))?;
    if job_kind != "run-profile" {
        validate_relative_path(
            "dockerfile",
            request.dockerfile.as_deref().unwrap_or("Dockerfile"),
        )?;
        validate_build_args(&request.build_args)?;
    }
    if request.push.unwrap_or(false) && !config.push_enabled {
        return Err("push is disabled by BUILD_SERVER_PUSH_ENABLED=false".to_string());
    }
    match request.executor.as_deref() {
        None | Some("local") => {}
        Some("lambda") => {
            if job_kind == "run-profile" {
                return Err("run-profile currently requires executor=local".to_string());
            }
            if !config.lambda_executor_enabled {
                return Err(
                    "executor \"lambda\" is disabled by BUILD_SERVER_LAMBDA_ENABLED=false"
                        .to_string(),
                );
            }
        }
        Some(other) => return Err(format!("executor {other:?} must be local or lambda")),
    }
    if let Some(request_id) = clean_optional(request.request_id.as_deref()) {
        validate_no_whitespace("requestId", &request_id, 128)?;
    }
    if job_kind == "run-profile" {
        Ok(())
    } else {
        validate_deploy(config, &request.deploy)
    }
}

pub(crate) fn request_job_kind(request: &BuildRequest) -> String {
    clean_optional(request.job_kind.as_deref()).unwrap_or_else(|| "build-image".to_string())
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn repository_and_path_validation_blocks_command_and_path_injection() {
        assert!(validate_repo_url("https://github.com/ORESoftware/example.git").is_ok());
        assert!(validate_repo_url("file:///etc/passwd").is_err());
        assert!(validate_repo_url("https://github.com/example.git\n--upload-pack=evil").is_err());
        assert!(validate_relative_path("contextDir", "services/api").is_ok());
        assert!(validate_relative_path("contextDir", "../../etc").is_err());
        assert!(validate_relative_path("contextDir", "/etc").is_err());
        // Mount-spec injection: a component with `,`/`=`/`:` could add a second
        // `src=` field to the nerdctl --mount string.
        assert!(validate_relative_path("contextDir", "x,src=/home/ec2-user").is_err());
        assert!(validate_relative_path("contextDir", "a=b").is_err());
        assert!(validate_relative_path("contextDir", "c:d").is_err());

        // Rollout resource must be a clean TYPE/NAME positional, never a flag.
        assert_eq!(validate_rollout_resource("api").unwrap(), "deployment/api");
        assert_eq!(
            validate_rollout_resource("deployment.apps/api").unwrap(),
            "deployment.apps/api"
        );
        assert!(validate_rollout_resource("--kubeconfig=/tmp/x/y").is_err());
        assert!(validate_rollout_resource("--server=http://evil/").is_err());
        assert!(validate_rollout_resource("deployment/a/b").is_err());
    }

    #[test]
    fn build_args_reject_secret_like_keys() {
        let safe = Some(BTreeMap::from([(
            "BUILD_PROFILE".to_string(),
            "release".to_string(),
        )]));
        let unsafe_args = Some(BTreeMap::from([(
            "GITHUB_TOKEN".to_string(),
            "do-not-pass-secrets-as-build-args".to_string(),
        )]));
        assert!(validate_build_args(&safe).is_ok());
        assert!(validate_build_args(&unsafe_args).is_err());
    }

    #[test]
    fn fixed_profiles_exist_and_do_not_accept_commands_from_callers() {
        let names = profiles::names().collect::<HashSet<_>>();
        for expected in [
            "flutter-android-debug",
            "flutter-web-release",
            "flutter-linux-release",
            "flutter-web-e2e",
            "playwright",
            "puppeteer",
        ] {
            assert!(names.contains(expected));
        }
        assert!(profiles::find("sh -c evil").is_none());
    }
}
