//! Broker command-line flag reconciliation.
//!
//! The broker keeps the existing `LMX_*` environment contract as its runtime
//! configuration API. CLI flags are parsed through the statically linked
//! `flags2env` parser and reconciled into that same env-shaped map, with CLI
//! values taking precedence over process environment values.

use std::collections::BTreeMap;
use std::ffi::{CStr, CString};
use std::fs::File;
use std::os::raw::{c_char, c_int};
use std::path::{Path, PathBuf};

pub const CLI_FLAGS_CONFIG_ENV: &str = "LMX_CLI_FLAGS_CONFIG";
pub const CLI_FLAGS_FILE_NAME: &str = ".cli-flags.toml";
pub const DEFAULT_ETC_CLI_FLAGS_CONFIG_PATH: &str = "/etc/dd-rust-network-mutex/.cli-flags.toml";

const PACKAGE_SHARE_DIR: &str = "dd-rust-network-mutex";
const PARSE_ERRORS_ENV: &str = "LMX_CLI_PARSE_ERRORS";
const POSITIONALS_ENV: &str = "LMX_CLI_POSITIONALS";
const UNKNOWN_OPTIONS_ENV: &str = "LMX_CLI_UNKNOWN_OPTIONS";

unsafe extern "C" {
    fn f2e_parse_json_argv_from_file(
        config_path: *const c_char,
        argv_json: *const c_char,
    ) -> *mut c_char;
    fn f2e_help_table_from_file(
        config_path: *const c_char,
        command_name: *const c_char,
        terminal_columns: c_int,
    ) -> *mut c_char;
    fn f2e_free(value: *mut c_char);
}

#[derive(Debug, Clone)]
pub enum BrokerCliConfig {
    Run(BrokerCliEnv),
    Help(BrokerCliHelp),
}

#[derive(Debug, Clone)]
pub struct BrokerCliEnv {
    merged_env: BTreeMap<String, String>,
    cli_overrides: BTreeMap<String, String>,
    source_path: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct BrokerCliHelp {
    table: String,
    source_path: PathBuf,
}

#[derive(Debug, thiserror::Error)]
pub enum CliFlagError {
    #[error("failed to serialize broker argv for flags2env")]
    ArgsJson {
        #[source]
        source: serde_json::Error,
    },
    #[error("failed to parse flags2env JSON output while {operation}")]
    NativeJson {
        operation: &'static str,
        #[source]
        source: serde_json::Error,
    },
    #[error("failed to parse flags2env metadata env {key}")]
    MetadataJson {
        key: &'static str,
        #[source]
        source: serde_json::Error,
    },
    #[error("broker CLI config path is not valid UTF-8")]
    NonUtf8ConfigPath,
    #[error("broker CLI config path contains an interior NUL byte")]
    ConfigPathNul,
    #[error("broker CLI command name contains an interior NUL byte")]
    CommandNameNul,
    #[error("flags2env returned a null pointer while {operation}")]
    NativeNull { operation: &'static str },
    #[error(
        "no reviewed broker CLI contract found; set LMX_CLI_FLAGS_CONFIG to an absolute readable file or install the package-owned contract"
    )]
    MissingConfig,
    #[error("LMX_CLI_FLAGS_CONFIG must be an absolute path")]
    ExplicitConfigMustBeAbsolute,
    #[error("LMX_CLI_FLAGS_CONFIG does not name a readable regular file")]
    ExplicitConfigUnreadable,
    #[error("invalid broker CLI flag value(s) (count: {count})")]
    ParseErrors { count: usize },
    #[error("unknown broker CLI option(s) (count: {count})")]
    UnknownOptions { count: usize },
    #[error("unexpected broker positional argument(s) (count: {count})")]
    Positionals { count: usize },
}

impl BrokerCliEnv {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.merged_env.get(key).map(String::as_str)
    }

    pub fn merged_env(&self) -> &BTreeMap<String, String> {
        &self.merged_env
    }

    pub fn cli_overrides(&self) -> &BTreeMap<String, String> {
        &self.cli_overrides
    }

    pub fn source_path(&self) -> Option<&Path> {
        self.source_path.as_deref()
    }

    pub fn apply_cli_overrides_to_process_env(&self) {
        for (key, value) in &self.cli_overrides {
            std::env::set_var(key, value);
        }
    }
}

