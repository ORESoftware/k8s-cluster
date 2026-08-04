use std::{
    collections::HashMap,
    env,
    ffi::{CStr, CString},
    fs::File,
    os::raw::{c_char, c_int},
    path::{Path, PathBuf},
    sync::OnceLock,
};

const COMMAND_NAME: &str = "dd-in-house-mip-solver-node";
const CONFIG_ENV: &str = "FLAGS2ENV_CONFIG";
const CONFIG_FILE: &str = ".cli-flags.toml";
const PACKAGE_SHARE_DIR: &str = "dd-in-house-mip-solver-node";

unsafe extern "C" {
    fn f2e_parse_json_argv_from_file(
        config_path: *const c_char,
        argv_json: *const c_char,
    ) -> *mut c_char;
    fn f2e_is_help_requested_json_argv(argv_json: *const c_char) -> c_int;
    fn f2e_help_table_from_file(
        config_path: *const c_char,
        command_name: *const c_char,
        terminal_columns: c_int,
    ) -> *mut c_char;
    fn f2e_free(value: *mut c_char);
}

static RUNTIME_CONFIG: OnceLock<RuntimeConfig> = OnceLock::new();

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CliFlagSource {
    LinkedParser,
    EnvOnly,
}

#[derive(Clone, Debug)]
pub struct RuntimeConfig {
    values: HashMap<String, String>,
    cli_values: HashMap<String, String>,
    config_path: Option<PathBuf>,
    cli_flag_source: CliFlagSource,
    help_requested: bool,
    help_table: Option<String>,
    warnings: Vec<String>,
    fatal_error: Option<String>,
}

