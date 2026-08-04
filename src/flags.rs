//! Strict CLI flag normalization before telemetry and typed config reads.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use flags2env::BundledFlags2Env;

const CONFIG_ENV: &str = "THREEFA_FLAGS_CONFIG";
const CONFIG_FILE: &str = ".cli-flags.toml";
const PACKAGE_SHARE_DIR: &str = "threefa-backend";

#[derive(Debug, thiserror::Error)]
pub enum CliFlagError {
    #[error("cannot resolve command-line configuration: {0}")]
    Configuration(String),
    #[error("flags-2-env configuration audit failed: {0}")]
    Audit(String),
    #[error("flags-2-env parse failed: {0}")]
    Parse(String),
    #[error("unknown command-line option(s): {0}")]
    UnknownOptions(String),
    #[error("invalid command-line value(s): {0}")]
    InvalidValues(String),
}

fn is_safe_option_name(option: &str) -> bool {
    option
        .strip_prefix("--")
        .or_else(|| option.strip_prefix('-'))
        .is_some_and(|name| {
            !name.is_empty()
                && name.chars().all(|character| {
                    character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
                })
        })
}

fn redact_unknown_option(option: &str) -> String {
    let (name, has_value) = option
        .split_once('=')
        .map_or((option, false), |(name, _)| (name, true));
    if !is_safe_option_name(name) {
        return "<redacted-option>".to_string();
    }
    if has_value {
        format!("{name}=<redacted>")
    } else {
        name.to_owned()
    }
}

fn resolve_config_path() -> Result<PathBuf, CliFlagError> {
    let explicit = std::env::var_os(CONFIG_ENV)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from);
    let executable = std::env::current_exe().ok();
    let source_root = Some(PathBuf::from(env!("CARGO_MANIFEST_DIR")));
    resolve_config_path_from(explicit, executable, source_root)
}

fn resolve_config_path_from(
    explicit: Option<PathBuf>,
    executable: Option<PathBuf>,
    source_root: Option<PathBuf>,
) -> Result<PathBuf, CliFlagError> {
    if let Some(path) = explicit {
        if !path.is_absolute() {
            return Err(CliFlagError::Configuration(format!(
                "{CONFIG_ENV} must be an absolute path"
            )));
        }
        if path.is_file() {
            return path.canonicalize().map_err(|_| {
                CliFlagError::Configuration(format!(
                    "{CONFIG_ENV} does not name a readable regular file"
                ))
            });
        }
        return Err(CliFlagError::Configuration(format!(
            "{CONFIG_ENV} does not name a readable regular file"
        )));
    }

    let mut candidates = Vec::new();
    if let Some(parent) = executable.as_deref().and_then(Path::parent) {
        candidates.push(
            parent
                .join("..")
                .join("share")
                .join(PACKAGE_SHARE_DIR)
                .join(CONFIG_FILE),
        );
        candidates.push(parent.join(CONFIG_FILE));
    }
    if let Some(source_root) = source_root {
        // Source-tree development remains convenient without letting a service
        // launcher or container WORKDIR choose executable policy at runtime.
        candidates.push(source_root.join(CONFIG_FILE));
    }

    candidates
        .into_iter()
        .find(|candidate| candidate.is_file())
        .ok_or_else(|| {
            CliFlagError::Configuration(format!(
                "cannot locate trusted {CONFIG_FILE}; set {CONFIG_ENV} to an absolute reviewed path or install it beside the executable or under ../share/{PACKAGE_SHARE_DIR}"
            ))
        })
}

pub fn parse_cli_flags(
    argv: &[String],
    config_path: Option<&str>,
) -> Result<HashMap<String, String>, CliFlagError> {
    let parser = BundledFlags2Env::new();
    parser
        .audit_config(config_path)
        .map_err(|error| CliFlagError::Audit(error.to_string()))?;
    let parsed = parser
        .parse_structured(argv, config_path)
        .map_err(|error| CliFlagError::Parse(error.to_string()))?;
    if !parsed.unknown_options.is_empty() {
        return Err(CliFlagError::UnknownOptions(
            parsed
                .unknown_options
                .iter()
                .map(|option| redact_unknown_option(option))
                .collect::<Vec<_>>()
                .join(", "),
        ));
    }
    if !parsed.errors.is_empty() {
        return Err(CliFlagError::InvalidValues(parsed.errors.join("; ")));
    }
    Ok(parsed.flags)
}