impl BrokerCliHelp {
    pub fn table(&self) -> &str {
        &self.table
    }

    pub fn source_path(&self) -> &Path {
        &self.source_path
    }
}

pub fn load_broker_cli_config() -> Result<BrokerCliConfig, CliFlagError> {
    load_broker_cli_config_from(std::env::args().collect(), std::env::vars())
}

fn load_broker_cli_config_from<I, K, V>(
    args: Vec<String>,
    env: I,
) -> Result<BrokerCliConfig, CliFlagError>
where
    I: IntoIterator<Item = (K, V)>,
    K: Into<String>,
    V: Into<String>,
{
    let process_env = env
        .into_iter()
        .map(|(key, value)| (key.into(), value.into()))
        .collect::<BTreeMap<_, _>>();

    let user_args = args.iter().skip(1).cloned().collect::<Vec<_>>();
    let config_path = resolve_cli_flags_config_path(&process_env)?;

    let Some(config_path) = config_path else {
        if user_args.is_empty() {
            return Ok(BrokerCliConfig::Run(BrokerCliEnv {
                merged_env: process_env,
                cli_overrides: BTreeMap::new(),
                source_path: None,
            }));
        }
        return Err(CliFlagError::MissingConfig);
    };

    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        return Ok(BrokerCliConfig::Help(BrokerCliHelp {
            table: render_help_table(&config_path, command_name(&args)?)?,
            source_path: config_path,
        }));
    }

    let mut cli_overrides = parse_cli_overrides(&config_path, &user_args)?;
    validate_parser_metadata(&mut cli_overrides)?;

    let mut merged_env = process_env;
    for (key, value) in &cli_overrides {
        merged_env.insert(key.clone(), value.clone());
    }

    Ok(BrokerCliConfig::Run(BrokerCliEnv {
        merged_env,
        cli_overrides,
        source_path: Some(config_path),
    }))
}

fn resolve_cli_flags_config_path(
    env: &BTreeMap<String, String>,
) -> Result<Option<PathBuf>, CliFlagError> {
    let explicit = env
        .get(CLI_FLAGS_CONFIG_ENV)
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(PathBuf::from);
    let executable = std::env::current_exe().ok();
    let implicit_candidates = vec![
        PathBuf::from(DEFAULT_ETC_CLI_FLAGS_CONFIG_PATH),
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(CLI_FLAGS_FILE_NAME),
    ];

    resolve_cli_flags_config_path_from(explicit, executable, implicit_candidates)
}

fn resolve_cli_flags_config_path_from(
    explicit: Option<PathBuf>,
    executable: Option<PathBuf>,
    implicit_candidates: Vec<PathBuf>,
) -> Result<Option<PathBuf>, CliFlagError> {
    if let Some(path) = explicit {
        return validate_explicit_config(path).map(Some);
    }

    let mut candidates = Vec::new();
    if let Some(parent) = executable.as_deref().and_then(Path::parent) {
        candidates.push(
            parent
                .join("..")
                .join("share")
                .join(PACKAGE_SHARE_DIR)
                .join(CLI_FLAGS_FILE_NAME),
        );
        candidates.push(parent.join(CLI_FLAGS_FILE_NAME));
    }
    candidates.extend(implicit_candidates);

    Ok(candidates
        .into_iter()
        .find_map(|candidate| trusted_regular_file(&candidate)))
}

fn validate_explicit_config(path: PathBuf) -> Result<PathBuf, CliFlagError> {
    if !path.is_absolute() {
        return Err(CliFlagError::ExplicitConfigMustBeAbsolute);
    }
    trusted_regular_file(&path).ok_or(CliFlagError::ExplicitConfigUnreadable)
}

fn trusted_regular_file(path: &Path) -> Option<PathBuf> {
    if !path.is_absolute() {
        return None;
    }
    let canonical = path.canonicalize().ok()?;
    if !canonical.metadata().ok()?.is_file() {
        return None;
    }
    File::open(&canonical).ok()?;
    Some(canonical)
}