impl RuntimeConfig {
    pub fn load(argv: Vec<String>) -> Self {
        let env_values: HashMap<String, String> = env::vars().collect();
        let parsing_requested =
            has_cli_flags(&argv) || has_non_empty(&env_values, CONFIG_ENV);
        let help_requested = has_help_flag(&argv);

        let config_path = match discover_config_path(&env_values, &argv) {
            Ok(path) => path,
            Err(error) => {
                return Self::from_parts(
                    env_values,
                    HashMap::new(),
                    None,
                    CliFlagSource::EnvOnly,
                    help_requested,
                    None,
                    Vec::new(),
                    Some(error),
                );
            }
        };

        let Some(config_path) = config_path else {
            let fatal_error = parsing_requested
                .then(|| "trusted flags2env config not found".to_string());
            return Self::from_parts(
                env_values,
                HashMap::new(),
                None,
                CliFlagSource::EnvOnly,
                help_requested,
                None,
                Vec::new(),
                fatal_error,
            );
        };

        if help_requested {
            return match help_table_from_linked_parser(&argv, &config_path) {
                Ok(Some(help_table)) => Self::from_parts(
                    env_values,
                    HashMap::new(),
                    Some(config_path),
                    CliFlagSource::LinkedParser,
                    true,
                    Some(help_table),
                    Vec::new(),
                    None,
                ),
                Ok(None) | Err(_) => Self::from_parts(
                    env_values,
                    HashMap::new(),
                    Some(config_path),
                    CliFlagSource::LinkedParser,
                    true,
                    None,
                    Vec::new(),
                    Some(
                        "flags2env linked parser could not generate the help table".to_string(),
                    ),
                ),
            };
        }

        let parsed = match parse_with_linked_parser(&argv, &config_path) {
            Ok(values) => values,
            Err(_) => {
                return Self::from_parts(
                    env_values,
                    HashMap::new(),
                    Some(config_path),
                    CliFlagSource::LinkedParser,
                    false,
                    None,
                    Vec::new(),
                    Some("flags2env linked parser failed".to_string()),
                );
            }
        };

        match validated_cli_values(parsed) {
            Ok(cli_values) => Self::from_parts(
                env_values,
                cli_values,
                Some(config_path),
                CliFlagSource::LinkedParser,
                false,
                None,
                Vec::new(),
                None,
            ),
            Err(error) => Self::from_parts(
                env_values,
                HashMap::new(),
                Some(config_path),
                CliFlagSource::LinkedParser,
                false,
                None,
                Vec::new(),
                Some(error),
            ),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn from_parts(
        env_values: HashMap<String, String>,
        cli_values: HashMap<String, String>,
        config_path: Option<PathBuf>,
        cli_flag_source: CliFlagSource,
        help_requested: bool,
        help_table: Option<String>,
        warnings: Vec<String>,
        fatal_error: Option<String>,
    ) -> Self {
        let mut values = env_values;
        values.extend(cli_values.clone());
        RuntimeConfig {
            values,
            cli_values,
            config_path,
            cli_flag_source,
            help_requested,
            help_table,
            warnings,
            fatal_error,
        }
    }

    pub fn value(&self, key: &str, fallback: &str) -> String {
        self.optional_value(key)
            .unwrap_or_else(|| fallback.to_string())
    }

    pub fn optional_value(&self, key: &str) -> Option<String> {
        self.values
            .get(key)
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    }

    pub fn u64_value(&self, key: &str, fallback: u64) -> u64 {
        self.optional_value(key)
            .and_then(|value| value.parse::<u64>().ok())
            .filter(|value| *value > 0)
            .unwrap_or(fallback)
    }

    pub fn usize_value(&self, key: &str, fallback: usize) -> usize {
        self.u64_value(key, fallback as u64) as usize
    }

    pub fn usize_value_allow_zero(&self, key: &str, fallback: usize) -> usize {
        self.optional_value(key)
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(fallback)
    }

    pub fn f64_value(&self, key: &str, fallback: f64) -> f64 {
        self.optional_value(key)
            .and_then(|value| value.parse::<f64>().ok())
            .filter(|value| value.is_finite() && *value > 0.0)
            .unwrap_or(fallback)
    }

    pub fn bool_value(&self, key: &str, fallback: bool) -> bool {
        match self.optional_value(key) {
            Some(value) => match value.to_ascii_lowercase().as_str() {
                "1" | "true" | "yes" | "y" | "on" => true,
                "0" | "false" | "no" | "n" | "off" => false,
                _ => fallback,
            },
            None => fallback,
        }
    }

    pub fn first_configured_key(&self, keys: &[&str]) -> Option<String> {
        keys.iter()
            .find(|key| self.optional_value(key).is_some())
            .map(|key| (*key).to_string())
    }

    pub fn first_configured_value(&self, keys: &[&str]) -> Option<String> {
        keys.iter().find_map(|key| self.optional_value(key))
    }

    pub fn config_path(&self) -> Option<&Path> {
        self.config_path.as_deref()
    }

    pub fn cli_flag_source(&self) -> CliFlagSource {
        self.cli_flag_source
    }

    pub fn cli_values(&self) -> &HashMap<String, String> {
        &self.cli_values
    }

    pub fn help_table(&self) -> Option<&str> {
        self.help_table.as_deref()
    }

    pub fn help_requested(&self) -> bool {
        self.help_requested
    }

    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    pub fn fatal_error(&self) -> Option<&str> {
        self.fatal_error.as_deref()
    }
}

pub fn initialize() -> &'static RuntimeConfig {
    let config = RUNTIME_CONFIG.get_or_init(|| RuntimeConfig::load(env::args().collect()));
    if let Some(error) = config.fatal_error() {
        eprintln!("runtime config error: {error}");
        std::process::exit(2);
    }
    config
}

pub fn current() -> &'static RuntimeConfig {
    initialize()
}

pub fn value(key: &str, fallback: &str) -> String {
    current().value(key, fallback)
}

pub fn optional_value(key: &str) -> Option<String> {
    current().optional_value(key)
}

pub fn u64_value(key: &str, fallback: u64) -> u64 {
    current().u64_value(key, fallback)
}

pub fn usize_value(key: &str, fallback: usize) -> usize {
    current().usize_value(key, fallback)
}

pub fn usize_value_allow_zero(key: &str, fallback: usize) -> usize {
    current().usize_value_allow_zero(key, fallback)
}

pub fn f64_value(key: &str, fallback: f64) -> f64 {
    current().f64_value(key, fallback)
}

pub fn bool_value(key: &str, fallback: bool) -> bool {
    current().bool_value(key, fallback)
}

pub fn first_configured_key(keys: &[&str]) -> Option<String> {
    current().first_configured_key(keys)
}

pub fn first_configured_value(keys: &[&str]) -> Option<String> {
    current().first_configured_value(keys)
}

