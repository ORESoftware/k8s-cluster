use std::{
    collections::HashMap,
    env,
    ffi::{CStr, CString},
    os::raw::{c_char, c_int},
    path::{Path, PathBuf},
    process::Command,
    sync::OnceLock,
};

use libloading::{Library, Symbol};

type ParseJsonArgvFromFile = unsafe extern "C" fn(*const c_char, *const c_char) -> *mut c_char;
type FreeValue = unsafe extern "C" fn(*mut c_char);

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
    NativeLibrary,
    CliBinary,
    EnvOnly,
}

#[derive(Clone, Debug)]
pub struct RuntimeConfig {
    values: HashMap<String, String>,
    cli_values: HashMap<String, String>,
    config_path: Option<PathBuf>,
    cli_flag_source: CliFlagSource,
    help_table: Option<String>,
    warnings: Vec<String>,
}

impl RuntimeConfig {
    pub fn load(argv: Vec<String>) -> Self {
        let env_values: HashMap<String, String> = env::vars().collect();
        let config_path = discover_config_path(&env_values, &argv);
        let should_report_parser_failure = has_cli_flags(&argv)
            || has_non_empty(&env_values, "FLAGS2ENV_CONFIG")
            || has_non_empty(&env_values, "FLAGS2ENV_NATIVE_LIB")
            || has_non_empty(&env_values, "FLAGS2ENV_BIN");
        let mut warnings = Vec::new();
        let mut help_table = None;

        if let Some(path) = config_path.as_deref() {
            match help_table_from_linked_parser(&argv, path) {
                Ok(table) => help_table = table,
                Err(error) if has_help_flag(&argv) => {
                    warnings.push(format!("flags2env help table unavailable: {error}"));
                }
                Err(_) => {}
            }
        }

        let (cli_values, cli_flag_source) = if help_table.is_some() {
            (HashMap::new(), CliFlagSource::LinkedParser)
        } else {
            match config_path.as_deref() {
                Some(path) => match parse_with_linked_parser(&argv, path) {
                    Ok(values) => (values, CliFlagSource::LinkedParser),
                    Err(linked_error) => {
                        match parse_with_native_library(&argv, path, &env_values) {
                            Ok(values) => (values, CliFlagSource::NativeLibrary),
                            Err(native_error) => {
                                match parse_with_cli_binary(&argv, path, &env_values) {
                                    Ok(values) => (values, CliFlagSource::CliBinary),
                                    Err(cli_error) => {
                                        if should_report_parser_failure {
                                            warnings.push(format!(
                                                "flags2env unavailable; CLI flags were not applied ({linked_error}; {native_error}; {cli_error})"
                                            ));
                                        }
                                        (HashMap::new(), CliFlagSource::EnvOnly)
                                    }
                                }
                            }
                        }
                    }
                },
                None => {
                    if should_report_parser_failure {
                        warnings.push(
                            "flags2env config not found; CLI flags were not applied".to_string(),
                        );
                    }
                    (HashMap::new(), CliFlagSource::EnvOnly)
                }
            }
        };

        if let Some(errors) = cli_values.get("MIP_SOLVER_CLI_PARSE_ERRORS") {
            warnings.push(format!("flags2env parse errors: {errors}"));
        }
        if let Some(unknown) = cli_values.get("MIP_SOLVER_UNKNOWN_CLI_FLAGS") {
            warnings.push(format!("flags2env unknown CLI flags: {unknown}"));
        }

        Self::from_parts(
            env_values,
            cli_values,
            config_path,
            cli_flag_source,
            help_table,
            warnings,
        )
    }

    fn from_parts(
        env_values: HashMap<String, String>,
        cli_values: HashMap<String, String>,
        config_path: Option<PathBuf>,
        cli_flag_source: CliFlagSource,
        help_table: Option<String>,
        warnings: Vec<String>,
    ) -> Self {
        let mut values = env_values;
        values.extend(cli_values.clone());
        RuntimeConfig {
            values,
            cli_values,
            config_path,
            cli_flag_source,
            help_table,
            warnings,
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
        self.help_table.is_some()
    }

    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }
}

pub fn initialize() -> &'static RuntimeConfig {
    RUNTIME_CONFIG.get_or_init(|| RuntimeConfig::load(env::args().collect()))
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
    let command_name = CString::new(
        argv.first()
            .map(String::as_str)
            .unwrap_or("dd-in-house-mip-solver-node"),
    )
    .map_err(|error| error.to_string())?;

    let table = unsafe {
        let result = f2e_help_table_from_file(config_path.as_ptr(), command_name.as_ptr(), 0);
        if result.is_null() {
            return Err("native help table returned null".to_string());
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
            return Ok(HashMap::new());
        }
        let raw = CStr::from_ptr(result).to_string_lossy().to_string();
        f2e_free(result);
        raw
    };

    serde_json::from_str(&parsed).map_err(|error| error.to_string())
}