fn parse_cli_overrides(
    config_path: &Path,
    user_args: &[String],
) -> Result<BTreeMap<String, String>, CliFlagError> {
    let argv_json =
        serde_json::to_string(user_args).map_err(|source| CliFlagError::ArgsJson { source })?;
    let config_path = cstring_path(config_path)?;
    let argv_json = CString::new(argv_json).expect("serde_json escaped interior NUL bytes");
    let raw = unsafe {
        take_owned_c_string(
            f2e_parse_json_argv_from_file(config_path.as_ptr(), argv_json.as_ptr()),
            "parsing broker CLI flags",
        )?
    };

    serde_json::from_str(&raw).map_err(|source| CliFlagError::NativeJson {
        operation: "parsing broker CLI flags",
        source,
    })
}

fn render_help_table(config_path: &Path, command_name: String) -> Result<String, CliFlagError> {
    let config_path = cstring_path(config_path)?;
    let command_name = CString::new(command_name).map_err(|_| CliFlagError::CommandNameNul)?;
    let columns = terminal_columns();
    unsafe {
        take_owned_c_string(
            f2e_help_table_from_file(config_path.as_ptr(), command_name.as_ptr(), columns),
            "rendering broker CLI help",
        )
    }
}

fn validate_parser_metadata(
    cli_overrides: &mut BTreeMap<String, String>,
) -> Result<(), CliFlagError> {
    let parse_errors = take_json_array(cli_overrides, PARSE_ERRORS_ENV)?;
    let unknown_options = take_json_array(cli_overrides, UNKNOWN_OPTIONS_ENV)?;
    let positionals = take_json_array(cli_overrides, POSITIONALS_ENV)?;

    if !parse_errors.is_empty() {
        return Err(CliFlagError::ParseErrors {
            count: parse_errors.len(),
        });
    }
    if !unknown_options.is_empty() {
        return Err(CliFlagError::UnknownOptions {
            count: unknown_options.len(),
        });
    }
    if !positionals.is_empty() {
        return Err(CliFlagError::Positionals {
            count: positionals.len(),
        });
    }

    Ok(())
}

fn take_json_array(
    map: &mut BTreeMap<String, String>,
    key: &'static str,
) -> Result<Vec<String>, CliFlagError> {
    let Some(value) = map.remove(key) else {
        return Ok(Vec::new());
    };

    serde_json::from_str(&value).map_err(|source| CliFlagError::MetadataJson { key, source })
}

fn cstring_path(path: &Path) -> Result<CString, CliFlagError> {
    let value = path.to_str().ok_or(CliFlagError::NonUtf8ConfigPath)?;
    CString::new(value).map_err(|_| CliFlagError::ConfigPathNul)
}

unsafe fn take_owned_c_string(
    ptr: *mut c_char,
    operation: &'static str,
) -> Result<String, CliFlagError> {
    if ptr.is_null() {
        return Err(CliFlagError::NativeNull { operation });
    }
    let value = unsafe { CStr::from_ptr(ptr) }
        .to_string_lossy()
        .into_owned();
    unsafe { f2e_free(ptr) };
    Ok(value)
}