fn help_table_from_linked_parser(
    argv: &[String],
    config_path: &Path,
) -> Result<Option<String>, String> {
    let argv_json = CString::new(serde_json::to_string(argv).map_err(|error| error.to_string())?)
        .map_err(|error| error.to_string())?;
    let help_requested = unsafe { f2e_is_help_requested_json_argv(argv_json.as_ptr()) } != 0;
    if !help_requested {
        return Ok(None);
    }

    let config_path = CString::new(config_path.to_string_lossy().as_bytes())
        .map_err(|error| error.to_string())?;
    let command_name = CString::new(COMMAND_NAME).map_err(|error| error.to_string())?;

    let table = unsafe {
        let result = f2e_help_table_from_file(config_path.as_ptr(), command_name.as_ptr(), 0);
        if result.is_null() {
            return Err("linked help table returned null".to_string());
        }
        let raw = CStr::from_ptr(result).to_string_lossy().to_string();
        f2e_free(result);
        raw
    };

    Ok(Some(table))
}

fn parse_with_linked_parser(
    argv: &[String],
    config_path: &Path,
) -> Result<HashMap<String, String>, String> {
    let argv_json = CString::new(serde_json::to_string(argv).map_err(|error| error.to_string())?)
        .map_err(|error| error.to_string())?;
    let config_path = CString::new(config_path.to_string_lossy().as_bytes())
        .map_err(|error| error.to_string())?;

    let parsed = unsafe {
        let result = f2e_parse_json_argv_from_file(config_path.as_ptr(), argv_json.as_ptr());
        if result.is_null() {
            return Err("linked parser returned null".to_string());
        }
        let raw = CStr::from_ptr(result).to_string_lossy().to_string();
        f2e_free(result);
        raw
    };

    serde_json::from_str(&parsed).map_err(|error| error.to_string())
}

fn validated_cli_values(
    mut values: HashMap<String, String>,
) -> Result<HashMap<String, String>, String> {
    let parse_errors = values
        .remove("MIP_SOLVER_CLI_PARSE_ERRORS")
        .is_some_and(|value| !value.trim().is_empty());
    let unknown_options = values
        .remove("MIP_SOLVER_UNKNOWN_CLI_FLAGS")
        .is_some_and(|value| !value.trim().is_empty());

    if parse_errors {
        return Err("flags2env rejected one or more CLI values".to_string());
    }
    if unknown_options {
        return Err("flags2env rejected one or more unknown CLI options".to_string());
    }
    Ok(values)
}

fn discover_config_path(
    env_values: &HashMap<String, String>,
    argv: &[String],
) -> Result<Option<PathBuf>, String> {
    let cli_explicit = cli_config_path(argv)?;
    let env_explicit = map_value(env_values, CONFIG_ENV).map(PathBuf::from);
    let executable = env::current_exe().ok();

    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut source_candidates = vec![manifest_dir.join(CONFIG_FILE)];
    if env!("CARGO_PKG_NAME").ends_with("-local") {
        if let Some(parent) = manifest_dir.parent() {
            source_candidates.push(parent.join(CONFIG_FILE));
        }
    }

    discover_config_path_from(
        cli_explicit,
        env_explicit,
        executable,
        source_candidates,
    )
}

