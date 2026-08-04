from pathlib import Path


def replace_once(text: str, old: str, new: str, label: str) -> str:
    count = text.count(old)
    if count != 1:
        raise RuntimeError(f"{label}: expected one match, found {count}")
    return text.replace(old, new, 1)


def append_once(text: str, marker: str, addition: str, label: str) -> str:
    if addition in text:
        raise RuntimeError(f"{label}: addition already present")
    if marker not in text:
        raise RuntimeError(f"{label}: marker missing")
    return text.replace(marker, marker + addition, 1)


# Expose the reloadable token source from the library crate.
path = Path("remote/deployments/gha-clone-server-rs/src/lib.rs")
text = path.read_text(encoding="utf-8")
text = replace_once(text, "use std::collections", "pub mod credentials;\n\nuse std::collections", "library module")
path.write_text(text, encoding="utf-8")

# Read the GitHub token on every workflow fetch, so projected-secret updates are
# visible without a pod restart.
path = Path("remote/deployments/gha-clone-server-rs/src/main.rs")
text = path.read_text(encoding="utf-8")
text = replace_once(
    text,
    "use gha_clone_server::{\n    build_plan, capabilities, is_full_commit_sha, verify_github_signature, PlanRequest,\n    PlannerLimits, WorkflowPlan, SERVICE_NAME,\n};",
    "use gha_clone_server::{\n    build_plan, capabilities, credentials::TokenSource, is_full_commit_sha,\n    verify_github_signature, PlanRequest, PlannerLimits, WorkflowPlan, SERVICE_NAME,\n};",
    "server imports",
)
text = replace_once(
    text,
    "    github_token: Option<String>,",
    "    github_token_source: Option<TokenSource>,",
    "server config token field",
)
text = replace_once(
    text,
    '            github_token: env_optional("GHA_CLONE_GITHUB_TOKEN"),',
    '            github_token_source: TokenSource::from_env(\n                "GHA_CLONE_GITHUB_TOKEN",\n                "GHA_CLONE_GITHUB_TOKEN_FILE",\n            )?,',
    "server token configuration",
)
text = replace_once(
    text,
    "    fn execution_ready(&self) -> bool {\n        !self.execution_enabled\n            || (self.auth_secret.is_some()\n                && self.build_server_url.is_some()\n                && self.build_server_auth.is_some()\n                && !self.allowed_repositories.is_empty())\n    }",
    "    fn execution_ready(&self) -> bool {\n        let independent_ready = !self.execution_enabled\n            || (self.auth_secret.is_some()\n                && self.build_server_url.is_some()\n                && self.build_server_auth.is_some()\n                && !self.allowed_repositories.is_empty());\n        let webhook_ready = !self.webhook_execution_enabled\n            || (self.execution_enabled\n                && self.webhook_secret.is_some()\n                && self\n                    .github_token_source\n                    .as_ref()\n                    .is_some_and(|source| source.read().is_ok())\n                && !self.workflow_rules.is_empty());\n        independent_ready && webhook_ready\n    }",
    "server readiness",
)
text = replace_once(
    text,
    '        "githubApiConfigured": state.config.github_token.is_some(),',
    '        "githubApiConfigured": state.config.github_token_source.is_some(),\n        "githubCredentialSource": state\n            .config\n            .github_token_source\n            .as_ref()\n            .map(TokenSource::kind)\n            .unwrap_or("none"),',
    "server health credential state",
)
text = replace_once(
    text,
    "    if let Some(token) = state.config.github_token.as_deref() {\n        request = request.bearer_auth(token);\n    }",
    "    if let Some(source) = state.config.github_token_source.as_ref() {\n        request = request.bearer_auth(source.read()?);\n    }",
    "workflow token reload",
)
path.write_text(text, encoding="utf-8")

for relative in [
    "remote/deployments/gha-clone-server-rs/tests/meta_self_test.rs",
    "remote/deployments/gha-clone-server-rs/tests/msgint_operator_config.rs",
]:
    path = Path(relative)
    text = path.read_text(encoding="utf-8")
    text = replace_once(
        text,
        '        .env_remove("GHA_CLONE_GITHUB_TOKEN")',
        '        .env_remove("GHA_CLONE_GITHUB_TOKEN")\n        .env_remove("GHA_CLONE_GITHUB_TOKEN_FILE")',
        f"{relative} token environment isolation",
    )
    path.write_text(text, encoding="utf-8")