/// Apply CLI-derived environment overrides exactly once at process startup.
///
/// Call this before telemetry initialization, Tokio worker threads, or typed
/// configuration reads. Secrets are not declared as flags in `.cli-flags.toml`.
pub fn apply_cli_flags() -> Result<(), CliFlagError> {
    let config_path = resolve_config_path()?;
    let config_path = config_path.to_str().ok_or_else(|| {
        CliFlagError::Configuration(".cli-flags.toml path is not valid UTF-8".to_string())
    })?;
    let argv = std::env::args().collect::<Vec<_>>();
    for (key, value) in parse_cli_flags(&argv, Some(config_path))? {
        std::env::set_var(key, value);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    fn config() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("temporary directory");
        fs::write(
            dir.path().join(CONFIG_FILE),
            r#"
[parse]
allow_unknown = false

[env]
ignore = ["DATABASE_URL"]

[flags.bind-addr]
env = "BIND_ADDR"
aliases = ["bind-addr"]
type = "string"
default = "0.0.0.0:8080"

[flags.metrics-bind-addr]
env = "METRICS_BIND_ADDR"
aliases = ["metrics-bind-addr"]
type = "string"
default = "0.0.0.0:9091"
"#,
        )
        .expect("write config");
        dir
    }

    fn write_contract(path: &Path) {
        fs::create_dir_all(path.parent().expect("contract parent"))
            .expect("create contract parent");
        fs::write(
            path,
            r#"
[parse]
allow_unknown = false

[flags.bind-addr]
env = "BIND_ADDR"
aliases = ["bind-addr"]
type = "string"
default = "0.0.0.0:8080"
"#,
        )
        .expect("write contract");
    }

    #[test]
    fn flags_override_defaults_without_exposing_secrets() {
        let dir = config();
        let path = dir.path().join(CONFIG_FILE);
        let argv = vec![
            "threefa-sync-server".to_string(),
            "--bind-addr=127.0.0.1:18080".to_string(),
        ];
        let parsed = parse_cli_flags(&argv, path.to_str()).expect("valid flags");
        assert_eq!(
            parsed.get("BIND_ADDR").map(String::as_str),
            Some("127.0.0.1:18080")
        );
        assert_eq!(
            parsed.get("METRICS_BIND_ADDR").map(String::as_str),
            Some("0.0.0.0:9091")
        );
        assert!(!parsed.contains_key("DATABASE_URL"));
    }

    #[test]
    fn unknown_flags_fail_closed_without_echoing_values() {
        let dir = config();
        let path = dir.path().join(CONFIG_FILE);
        let argv = vec![
            "threefa-sync-server".to_string(),
            "--database-url=postgres://should-not-be-a-flag".to_string(),
        ];
        let error = parse_cli_flags(&argv, path.to_str()).expect_err("unknown flag");
        assert!(matches!(error, CliFlagError::UnknownOptions(_)));
        let rendered = error.to_string();
        assert!(rendered.contains("--database-url=<redacted>"));
        assert!(!rendered.contains("should-not-be-a-flag"));
    }

    #[test]
    fn split_unknown_flag_does_not_echo_the_following_positional_value() {
        let dir = config();
        let path = dir.path().join(CONFIG_FILE);
        let argv = vec![
            "threefa-sync-server".to_string(),
            "--database-url".to_string(),
            "postgres://split-secret@redacted.invalid/threefa".to_string(),
        ];
        let error = parse_cli_flags(&argv, path.to_str()).expect_err("unknown flag");
        let rendered = error.to_string();
        assert!(rendered.contains("--database-url"));
        assert!(!rendered.contains("split-secret"));
    }

    #[test]
    fn malformed_secret_bearing_option_tokens_are_fully_redacted() {
        let dir = config();
        let path = dir.path().join(CONFIG_FILE);
        let argv = vec![
            "threefa-sync-server".to_string(),
            "--postgres://embedded-secret@redacted.invalid/threefa".to_string(),
        ];
        let error = parse_cli_flags(&argv, path.to_str()).expect_err("unknown flag");
        let rendered = error.to_string();
        assert!(rendered.contains("<redacted-option>"));
        assert!(!rendered.contains("embedded-secret"));
    }

    #[test]
    fn explicit_contract_override_requires_an_absolute_regular_file() {
        let tree = tempfile::tempdir().expect("temporary tree");
        let explicit = tree.path().join("operator/reviewed.toml");
        let executable = tree.path().join("install/bin/threefa-sync-server");
        let packaged = tree
            .path()
            .join("install/share/threefa-backend/.cli-flags.toml");
        write_contract(&explicit);
        write_contract(&packaged);

        let resolved = resolve_config_path_from(
            Some(explicit.clone()),
            Some(executable.clone()),
            None,
        )
        .expect("explicit contract");
        assert_eq!(
            resolved,
            explicit.canonicalize().expect("canonical explicit contract")
        );

        let relative_error = resolve_config_path_from(
            Some(PathBuf::from("reviewed.toml")),
            Some(executable.clone()),
            None,
        )
        .expect_err("relative explicit override must fail closed")
        .to_string();
        assert!(relative_error.contains("absolute path"));
        assert!(!relative_error.contains("reviewed.toml"));

        let missing = tree.path().join("missing.toml");
        let error = resolve_config_path_from(Some(missing.clone()), Some(executable), None)
            .expect_err("missing explicit override must fail closed")
            .to_string();
        assert!(error.contains(CONFIG_ENV));
        assert!(!error.contains(&missing.display().to_string()));
    }

    #[test]
    fn packaged_share_contract_beats_colocated_contract() {
        let tree = tempfile::tempdir().expect("temporary tree");
        let executable = tree.path().join("install/bin/threefa-sync-server");
        let packaged = tree
            .path()
            .join("install/share/threefa-backend/.cli-flags.toml");
        let colocated = tree.path().join("install/bin/.cli-flags.toml");
        write_contract(&packaged);
        write_contract(&colocated);

        let resolved = resolve_config_path_from(None, Some(executable.clone()), None)
            .expect("packaged contract");
        assert_eq!(
            resolved,
            executable
                .parent()
                .expect("executable parent")
                .join("../share/threefa-backend/.cli-flags.toml")
        );
    }

    #[test]
    fn unrelated_working_directory_contract_is_not_a_candidate() {
        let tree = tempfile::tempdir().expect("temporary tree");
        let attacker_contract = tree.path().join("attacker/.cli-flags.toml");
        let executable = tree.path().join("install/bin/threefa-sync-server");
        let packaged = tree
            .path()
            .join("install/share/threefa-backend/.cli-flags.toml");
        write_contract(&attacker_contract);
        write_contract(&packaged);
        fs::create_dir_all(executable.parent().expect("executable parent"))
            .expect("create executable parent");

        let resolved = resolve_config_path_from(None, Some(executable.clone()), None)
            .expect("trusted package contract");
        assert_ne!(resolved, attacker_contract);
        assert_eq!(
            resolved,
            executable
                .parent()
                .expect("executable parent")
                .join("../share/threefa-backend/.cli-flags.toml")
        );
    }

    #[test]
    fn source_tree_contract_keeps_cargo_run_usable() {
        let tree = tempfile::tempdir().expect("temporary tree");
        let source_contract = tree.path().join("source/.cli-flags.toml");
        write_contract(&source_contract);

        let resolved = resolve_config_path_from(
            None,
            Some(tree.path().join("target/debug/threefa-sync-server")),
            Some(tree.path().join("source")),
        )
        .expect("source contract");
        assert_eq!(resolved, source_contract);
    }
}