fn discover_config_path_from(
    cli_explicit: Option<PathBuf>,
    env_explicit: Option<PathBuf>,
    executable: Option<PathBuf>,
    source_candidates: Vec<PathBuf>,
) -> Result<Option<PathBuf>, String> {
    if let Some(path) = cli_explicit {
        return validate_explicit_config(path, "CLI flags config").map(Some);
    }
    if let Some(path) = env_explicit {
        return validate_explicit_config(path, CONFIG_ENV).map(Some);
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
    candidates.extend(source_candidates);

    Ok(candidates
        .into_iter()
        .find_map(|candidate| trusted_regular_file(&candidate)))
}

fn validate_explicit_config(path: PathBuf, source: &str) -> Result<PathBuf, String> {
    if !path.is_absolute() {
        return Err(format!("{source} must be an absolute path"));
    }
    trusted_regular_file(&path)
        .ok_or_else(|| format!("{source} does not name a readable regular file"))
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

fn cli_config_path(argv: &[String]) -> Result<Option<PathBuf>, String> {
    let mut args = argv.iter().skip(1).peekable();
    while let Some(arg) = args.next() {
        if arg == "--" {
            return Ok(None);
        }
        if let Some(path) = arg
            .strip_prefix("--cli-flags-config=")
            .or_else(|| arg.strip_prefix("--flags2env-config="))
            .or_else(|| arg.strip_prefix("--config="))
        {
            return explicit_cli_path(path).map(Some);
        }
        if matches!(
            arg.as_str(),
            "--cli-flags-config" | "--flags2env-config" | "--config"
        ) {
            let path = args
                .next()
                .ok_or_else(|| "CLI flags config requires a path".to_string())?;
            return explicit_cli_path(path).map(Some);
        }
    }
    Ok(None)
}

fn explicit_cli_path(path: &str) -> Result<PathBuf, String> {
    let path = path.trim();
    if path.is_empty() {
        Err("CLI flags config requires a path".to_string())
    } else {
        Ok(PathBuf::from(path))
    }
}

fn map_value(values: &HashMap<String, String>, key: &str) -> Option<String> {
    values
        .get(key)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn has_non_empty(values: &HashMap<String, String>, key: &str) -> bool {
    map_value(values, key).is_some()
}

fn has_cli_flags(argv: &[String]) -> bool {
    argv.iter()
        .skip(1)
        .any(|arg| arg.starts_with('-') && arg != "-")
}

fn has_help_flag(argv: &[String]) -> bool {
    argv.iter().skip(1).any(|arg| arg == "--help")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };

    const SOURCE: &str = include_str!("runtime_config.rs");

    struct TestTree(PathBuf);

    impl TestTree {
        fn new(name: &str) -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock")
                .as_nanos();
            let path = env::temp_dir().join(format!(
                "mip-solver-runtime-config-{name}-{}-{nonce}",
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
allow_unknown = false

[flags.port]
env = "PORT"
aliases = ["port"]
type = "integer"
"#,
        )
        .expect("write contract");
    }

    fn config_from_parser_result(
        env_values: HashMap<String, String>,
        parsed: HashMap<String, String>,
    ) -> RuntimeConfig {
        match validated_cli_values(parsed) {
            Ok(cli_values) => RuntimeConfig::from_parts(
                env_values,
                cli_values,
                None,
                CliFlagSource::LinkedParser,
                false,
                None,
                Vec::new(),
                None,
            ),
            Err(error) => RuntimeConfig::from_parts(
                env_values,
                HashMap::new(),
                None,
                CliFlagSource::LinkedParser,
                false,
                None,
                Vec::new(),
                Some(error),
            ),
        }
    }

    #[test]
    fn cli_values_override_environment_values() {
        let config = RuntimeConfig::from_parts(
            HashMap::from([("PORT".to_string(), "8097".to_string())]),
            HashMap::from([("PORT".to_string(), "9090".to_string())]),
            None,
            CliFlagSource::LinkedParser,
            false,
            None,
            Vec::new(),
            None,
        );

        assert_eq!(config.value("PORT", "3000"), "9090");
        assert_eq!(config.usize_value("PORT", 3000), 9090);
        assert!(config.fatal_error().is_none());
    }

    #[test]
    fn linked_parser_loads_cli_flags_from_toml() {
        let config = RuntimeConfig::load(vec![
            COMMAND_NAME.to_string(),
            "--port".to_string(),
            "9191".to_string(),
            "--role=slave".to_string(),
            "--max-http-body-bytes=33554432".to_string(),
            "--max-cut-rounds=12".to_string(),
            "--verbose".to_string(),
        ]);

        assert!(config.fatal_error().is_none());
        assert_eq!(config.cli_flag_source, CliFlagSource::LinkedParser);
        assert_eq!(config.value("PORT", "8097"), "9191");
        assert_eq!(config.value("MIP_SOLVER_NODE_ROLE", "master"), "slave");
        assert_eq!(
            config.value("MIP_SOLVER_MAX_HTTP_BODY_BYTES", "0"),
            "33554432"
        );
        assert_eq!(config.value("MIP_SOLVER_MAX_CUT_ROUNDS", "0"), "12");
        assert_eq!(config.value("MIP_SOLVER_VERBOSE", "false"), "true");
    }

    #[test]
    fn linked_parser_generates_help_table() {
        let config = RuntimeConfig::load(vec![COMMAND_NAME.to_string(), "--help".to_string()]);

        assert!(config.fatal_error().is_none());
        assert!(config.help_requested());
        let help = config.help_table().expect("help table");
        assert!(help.contains("--port"));
        assert!(help.contains("MIP_SOLVER_NODE_ROLE"));
        assert!(config.cli_values.is_empty());
    }

    #[test]
    fn cli_config_path_overrides_default_discovery() {
        let cli_flags_path = cli_flags_path();
        let canonical = cli_flags_path
            .canonicalize()
            .expect("canonical CLI flags path");
        let config = RuntimeConfig::load(vec![
            COMMAND_NAME.to_string(),
            "--cli-flags-config".to_string(),
            cli_flags_path.display().to_string(),
            "--port=9292".to_string(),
        ]);

        assert!(config.fatal_error().is_none());
        assert_eq!(config.config_path.as_deref(), Some(canonical.as_path()));
        assert_eq!(config.value("PORT", "8097"), "9292");
    }

    #[test]
    fn invalid_typed_cli_values_are_fatal_without_echoing_values() {
        let rejected_value = "not-a-number-runtime-secret";
        let config = config_from_parser_result(
            HashMap::new(),
            HashMap::from([(
                "MIP_SOLVER_CLI_PARSE_ERRORS".to_string(),
                rejected_value.to_string(),
            )]),
        );

        assert_eq!(
            config.fatal_error(),
            Some("flags2env rejected one or more CLI values")
        );
        assert!(config.cli_values().is_empty());
        assert!(!config
            .fatal_error()
            .expect("fatal error")
            .contains(rejected_value));
    }

    #[test]
    fn unknown_cli_flags_are_fatal_without_echoing_values() {
        let rejected_value = "postgres://runtime-secret@redacted.invalid/mip";
        let config = config_from_parser_result(
            HashMap::new(),
            HashMap::from([(
                "MIP_SOLVER_UNKNOWN_CLI_FLAGS".to_string(),
                rejected_value.to_string(),
            )]),
        );

        assert_eq!(
            config.fatal_error(),
            Some("flags2env rejected one or more unknown CLI options")
        );
        assert!(config.cli_values().is_empty());
        assert!(!config
            .fatal_error()
            .expect("fatal error")
            .contains(rejected_value));
    }

    #[test]
    fn parse_errors_win_without_merging_any_cli_values() {
        let config = config_from_parser_result(
            HashMap::from([("PORT".to_string(), "8097".to_string())]),
            HashMap::from([
                ("PORT".to_string(), "9292".to_string()),
                (
                    "MIP_SOLVER_CLI_PARSE_ERRORS".to_string(),
                    "caller supplied text".to_string(),
                ),
            ]),
        );

        assert_eq!(
            config.fatal_error(),
            Some("flags2env rejected one or more CLI values")
        );
        assert_eq!(config.value("PORT", "3000"), "8097");
        assert!(config.cli_values().is_empty());
    }

    #[test]
    fn typed_values_fall_back_when_environment_values_are_invalid() {
        let config = RuntimeConfig::from_parts(
            HashMap::from([
                ("COUNT".to_string(), "0".to_string()),
                ("DEBUG".to_string(), "maybe".to_string()),
            ]),
            HashMap::new(),
            None,
            CliFlagSource::EnvOnly,
            false,
            None,
            Vec::new(),
            None,
        );

        assert_eq!(config.u64_value("COUNT", 7), 7);
        assert_eq!(config.usize_value_allow_zero("COUNT", 7), 0);
        assert!(config.bool_value("DEBUG", true));
    }

    #[test]
    fn explicit_contract_requires_an_absolute_readable_regular_file() {
        let tree = TestTree::new("explicit");
        let explicit = tree.path().join("operator/reviewed.toml");
        write_contract(&explicit);

        let resolved =
            discover_config_path_from(Some(explicit.clone()), None, None, Vec::new())
                .expect("explicit contract")
                .expect("resolved explicit contract");
        assert_eq!(
            resolved,
            explicit.canonicalize().expect("canonical explicit contract")
        );

        let relative = discover_config_path_from(
            Some(PathBuf::from("reviewed.toml")),
            None,
            None,
            Vec::new(),
        )
        .expect_err("relative explicit contract must fail closed");
        assert_eq!(relative, "CLI flags config must be an absolute path");
        assert!(!relative.contains("reviewed.toml"));

        let missing = tree.path().join("operator/missing-runtime-secret.toml");
        let error =
            discover_config_path_from(None, Some(missing.clone()), None, Vec::new())
                .expect_err("missing explicit contract must fail closed");
        assert_eq!(
            error,
            "FLAGS2ENV_CONFIG does not name a readable regular file"
        );
        assert!(!error.contains(&missing.display().to_string()));
        assert!(!error.contains("runtime-secret"));
    }

    #[test]
    fn cli_selector_wins_and_fails_closed_instead_of_falling_back_to_env() {
        let tree = TestTree::new("selector-precedence");
        let env_contract = tree.path().join("operator/env.toml");
        write_contract(&env_contract);

        let error = discover_config_path_from(
            Some(PathBuf::from("relative.toml")),
            Some(env_contract),
            None,
            Vec::new(),
        )
        .expect_err("invalid CLI selector must not fall through");
        assert_eq!(error, "CLI flags config must be an absolute path");
    }

    #[test]
    fn packaged_share_contract_beats_colocated_contract() {
        let tree = TestTree::new("package-order");
        let executable = tree.path().join("install/bin/dd-in-house-mip-solver-node");
        let packaged = tree.path().join(
            "install/share/dd-in-house-mip-solver-node/.cli-flags.toml",
        );
        let colocated = tree.path().join("install/bin/.cli-flags.toml");
        write_contract(&packaged);
        write_contract(&colocated);

        let resolved =
            discover_config_path_from(None, None, Some(executable), Vec::new())
                .expect("trusted candidates")
                .expect("packaged contract");
        assert_eq!(
            resolved,
            packaged.canonicalize().expect("canonical packaged contract")
        );
    }

    #[test]
    fn unrelated_working_directory_contract_is_never_a_candidate() {
        let tree = TestTree::new("hostile-cwd");
        let attacker_contract = tree.path().join("attacker/.cli-flags.toml");
        let executable = tree.path().join("install/bin/dd-in-house-mip-solver-node");
        let packaged = tree.path().join(
            "install/share/dd-in-house-mip-solver-node/.cli-flags.toml",
        );
        write_contract(&attacker_contract);
        write_contract(&packaged);

        let resolved =
            discover_config_path_from(None, None, Some(executable), Vec::new())
                .expect("trusted candidates")
                .expect("packaged contract");
        assert_ne!(
            resolved,
            attacker_contract
                .canonicalize()
                .expect("canonical attacker contract")
        );
        assert_eq!(
            resolved,
            packaged.canonicalize().expect("canonical packaged contract")
        );
    }

    #[test]
    fn compile_time_local_wrapper_can_name_the_reviewed_source_contract() {
        let tree = TestTree::new("source-wrapper");
        let root_contract = tree.path().join("source/.cli-flags.toml");
        write_contract(&root_contract);

        let resolved = discover_config_path_from(
            None,
            None,
            Some(tree.path().join("target/debug/dd-in-house-mip-solver-node")),
            vec![
                tree.path().join("source/local/.cli-flags.toml"),
                root_contract.clone(),
            ],
        )
        .expect("trusted candidates")
        .expect("source contract");
        assert_eq!(
            resolved,
            root_contract
                .canonicalize()
                .expect("canonical source contract")
        );
    }

    #[test]
    fn missing_cli_config_value_fails_closed_without_consuming_solver_flags() {
        let error = cli_config_path(&[
            COMMAND_NAME.to_string(),
            "--cli-flags-config".to_string(),
        ])
        .expect_err("missing config path");
        assert_eq!(error, "CLI flags config requires a path");
    }

    #[test]
    fn no_parser_request_can_use_environment_only_mode_without_a_contract() {
        let config = RuntimeConfig::from_parts(
            HashMap::from([("PORT".to_string(), "8097".to_string())]),
            HashMap::new(),
            None,
            CliFlagSource::EnvOnly,
            false,
            None,
            Vec::new(),
            None,
        );
        assert!(config.fatal_error().is_none());
        assert_eq!(config.value("PORT", "3000"), "8097");
    }

    #[test]
    fn production_flags_runtime_has_no_ambient_code_or_contract_fallbacks() {
        for forbidden in [
            concat!("current_", "dir("),
            concat!("Library", "::new"),
            concat!("Command", "::new"),
            concat!("parse_with_native_", "library"),
            concat!("parse_with_cli_", "binary"),
            concat!("FLAGS2ENV_NATIVE_", "LIB"),
            concat!("FLAGS2ENV_", "BIN"),
        ] {
            assert!(
                !SOURCE.contains(forbidden),
                "runtime_config.rs contains forbidden flags fallback: {forbidden}"
            );
        }
    }

    fn cli_flags_path() -> PathBuf {
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let direct = manifest_dir.join(CONFIG_FILE);
        if direct.is_file() {
            return direct;
        }
        manifest_dir
            .parent()
            .expect("local wrapper manifest parent")
            .join(CONFIG_FILE)
    }
}
