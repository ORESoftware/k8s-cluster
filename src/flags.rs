//! Strict CLI flag normalization before telemetry and typed config reads.

use std::collections::HashMap;

use flags2env::BundledFlags2Env;

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
                && name
                    .chars()
                    .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.'))
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
    let config_path = std::env::current_dir()
        .map_err(|error| CliFlagError::Configuration(error.to_string()))?
        .join(".cli-flags.toml");
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
            dir.path().join(".cli-flags.toml"),
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

    #[test]
    fn flags_override_defaults_without_exposing_secrets() {
        let dir = config();
        let path = dir.path().join(".cli-flags.toml");
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
        let path = dir.path().join(".cli-flags.toml");
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
        let path = dir.path().join(".cli-flags.toml");
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
        let path = dir.path().join(".cli-flags.toml");
        let argv = vec![
            "threefa-sync-server".to_string(),
            "--postgres://embedded-secret@redacted.invalid/threefa".to_string(),
        ];
        let error = parse_cli_flags(&argv, path.to_str()).expect_err("unknown flag");
        let rendered = error.to_string();
        assert!(rendered.contains("<redacted-option>"));
        assert!(!rendered.contains("embedded-secret"));
    }
}