fn parse_with_native_library(
    argv: &[String],
    config_path: &Path,
    env_values: &HashMap<String, String>,
) -> Result<HashMap<String, String>, String> {
    let argv_json = CString::new(serde_json::to_string(argv).map_err(|error| error.to_string())?)
        .map_err(|error| error.to_string())?;
    let config_path = CString::new(config_path.to_string_lossy().as_bytes())
        .map_err(|error| error.to_string())?;
    let candidates = native_library_candidates(env_values);
    let mut errors = Vec::new();

    for candidate in candidates {
        match unsafe { Library::new(&candidate) } {
            Ok(library) => {
                let parsed = unsafe {
                    let parse: Symbol<ParseJsonArgvFromFile> = library
                        .get(b"f2e_parse_json_argv_from_file")
                        .map_err(|error| error.to_string())?;
                    let free: Symbol<FreeValue> = library
                        .get(b"f2e_free")
                        .map_err(|error| error.to_string())?;
                    let result = parse(config_path.as_ptr(), argv_json.as_ptr());
                    if result.is_null() {
                        return Ok(HashMap::new());
                    }
                    let raw = CStr::from_ptr(result).to_string_lossy().to_string();
                    free(result);
                    raw
                };
                return serde_json::from_str(&parsed).map_err(|error| error.to_string());
            }
            Err(error) => errors.push(format!("{candidate}: {error}")),
        }
    }

    Err(if errors.is_empty() {
        "no native library candidates configured".to_string()
    } else {
        errors.join("; ")
    })
}