fn terminal_columns() -> c_int {
    std::env::var("COLUMNS")
        .ok()
        .and_then(|value| value.trim().parse::<c_int>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(100)
}

fn command_name(args: &[String]) -> Result<String, CliFlagError> {
    let raw = args
        .first()
        .map(String::as_str)
        .unwrap_or("dd-rust-network-mutex");
    let name = Path::new(raw)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(raw)
        .to_string();
    if name.contains('\0') {
        return Err(CliFlagError::CommandNameNul);
    }
    Ok(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    const SOURCE: &str = include_str!("cli_flags.rs");

    struct TestTree(PathBuf);

    impl TestTree {
        fn new(name: &str) -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock")
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "live-mutex-cli-flags-{name}-{}-{nonce}",
                std::process::id()
            ));
            fs::create_dir_all(&path).expect("create test tree");
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TestTree {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn write_contract(path: &Path) {
        fs::create_dir_all(path.parent().expect("contract parent"))
            .expect("create contract parent");
        fs::write(
            path,
            r#"
[parse]
unknown_options_env = "LMX_CLI_UNKNOWN_OPTIONS"
errors_env = "LMX_CLI_PARSE_ERRORS"

[flags.tcp_port]
env = "LMX_TCP_PORT"
aliases = ["tcp-port"]
type = "integer"
"#,
        )
        .expect("write contract");
    }

    fn manifest_cli_config() -> String {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(CLI_FLAGS_FILE_NAME)
            .to_string_lossy()
            .into_owned()
    }

    fn env_with_manifest_config(extra: &[(&str, &str)]) -> Vec<(String, String)> {
        let mut env = vec![(CLI_FLAGS_CONFIG_ENV.to_string(), manifest_cli_config())];
        env.extend(
            extra
                .iter()
                .map(|(key, value)| ((*key).to_string(), (*value).to_string())),
        );
        env
    }

    #[test]
    fn cli_flags_override_env_values() {
        let cfg = load_broker_cli_config_from(
            vec![
                "dd-rust-network-mutex".into(),
                "--tcp-port".into(),
                "7777".into(),
                "--disable-http".into(),
            ],
            env_with_manifest_config(&[("LMX_TCP_PORT", "6970"), ("LMX_DISABLE_HTTP", "false")]),
        )
        .expect("cli config");

        let BrokerCliConfig::Run(env) = cfg else {
            panic!("expected run config");
        };

        assert_eq!(env.get("LMX_TCP_PORT"), Some("7777"));
        assert_eq!(env.get("LMX_DISABLE_HTTP"), Some("true"));
        assert_eq!(
            env.cli_overrides().get("LMX_TCP_PORT"),
            Some(&"7777".into())
        );
        assert_eq!(
            env.cli_overrides().get("LMX_DISABLE_HTTP"),
            Some(&"true".into())
        );
    }

    #[test]
    fn env_values_remain_when_cli_omits_them() {
        let cfg = load_broker_cli_config_from(
            vec!["dd-rust-network-mutex".into()],
            env_with_manifest_config(&[("LMX_HTTP_PORT", "6971")]),
        )
        .expect("cli config");

        let BrokerCliConfig::Run(env) = cfg else {
            panic!("expected run config");
        };

        assert_eq!(env.get("LMX_HTTP_PORT"), Some("6971"));
        assert!(env.cli_overrides().is_empty());
    }

    #[test]
    fn unknown_options_are_rejected_without_echoing_values() {
        let rejected = "postgres://runtime-secret@redacted.invalid/lmx";
        let err = load_broker_cli_config_from(
            vec![
                "dd-rust-network-mutex".into(),
                format!("--not-a-real-flag={rejected}"),
            ],
            env_with_manifest_config(&[]),
        )
        .expect_err("unknown flag should fail");

        assert!(matches!(
            &err,
            CliFlagError::UnknownOptions { count } if *count > 0
        ));
        assert!(!err.to_string().contains(rejected));
        assert!(!err.to_string().contains("runtime-secret"));
    }

    #[test]
    fn invalid_typed_values_are_rejected_without_echoing_values() {
        let rejected = "not-a-port-runtime-secret";
        let err = load_broker_cli_config_from(
            vec![
                "dd-rust-network-mutex".into(),
                "--tcp-port".into(),
                rejected.into(),
            ],
            env_with_manifest_config(&[]),
        )
        .expect_err("invalid integer flag should fail");

        assert!(matches!(
            &err,
            CliFlagError::ParseErrors { count } if *count > 0
        ));
        assert!(!err.to_string().contains(rejected));
        assert!(!err.to_string().contains("runtime-secret"));
    }

    #[test]
    fn explicit_missing_cli_flags_config_is_rejected_without_path_echo() {
        let missing = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target/definitely-missing-runtime-secret.toml")
            .to_string_lossy()
            .into_owned();
        let err = load_broker_cli_config_from(
            vec![
                "dd-rust-network-mutex".into(),
                "--tcp-port".into(),
                "6970".into(),
            ],
            vec![(CLI_FLAGS_CONFIG_ENV.to_string(), missing.clone())],
        )
        .expect_err("missing explicit config should fail");

        assert!(matches!(err, CliFlagError::ExplicitConfigUnreadable));
        let display = CliFlagError::ExplicitConfigUnreadable.to_string();
        assert!(!display.contains(&missing));
        assert!(!display.contains("runtime-secret"));
    }

    #[test]
    fn relative_explicit_cli_flags_config_fails_closed() {
        let err = load_broker_cli_config_from(
            vec!["dd-rust-network-mutex".into(), "--help".into()],
            vec![(
                CLI_FLAGS_CONFIG_ENV.to_string(),
                "attacker-runtime-secret.toml".to_string(),
            )],
        )
        .expect_err("relative explicit config should fail");

        assert!(matches!(err, CliFlagError::ExplicitConfigMustBeAbsolute));
        let display = CliFlagError::ExplicitConfigMustBeAbsolute.to_string();
        assert!(!display.contains("attacker-runtime-secret.toml"));
        assert!(!display.contains("runtime-secret"));
    }

    #[test]
    fn explicit_selector_wins_and_never_falls_through() {
        let tree = TestTree::new("explicit-precedence");
        let source = tree.path().join("source/.cli-flags.toml");
        write_contract(&source);

        let err =
            resolve_cli_flags_config_path_from(Some(PathBuf::from("relative.toml")), None, vec![source])
                .expect_err("invalid explicit selector must not fall through");
        assert!(matches!(err, CliFlagError::ExplicitConfigMustBeAbsolute));
    }

    #[test]
    fn packaged_share_contract_beats_colocated_and_fixed_contracts() {
        let tree = TestTree::new("package-order");
        let executable = tree.path().join("install/bin/dd-rust-network-mutex");
        fs::create_dir_all(executable.parent().expect("executable parent"))
            .expect("create executable parent");
        let packaged = tree
            .path()
            .join("install/share/dd-rust-network-mutex/.cli-flags.toml");
        let colocated = tree.path().join("install/bin/.cli-flags.toml");
        let fixed = tree.path().join("etc/.cli-flags.toml");
        write_contract(&packaged);
        write_contract(&colocated);
        write_contract(&fixed);

        let resolved =
            resolve_cli_flags_config_path_from(None, Some(executable), vec![fixed])
                .expect("trusted candidates")
                .expect("packaged contract");
        assert_eq!(
            resolved,
            packaged.canonicalize().expect("canonical packaged contract")
        );
    }

    #[test]
    fn fixed_or_source_owned_contracts_remain_supported() {
        let tree = TestTree::new("fixed-source");
        let fixed = tree.path().join("etc/.cli-flags.toml");
        let source = tree.path().join("source/.cli-flags.toml");
        write_contract(&fixed);
        write_contract(&source);

        let resolved = resolve_cli_flags_config_path_from(
            None,
            None,
            vec![fixed.clone(), source],
        )
        .expect("trusted candidates")
        .expect("fixed contract");
        assert_eq!(
            resolved,
            fixed.canonicalize().expect("canonical fixed contract")
        );
    }

    #[test]
    fn unrelated_working_directory_contract_is_never_a_candidate() {
        let tree = TestTree::new("hostile-cwd");
        let attacker = tree.path().join("attacker/.cli-flags.toml");
        let executable = tree.path().join("install/bin/dd-rust-network-mutex");
        let packaged = tree
            .path()
            .join("install/share/dd-rust-network-mutex/.cli-flags.toml");
        fs::create_dir_all(executable.parent().expect("executable parent"))
            .expect("create executable parent");
        write_contract(&attacker);
        write_contract(&packaged);

        let resolved =
            resolve_cli_flags_config_path_from(None, Some(executable), Vec::new())
                .expect("trusted candidates")
                .expect("packaged contract");
        assert_ne!(
            resolved,
            attacker.canonicalize().expect("canonical attacker contract")
        );
        assert_eq!(
            resolved,
            packaged.canonicalize().expect("canonical packaged contract")
        );
    }

    #[test]
    fn help_is_rendered_from_cli_flags_config() {
        let cfg = load_broker_cli_config_from(
            vec!["dd-rust-network-mutex".into(), "--help".into()],
            env_with_manifest_config(&[]),
        )
        .expect("help config");

        let BrokerCliConfig::Help(help) = cfg else {
            panic!("expected help config");
        };

        assert!(help.table().contains("--tcp-port"));
        assert!(help.table().contains("LMX_TCP_PORT"));
    }

    #[test]
    fn production_source_has_no_working_directory_contract_discovery() {
        for forbidden in [
            concat!("current_", "dir("),
            concat!("find_upward_", "cli_flags_config"),
        ] {
            assert!(
                !SOURCE.contains(forbidden),
                "cli_flags.rs contains forbidden ambient discovery: {forbidden}"
            );
        }
    }
}