# Build-server profile clones reload the token file for every git process.
path = Path("remote/deployments/build-server-rs/src/config.rs")
text = path.read_text(encoding="utf-8")
text = replace_once(
    text,
    "use std::{collections::HashSet, env, path::PathBuf, time::Duration};",
    "use std::{\n    collections::HashSet,\n    env, fmt, fs,\n    path::{Component, Path, PathBuf},\n    time::Duration,\n};",
    "build config imports",
)
credential_code = r'''
const MAX_GIT_TOKEN_BYTES: usize = 4096;
const MIN_GIT_TOKEN_BYTES: usize = 20;

#[derive(Clone)]
pub(crate) enum GitCredentialSource {
    Inline(String),
    File(PathBuf),
}

impl fmt::Debug for GitCredentialSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Inline(_) => formatter.write_str("Inline(<redacted>)"),
            Self::File(path) => formatter.debug_tuple("File").field(path).finish(),
        }
    }
}

impl GitCredentialSource {
    fn from_environment() -> Option<Self> {
        if let Some(path) = first_env(&["BUILD_SERVER_GIT_TOKEN_FILE"]) {
            return Some(Self::File(PathBuf::from(path)));
        }
        first_env(&["BUILD_SERVER_GIT_TOKEN", "GH_PAT"]).map(Self::Inline)
    }

    fn token(&self) -> Result<String, String> {
        let token = match self {
            Self::Inline(token) => token.clone(),
            Self::File(path) => {
                validate_git_token_path(path)?;
                let metadata = fs::metadata(path)
                    .map_err(|_| "build-server GitHub token file is unavailable".to_string())?;
                if !metadata.is_file() {
                    return Err("build-server GitHub token path is not a regular file".to_string());
                }
                if metadata.len() as usize > MAX_GIT_TOKEN_BYTES {
                    return Err("build-server GitHub token file exceeds the byte limit".to_string());
                }
                fs::read_to_string(path)
                    .map_err(|_| "build-server GitHub token file could not be read".to_string())?
                    .trim()
                    .to_string()
            }
        };
        validate_git_token(&token)?;
        Ok(token)
    }

    fn authorization_header(&self) -> Result<String, String> {
        let token = self.token()?;
        Ok(format!(
            "AUTHORIZATION: basic {}",
            BASE64.encode(format!("x-access-token:{token}"))
        ))
    }
}

fn validate_git_token_path(path: &Path) -> Result<(), String> {
    if !path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(
            "BUILD_SERVER_GIT_TOKEN_FILE must be an absolute path without '..'".to_string(),
        );
    }
    Ok(())
}

fn validate_git_token(token: &str) -> Result<(), String> {
    if token.len() < MIN_GIT_TOKEN_BYTES || token.len() > MAX_GIT_TOKEN_BYTES {
        return Err(format!(
            "build-server GitHub token must contain between {MIN_GIT_TOKEN_BYTES} and {MAX_GIT_TOKEN_BYTES} bytes"
        ));
    }
    if token.chars().any(char::is_whitespace) || token.chars().any(char::is_control) {
        return Err("build-server GitHub token must be a single printable token".to_string());
    }
    Ok(())
}

'''
text = replace_once(
    text,
    "use crate::{fiducia, gh_secrets, profiles, webhooks};\n\n",
    "use crate::{fiducia, gh_secrets, profiles, webhooks};\n\n" + credential_code,
    "build credential helper",
)
text = replace_once(
    text,
    "    /// Precomputed Basic authorization header for trusted private GitHub clones.\n    /// Never serialized or written to command logs.\n    pub(crate) git_http_auth_header: Option<String>,",
    "    /// Reloadable credential for trusted private GitHub clones. The file\n    /// source is read for every git process so projected Secret rotation is live.\n    pub(crate) git_credential_source: Option<GitCredentialSource>,",
    "build credential field",
)
config_impl = r'''

impl Config {
    pub(crate) fn git_http_auth_header(&self) -> Result<Option<String>, String> {
        self.git_credential_source
            .as_ref()
            .map(GitCredentialSource::authorization_header)
            .transpose()
    }

    pub(crate) fn git_credentials_ready(&self) -> bool {
        self.git_http_auth_header().is_ok()
    }
}
'''
text = replace_once(
    text,
    "}\n\npub(crate) fn first_env",
    "}" + config_impl + "\npub(crate) fn first_env",
    "build config credential methods",
)
text = replace_once(
    text,
    "    let github_token = first_env(&[\"BUILD_SERVER_GIT_TOKEN\", \"GH_PAT\"]);\n    let git_http_auth_header = github_token.as_deref().map(|token| {\n        format!(\n            \"AUTHORIZATION: basic {}\",\n            BASE64.encode(format!(\"x-access-token:{token}\"))\n        )\n    });",
    "    // A mounted token file has precedence over legacy environment tokens.\n    // The file is re-read for each clone, allowing short-lived installation\n    // tokens to rotate without restarting the build-server pod.\n    let git_credential_source = GitCredentialSource::from_environment();",
    "build credential configuration",
)
text = replace_once(
    text,
    "        git_http_auth_header,",
    "        git_credential_source,",
    "build credential assignment",
)
config_tests = r'''

#[cfg(test)]
mod git_credential_tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temporary_path() -> PathBuf {
        std::env::temp_dir().join(format!(
            "dd-build-server-git-token-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ))
    }

    #[test]
    fn token_file_is_reloaded_and_debug_output_is_redacted() {
        let path = temporary_path();
        fs::write(&path, "ghs_first_build_token_123456789\n").expect("write first token");
        let source = GitCredentialSource::File(path.clone());
        let first = source.authorization_header().expect("first header");
        fs::write(&path, "ghs_second_build_token_987654321\n").expect("write rotated token");
        let second = source.authorization_header().expect("second header");
        assert_ne!(first, second);
        assert_eq!(format!("{:?}", GitCredentialSource::Inline("ghs_secret_token_123456789".into())), "Inline(<redacted>)");
        let _ = fs::remove_file(path);
    }

    #[test]
    fn token_files_must_be_absolute_and_tokens_single_line() {
        assert!(validate_git_token_path(Path::new("relative/token")).is_err());
        assert!(validate_git_token("ghs_token_with whitespace_123456").is_err());
        assert!(validate_git_token("short").is_err());
    }
}
'''
text += config_tests
path.write_text(text, encoding="utf-8")