fn parse_with_cli_binary(
    argv: &[String],
    config_path: &Path,
    env_values: &HashMap<String, String>,
) -> Result<HashMap<String, String>, String> {
    let binary = map_value(env_values, "FLAGS2ENV_BIN").unwrap_or_else(|| "flags2env".to_string());
    let output = Command::new(&binary)
        .arg("shell-env")
        .arg("--config")
        .arg(config_path)
        .arg("--")
        .args(argv)
        .output()
        .map_err(|error| format!("{binary}: {error}"))?;

    if !output.status.success() {
        return Err(format!(
            "{binary} exited with status {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    parse_shell_exports(&String::from_utf8_lossy(&output.stdout))
}

fn parse_shell_exports(output: &str) -> Result<HashMap<String, String>, String> {
    let mut values = HashMap::new();
    for line in output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        let Some(rest) = line.strip_prefix("export ") else {
            continue;
        };
        let Some((key, raw_value)) = rest.split_once('=') else {
            return Err(format!("invalid shell export line: {line}"));
        };
        let value = parse_single_quoted_value(raw_value)
            .ok_or_else(|| format!("invalid shell export value for {key}"))?;
        values.insert(key.to_string(), value);
    }
    Ok(values)
}

fn parse_single_quoted_value(raw: &str) -> Option<String> {
    let mut rest = raw.trim();
    let mut value = String::new();

    loop {
        rest = rest.strip_prefix('\'')?;
        let end = rest.find('\'')?;
        value.push_str(&rest[..end]);
        rest = &rest[end + 1..];
        if rest.is_empty() {
            return Some(value);
        }
        if let Some(next) = rest.strip_prefix("\\''") {
            value.push('\'');
            rest = next;
            continue;
        }
        return None;
    }
}

fn discover_config_path(env_values: &HashMap<String, String>, argv: &[String]) -> Option<PathBuf> {
    if let Some(path) = cli_config_path(argv) {
        if path.is_file() {
            return Some(path);
        }
    }

    if let Some(path) = map_value(env_values, "FLAGS2ENV_CONFIG").map(PathBuf::from) {
        if path.is_file() {
            return Some(path);
        }
    }

    let mut candidates = Vec::new();
    if let Ok(current_dir) = env::current_dir() {
        candidates.push(current_dir.join(".cli-flags.toml"));
    }

    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    candidates.push(manifest_dir.join(".cli-flags.toml"));
    if let Some(parent) = manifest_dir.parent() {
        candidates.push(parent.join(".cli-flags.toml"));
    }

    candidates.into_iter().find(|path| path.is_file())
}

fn cli_config_path(argv: &[String]) -> Option<PathBuf> {
    let mut args = argv.iter().skip(1).peekable();
    while let Some(arg) = args.next() {
        if arg == "--" {
            return None;
        }
        if let Some(path) = arg
            .strip_prefix("--cli-flags-config=")
            .or_else(|| arg.strip_prefix("--flags2env-config="))
            .or_else(|| arg.strip_prefix("--config="))
        {
            return non_empty_path(path);
        }
        if matches!(
            arg.as_str(),
            "--cli-flags-config" | "--flags2env-config" | "--config"
        ) {
            return args.next().and_then(|path| non_empty_path(path));
        }
    }
    None
}

fn non_empty_path(path: &str) -> Option<PathBuf> {
    let path = path.trim();
    if path.is_empty() {
        None
    } else {
        Some(PathBuf::from(path))
    }
}

fn native_library_candidates(env_values: &HashMap<String, String>) -> Vec<String> {
    let mut candidates = Vec::new();
    if let Some(path) = map_value(env_values, "FLAGS2ENV_NATIVE_LIB") {
        candidates.push(path);
    }

    if let Ok(current_dir) = env::current_dir() {
        let lib_name = default_library_name();
        candidates.push(
            current_dir
                .join("build")
                .join(lib_name)
                .to_string_lossy()
                .to_string(),
        );
        candidates.push(current_dir.join(lib_name).to_string_lossy().to_string());
    }
    candidates.push(default_library_name().to_string());
    dedupe(candidates)
}

fn default_library_name() -> &'static str {
    if cfg!(target_os = "macos") {
        "libflags2env.dylib"
    } else if cfg!(target_os = "windows") {
        "flags2env.dll"
    } else {
        "libflags2env.so"
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

fn dedupe(values: Vec<String>) -> Vec<String> {
    let mut deduped = Vec::new();
    for value in values {
        if !deduped.contains(&value) {
            deduped.push(value);
        }
    }
    deduped
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_values_override_environment_values() {
        let config = RuntimeConfig::from_parts(
            HashMap::from([("PORT".to_string(), "8097".to_string())]),
            HashMap::from([("PORT".to_string(), "9090".to_string())]),
            None,
            CliFlagSource::CliBinary,
            None,
            Vec::new(),
        );

        assert_eq!(config.value("PORT", "3000"), "9090");
        assert_eq!(config.usize_value("PORT", 3000), 9090);
    }

    #[test]
    fn linked_parser_loads_cli_flags_from_toml() {
        let config = RuntimeConfig::load(vec![
            "dd-in-house-mip-solver-node".to_string(),
            "--port".to_string(),
            "9191".to_string(),
            "--role=slave".to_string(),
            "--max-http-body-bytes=33554432".to_string(),
            "--max-cut-rounds=12".to_string(),
            "--verbose".to_string(),
        ]);

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
        let config = RuntimeConfig::load(vec![
            "dd-in-house-mip-solver-node".to_string(),
            "--help".to_string(),
        ]);

        assert!(config.help_requested());
        let help = config.help_table().expect("help table");
        assert!(help.contains("--port"));
        assert!(help.contains("MIP_SOLVER_NODE_ROLE"));
        assert!(config.cli_values.is_empty());
    }

    #[test]
    fn cli_config_path_overrides_default_discovery() {
        let cli_flags_path = cli_flags_path();
        let config = RuntimeConfig::load(vec![
            "dd-in-house-mip-solver-node".to_string(),
            "--cli-flags-config".to_string(),
            cli_flags_path.display().to_string(),
            "--port=9292".to_string(),
        ]);

        assert_eq!(
            config.config_path.as_deref(),
            Some(cli_flags_path.as_path())
        );
        assert_eq!(config.value("PORT", "8097"), "9292");
    }

    #[test]
    fn invalid_typed_cli_values_are_reported() {
        let config = RuntimeConfig::load(vec![
            "dd-in-house-mip-solver-node".to_string(),
            "--max-nodes=not-a-number".to_string(),
        ]);

        assert!(config.optional_value("MIP_SOLVER_MAX_NODES").is_none());
        assert!(config
            .warnings
            .iter()
            .any(|warning| warning.contains("parse errors")));
    }

    #[test]
    fn unknown_cli_flags_are_reported() {
        let config = RuntimeConfig::load(vec![
            "dd-in-house-mip-solver-node".to_string(),
            "--definitely-not-a-real-flag".to_string(),
        ]);

        assert!(config
            .warnings
            .iter()
            .any(|warning| warning.contains("unknown CLI flags")));
    }

    #[test]
    fn typed_values_fall_back_when_invalid() {
        let config = RuntimeConfig::from_parts(
            HashMap::from([
                ("COUNT".to_string(), "0".to_string()),
                ("DEBUG".to_string(), "maybe".to_string()),
            ]),
            HashMap::new(),
            None,
            CliFlagSource::EnvOnly,
            None,
            Vec::new(),
        );

        assert_eq!(config.u64_value("COUNT", 7), 7);
        assert_eq!(config.usize_value_allow_zero("COUNT", 7), 0);
        assert!(config.bool_value("DEBUG", true));
    }

    fn cli_flags_path() -> PathBuf {
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let direct = manifest_dir.join(".cli-flags.toml");
        if direct.is_file() {
            return direct;
        }
        manifest_dir
            .parent()
            .expect("manifest parent")
            .join(".cli-flags.toml")
    }
}