path = Path("remote/deployments/build-server-rs/src/exec.rs")
text = path.read_text(encoding="utf-8")
text = replace_once(
    text,
    "    if program == config.git_bin {\n        if let Some(auth_header) = config.git_http_auth_header.as_deref() {\n            command\n                .env(\"GIT_CONFIG_COUNT\", \"1\")\n                .env(\"GIT_CONFIG_KEY_0\", \"http.https://github.com/.extraheader\")\n                .env(\"GIT_CONFIG_VALUE_0\", auth_header);\n        }\n    }",
    "    if program == config.git_bin {\n        if let Some(auth_header) = config.git_http_auth_header()? {\n            command\n                .env(\"GIT_CONFIG_COUNT\", \"1\")\n                .env(\"GIT_CONFIG_KEY_0\", \"http.https://github.com/.extraheader\")\n                .env(\"GIT_CONFIG_VALUE_0\", auth_header);\n        }\n    }",
    "git command credential reload",
)
text = replace_once(
    text,
    "    config.server_auth_secret.is_some()\n        && config.work_root.exists()",
    "    config.server_auth_secret.is_some()\n        && config.git_credentials_ready()\n        && config.work_root.exists()",
    "build readiness credential check",
)
path.write_text(text, encoding="utf-8")

# Kubernetes projected Secret files update in place; avoid static token env vars.
path = Path("remote/argocd/dd-next-runtime/dd-gha-clone-server.deployment.yaml")
text = path.read_text(encoding="utf-8")
text = replace_once(
    text,
    "            - name: GHA_CLONE_GITHUB_TOKEN\n              valueFrom:\n                secretKeyRef:\n                  name: dd-gha-clone-server-secrets\n                  key: github_app_installation_token",
    "            - name: GHA_CLONE_GITHUB_TOKEN_FILE\n              value: /var/run/secrets/gha-clone-github/github_app_installation_token",
    "gha clone token file env",
)
text = replace_once(
    text,
    "          volumeMounts:\n            - name: tmp",
    "          volumeMounts:\n            - name: github-token\n              mountPath: /var/run/secrets/gha-clone-github\n              readOnly: true\n            - name: tmp",
    "gha clone token volume mount",
)
text = replace_once(
    text,
    "      volumes:\n        - name: tmp",
    "      volumes:\n        - name: github-token\n          secret:\n            secretName: dd-gha-clone-server-secrets\n            defaultMode: 256\n            items:\n              - key: github_app_installation_token\n                path: github_app_installation_token\n        - name: tmp",
    "gha clone token volume",
)
path.write_text(text, encoding="utf-8")

path = Path("remote/argocd/dd-next-runtime/dd-build-server-gha-continuity.patch.yaml")
path.write_text(
    """apiVersion: apps/v1
kind: Deployment
metadata:
  name: dd-build-server
  namespace: default
spec:
  template:
    spec:
      containers:
        - name: build-server
          env:
            - name: BUILD_SERVER_ALLOWED_PROFILES
              value: rust-verify,node-verify,node-hardened-verify,python-verify,flutter-verify,flutter-android-debug,flutter-web-release,flutter-linux-release,flutter-linux-desktop-entrypoint,flutter-web-e2e,playwright,puppeteer,browser-e2e
            - name: BUILD_SERVER_ALLOWED_PROFILE_REPO_PREFIXES
              value: https://github.com/ORESoftware/,https://github.com/sonus-auris/,git@github.com:ORESoftware/,git@github.com:sonus-auris/,=https://github.com/messaging-intel/msgint-connectors.git
            - name: BUILD_SERVER_GIT_TOKEN_FILE
              value: /var/run/secrets/gha-clone-github/github_app_installation_token
          volumeMounts:
            - name: gha-clone-github-token
              mountPath: /var/run/secrets/gha-clone-github
              readOnly: true
      volumes:
        - name: gha-clone-github-token
          secret:
            secretName: dd-gha-clone-server-secrets
            defaultMode: 256
            items:
              - key: github_app_installation_token
                path: github_app_installation_token
""",
    encoding="utf-8",
)

path = Path("remote/argocd/dd-next-runtime/dd-gha-clone-server.externalsecret.yaml")
text = path.read_text(encoding="utf-8")
text = replace_once(
    text,
    "# github_app_installation_token must be short-lived and produced/rotated by the\n# reviewed GitHub App broker. Do not place a classic PAT in this secret.",
    "# github_app_installation_token must be short-lived and produced/rotated by the\n# reviewed GitHub App broker. It is projected as a file into both continuity\n# services, which reload it per GitHub/git operation. Do not place a classic PAT\n# in this secret.",
    "external secret rotation documentation",
)
path.write_text(text, encoding="utf-8")

# Keep the static deployment contract aligned with the new credential boundary.
path = Path("remote/tests/general/gha-clone-server-config.test.ts")
text = path.read_text(encoding="utf-8")
text = replace_once(
    text,
    "  assert.match(deployment, /name:\\s*dd-gha-clone-server-secrets/);",
    "  assert.match(deployment, /name:\\s*dd-gha-clone-server-secrets/);\n  assert.match(\n    deployment,\n    /name:\\s*GHA_CLONE_GITHUB_TOKEN_FILE\\s+value:\\s*\\/var\\/run\\/secrets\\/gha-clone-github\\/github_app_installation_token/,\n  );\n  assert.doesNotMatch(deployment, /name:\\s*GHA_CLONE_GITHUB_TOKEN\\s+valueFrom:/);\n  assert.match(deployment, /name:\\s*github-token\\s+secret:\\s+secretName:\\s*dd-gha-clone-server-secrets/);",
    "deployment token file assertions",
)
text = replace_once(
    text,
    "  assert.match(continuityPatch, /node-hardened-verify/);",
    "  assert.match(continuityPatch, /node-hardened-verify/);\n  assert.match(continuityPatch, /name:\\s*BUILD_SERVER_GIT_TOKEN_FILE/);\n  assert.match(continuityPatch, /github_app_installation_token/);",
    "build token file assertions",
)
text = text.replace(
    '  assert.match(integration, /env_remove\\(\"GHA_CLONE_GITHUB_TOKEN\"\\)/);',
    '  assert.match(integration, /env_remove\\(\"GHA_CLONE_GITHUB_TOKEN\"\\)/);\n  assert.match(integration, /env_remove\\(\"GHA_CLONE_GITHUB_TOKEN_FILE\"\\)/);',
)
path.write_text(text, encoding="utf-8")

path = Path("remote/deployments/gha-clone-server-rs/README.md")
text = path.read_text(encoding="utf-8")
text = replace_once(
    text,
    "| `GHA_CLONE_GITHUB_TOKEN` | short-lived GitHub App installation token for private workflow reads |",
    "| `GHA_CLONE_GITHUB_TOKEN_FILE` | preferred projected file containing a rotating GitHub App installation token |\n| `GHA_CLONE_GITHUB_TOKEN` | legacy inline token for local tests; mutually exclusive with the file source |",
    "gha clone credential table",
)
text = replace_once(
    text,
    "Use a GitHub App and External Secrets. Do not put classic PATs, private keys, or\nshared secrets in source, Argo parameters, Linear, logs, URLs, or image layers.",
    "Use a GitHub App and External Secrets. Do not put classic PATs, private keys, or\nshared secrets in source, Argo parameters, Linear, logs, URLs, or image layers.\nThe Kubernetes deployment projects the installation token as a Secret volume;\nthe server reads that file for every workflow fetch so broker rotation is live\nwithout a restart. Inline and file token sources are mutually exclusive.",
    "gha clone rotation documentation",
)
path.write_text(text, encoding="utf-8")

path = Path("remote/deployments/build-server-rs/readme.md")
text = path.read_text(encoding="utf-8")
marker = "`BUILD_SERVER_ALLOWED_PROFILE_REPO_PREFIXES` is a second, narrower allowlist applied only to\n"
addition = "`BUILD_SERVER_GIT_TOKEN_FILE` is the preferred private-clone credential. It is read for every git process so a projected short-lived GitHub App token can rotate without restarting the pod. `BUILD_SERVER_GIT_TOKEN` and the legacy `GH_PAT` remain compatibility fallbacks only when no file source is configured.\n\n"
if addition not in text:
    text = text.replace(marker, addition + marker, 1)
path.write_text(text, encoding="utf-8")
