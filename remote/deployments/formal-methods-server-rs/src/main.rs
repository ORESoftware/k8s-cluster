// dd-formal-methods-server
//
// Authenticated Rust HTTP service that ingests a codebase (git repo or inline
// source) and runs formal-methods style analysis over a small language-agnostic
// annotation DSL embedded in source comments. Verification conditions are
// discharged by shelling out to the `z3` SMT solver and the results are
// returned as structured findings with counter-examples where applicable.
//
// The annotation DSL recognised inside line comments (// ... | # ... | -- ...):
//
//     @var name: <Int|Real|Bool>          -- declare a logical variable
//     @assume <expr>                      -- assume the expression unconditionally
//     @requires <expr>                    -- precondition for the next contract
//     @ensures <expr>                     -- postcondition to prove
//     @invariant <expr>                   -- loop invariant to prove (with @variant for progress)
//     @variant <int-expr>                 -- monotonically decreasing termination measure
//     @assert <expr>                      -- ad-hoc property to prove right here
//
// Each contiguous block of these annotations is a "verification unit". The
// service emits one SMT query per @ensures / @assert / @invariant goal:
//
//     (and <requires...> <assume...>) AND (not <goal>)
//
// If Z3 reports sat the postcondition is falsifiable and the counterexample
// model is returned as the bug. If unsat the postcondition follows by
// deduction from the assumptions. If unknown the result is reported as
// undetermined.
//
// In addition to the explicit annotation system the service performs a small
// suite of automatic heuristic checks that do not require any annotations:
//
//   - tautology / contradiction detection on `if (cond)` lines that only
//     reference variables declared in the current @var scope.
//   - dead nested branch detection: the conjunction of outer and inner
//     `if (...)` path conditions is checked for satisfiability.
//   - unsatisfiable @requires: if the conjunction of preconditions for a
//     contract is itself unsat the function is unreachable as specified.

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    env,
    net::SocketAddr,
    path::{Component, Path, PathBuf},
    process::Stdio,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use axum::{
    body::Bytes,
    extract::{Path as AxumPath, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use hmac::{Hmac, Mac};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::Sha256;
use tokio::{
    fs::{self, OpenOptions},
    io::AsyncWriteExt,
    process::Command,
    sync::{RwLock, Semaphore},
    time::timeout,
};
use walkdir::WalkDir;

const SERVICE_NAME: &str = "dd-formal-methods-server";
const DEFAULT_PORT: u16 = 8110;
const SCHEMA_VERSION: &str = "formal-methods.v1";

// ---------------------------------------------------------------------------
// configuration & shared state
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct AppState {
    config: Arc<Config>,
    http: reqwest::Client,
    jobs: Arc<RwLock<HashMap<String, JobRecord>>>,
    semaphore: Arc<Semaphore>,
    counters: Arc<Counters>,
}

#[derive(Clone)]
struct Config {
    work_root: PathBuf,
    git_bin: String,
    z3_bin: String,
    allowed_repo_prefixes: Vec<String>,
    allowed_extensions: HashSet<String>,
    job_timeout: Duration,
    z3_timeout: Duration,
    max_log_bytes: u64,
    max_jobs: usize,
    max_files: usize,
    max_file_bytes: u64,
    max_findings_per_job: usize,
    max_inline_source_bytes: usize,
    server_auth_secret: Option<String>,
    github_webhook_secret: Option<String>,
    github_api_token: Option<String>,
    github_api_base: String,
    pr_diff_only: bool,
    pr_comment_enabled: bool,
    pr_comment_max_rows: usize,
    pr_base_fetch_depth: u64,
}

#[derive(Default)]
struct Counters {
    submitted: AtomicU64,
    running: AtomicU64,
    succeeded: AtomicU64,
    failed: AtomicU64,
    rejected: AtomicU64,
    findings_total: AtomicU64,
    z3_calls: AtomicU64,
    z3_failures: AtomicU64,
    webhooks_received: AtomicU64,
    webhooks_rejected: AtomicU64,
    pr_jobs_queued: AtomicU64,
    pr_comments_posted: AtomicU64,
    pr_comments_failed: AtomicU64,
}

// ---------------------------------------------------------------------------
// HTTP request / response types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct AnalyzeRequest {
    schema_version: Option<String>,
    repo_url: Option<String>,
    git_ref: Option<String>,
    paths: Option<Vec<String>>,
    languages: Option<Vec<String>>,
    inline_source: Option<String>,
    inline_filename: Option<String>,
    heuristics: Option<bool>,
    pull_request: Option<PullRequestRef>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct PullRequestRef {
    owner: String,
    repo: String,
    number: u64,
    head_sha: String,
    base_sha: String,
    head_clone_url: String,
    #[serde(default)]
    head_ref: Option<String>,
    #[serde(default)]
    base_ref: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    html_url: Option<String>,
    #[serde(default)]
    sender: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct JobRecord {
    id: String,
    status: JobStatus,
    request: AnalyzeRequest,
    created_at_ms: u128,
    started_at_ms: Option<u128>,
    finished_at_ms: Option<u128>,
    log_path: String,
    error: Option<String>,
    findings_count: usize,
    findings: Vec<Finding>,
    files_scanned: usize,
    z3_queries: u64,
    pull_request: Option<PullRequestRef>,
    changed_paths: Option<Vec<String>>,
    pr_comment_status: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
enum JobStatus {
    Queued,
    Running,
    Succeeded,
    Failed,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
enum FindingKind {
    PostconditionViolation,
    AssertionViolation,
    UnsatisfiablePrecondition,
    LoopInvariantNotEstablished,
    LoopInvariantNotPreserved,
    LoopVariantNotDecreasing,
    TautologyAlwaysTrue,
    TautologyAlwaysFalse,
    DeadNestedBranch,
    UnsupportedExpression,
    SolverUnknown,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
enum Severity {
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Finding {
    kind: FindingKind,
    severity: Severity,
    file: String,
    line: usize,
    end_line: usize,
    message: String,
    detail: Option<String>,
    goal: Option<String>,
    counterexample: Option<BTreeMap<String, String>>,
    smt_query: Option<String>,
    solver_status: Option<String>,
    reasoning: Option<&'static str>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct HealthResponse {
    ok: bool,
    service: &'static str,
    schema_version: &'static str,
    auth_configured: bool,
    z3_available: bool,
    github_webhook_configured: bool,
    github_comments_enabled: bool,
    pr_diff_only: bool,
    allowed_repo_prefixes: Vec<String>,
    allowed_extensions: Vec<String>,
    queued: usize,
    running: u64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ValidateRequest {
    schema_version: Option<String>,
    source: String,
    filename: Option<String>,
    heuristics: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ValidateResponse {
    schema_version: &'static str,
    findings_count: usize,
    findings: Vec<Finding>,
    z3_queries: u64,
}

// ---------------------------------------------------------------------------
// env / helpers
// ---------------------------------------------------------------------------

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn first_env(keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        env::var(key)
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    })
}

fn env_value(key: &str, fallback: &str) -> String {
    first_env(&[key]).unwrap_or_else(|| fallback.to_string())
}

fn env_u64(key: &str, fallback: u64) -> u64 {
    first_env(&[key])
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(fallback)
}

fn env_usize(key: &str, fallback: usize) -> usize {
    first_env(&[key])
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(fallback)
}

fn parse_csv(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn parse_extensions(value: &str) -> HashSet<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|ext| ext.trim_start_matches('.').to_ascii_lowercase())
        .collect()
}

fn resolve_bin(name: &str) -> String {
    if name.contains('/') {
        return name.to_string();
    }
    let Ok(path) = env::var("PATH") else {
        return name.to_string();
    };
    for dir in path.split(':') {
        if dir.is_empty() {
            continue;
        }
        let candidate = PathBuf::from(dir).join(name);
        if candidate.is_file() {
            return candidate.to_string_lossy().to_string();
        }
    }
    name.to_string()
}

fn env_bool(key: &str, fallback: bool) -> bool {
    first_env(&[key])
        .map(|v| {
            matches!(
                v.as_str(),
                "1" | "true" | "TRUE" | "yes" | "YES" | "on" | "ON"
            )
        })
        .unwrap_or(fallback)
}

fn config_from_env() -> Config {
    let github_api_token = first_env(&["GITHUB_API_TOKEN", "GITHUB_TOKEN"]);
    Config {
        work_root: PathBuf::from(env_value(
            "FORMAL_METHODS_WORK_ROOT",
            "/var/lib/dd-formal-methods-server/jobs",
        )),
        git_bin: resolve_bin(&env_value("FORMAL_METHODS_GIT_BIN", "git")),
        z3_bin: resolve_bin(&env_value("FORMAL_METHODS_Z3_BIN", "z3")),
        allowed_repo_prefixes: parse_csv(&env_value(
            "FORMAL_METHODS_ALLOWED_REPO_PREFIXES",
            "",
        )),
        allowed_extensions: parse_extensions(&env_value(
            "FORMAL_METHODS_ALLOWED_EXTENSIONS",
            "rs,go,ts,tsx,js,jsx,mjs,cjs,py,java,kt,scala,c,h,cc,cpp,hpp,cs,swift,gleam,ex,exs,erl,ml,mli,lua,sh,bash,dart,rb,r",
        )),
        job_timeout: Duration::from_secs(env_u64("FORMAL_METHODS_JOB_TIMEOUT_SECONDS", 900)),
        z3_timeout: Duration::from_secs(env_u64("FORMAL_METHODS_Z3_TIMEOUT_SECONDS", 5)),
        max_log_bytes: env_u64("FORMAL_METHODS_MAX_LOG_BYTES", 4 * 1024 * 1024),
        max_jobs: env_usize("FORMAL_METHODS_MAX_JOBS", 200),
        max_files: env_usize("FORMAL_METHODS_MAX_FILES", 5_000),
        max_file_bytes: env_u64("FORMAL_METHODS_MAX_FILE_BYTES", 512 * 1024),
        max_findings_per_job: env_usize("FORMAL_METHODS_MAX_FINDINGS_PER_JOB", 5_000),
        max_inline_source_bytes: env_usize("FORMAL_METHODS_MAX_INLINE_SOURCE_BYTES", 256 * 1024),
        server_auth_secret: first_env(&["FORMAL_METHODS_AUTH_SECRET", "SERVER_AUTH_SECRET"]),
        github_webhook_secret: first_env(&[
            "FORMAL_METHODS_GITHUB_WEBHOOK_SECRET",
            "GITHUB_WEBHOOK_SECRET",
        ]),
        pr_comment_enabled: env_bool(
            "FORMAL_METHODS_PR_COMMENT_ENABLED",
            github_api_token.is_some(),
        ),
        github_api_token,
        github_api_base: env_value("FORMAL_METHODS_GITHUB_API_BASE", "https://api.github.com"),
        pr_diff_only: env_bool("FORMAL_METHODS_PR_DIFF_ONLY", true),
        pr_comment_max_rows: env_usize("FORMAL_METHODS_PR_COMMENT_MAX_ROWS", 25),
        pr_base_fetch_depth: env_u64("FORMAL_METHODS_PR_BASE_FETCH_DEPTH", 200),
    }
}

fn request_is_authorized(headers: &HeaderMap, secret: &str) -> bool {
    headers
        .get("x-server-auth")
        .or_else(|| headers.get("x-formal-methods-auth"))
        .or_else(|| headers.get("x-agent-auth"))
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value == secret)
}

fn require_auth(headers: &HeaderMap, state: &AppState) -> Result<(), Response> {
    let Some(secret) = state.config.server_auth_secret.as_deref() else {
        state.counters.rejected.fetch_add(1, Ordering::Relaxed);
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "SERVER_AUTH_SECRET is not configured" })),
        )
            .into_response());
    };
    if !request_is_authorized(headers, secret) {
        state.counters.rejected.fetch_add(1, Ordering::Relaxed);
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(json!({
                "error": "unauthorized",
                "errMessage": "missing required formal-methods server auth header",
            })),
        )
            .into_response());
    }
    Ok(())
}

fn clean_optional(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn ensure_allowed_prefix(
    name: &str,
    value: &str,
    prefixes: &[String],
    env_name: &str,
) -> Result<(), String> {
    if prefixes.is_empty() || prefixes.iter().any(|prefix| value.starts_with(prefix)) {
        Ok(())
    } else {
        Err(format!("{name} is not allowed by {env_name}"))
    }
}

fn validate_repo_url(repo_url: &str) -> Result<(), String> {
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

fn validate_relative_path(name: &str, value: &str) -> Result<PathBuf, String> {
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
            Component::Normal(value) => clean.push(value),
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

fn validate_analyze_request(config: &Config, request: &AnalyzeRequest) -> Result<(), String> {
    if let Some(schema_version) = clean_optional(request.schema_version.as_deref()) {
        if schema_version != SCHEMA_VERSION {
            return Err(format!("schemaVersion must be {SCHEMA_VERSION}"));
        }
    }
    let has_repo = clean_optional(request.repo_url.as_deref()).is_some();
    let has_inline = clean_optional(request.inline_source.as_deref()).is_some();
    if !has_repo && !has_inline {
        return Err("either repoUrl or inlineSource is required".to_string());
    }
    if has_repo && has_inline {
        return Err("only one of repoUrl or inlineSource may be set".to_string());
    }
    if has_repo {
        let repo_url = request.repo_url.as_deref().unwrap().trim();
        validate_repo_url(repo_url)?;
        ensure_allowed_prefix(
            "repoUrl",
            repo_url,
            &config.allowed_repo_prefixes,
            "FORMAL_METHODS_ALLOWED_REPO_PREFIXES",
        )?;
        if let Some(git_ref) = clean_optional(request.git_ref.as_deref()) {
            if git_ref.len() > 180 || git_ref.chars().any(|c| c.is_control() || c.is_whitespace()) {
                return Err("gitRef must be a single token of at most 180 chars".to_string());
            }
        }
        if let Some(paths) = request.paths.as_ref() {
            if paths.len() > 64 {
                return Err("paths must contain at most 64 entries".to_string());
            }
            for path in paths {
                validate_relative_path("paths[]", path)?;
            }
        }
    }
    if has_inline {
        let source = request.inline_source.as_deref().unwrap();
        if source.len() > config.max_inline_source_bytes {
            return Err(format!(
                "inlineSource must be {} bytes or fewer",
                config.max_inline_source_bytes
            ));
        }
        if let Some(name) = clean_optional(request.inline_filename.as_deref()) {
            validate_relative_path("inlineFilename", &name)?;
        }
    }
    if let Some(languages) = request.languages.as_ref() {
        if languages.len() > 32 {
            return Err("languages must contain at most 32 entries".to_string());
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// expression DSL: lexer + Pratt parser + SMT-LIB printer
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
enum Token {
    Ident(String),
    Int(String),
    Real(String),
    True,
    False,
    LParen,
    RParen,
    Comma,
    OpOr,
    OpAnd,
    OpNot,
    OpEq,
    OpNeq,
    OpLt,
    OpLe,
    OpGt,
    OpGe,
    OpPlus,
    OpMinus,
    OpStar,
    OpSlash,
    OpPercent,
}

#[derive(Debug, Clone, PartialEq)]
#[allow(dead_code)]
enum SortHint {
    Int,
    Real,
    Bool,
    Unknown,
}

#[derive(Debug, Clone)]
enum Expr {
    Var(String),
    IntLit(String),
    RealLit(String),
    BoolLit(bool),
    Unary(&'static str, Box<Expr>),
    Binary(&'static str, Box<Expr>, Box<Expr>),
    Call(String, Vec<Expr>),
}

fn lex(input: &str) -> Result<Vec<Token>, String> {
    let mut tokens = Vec::new();
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i] as char;
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        if c.is_ascii_alphabetic() || c == '_' {
            let start = i;
            while i < bytes.len() {
                let ch = bytes[i] as char;
                if ch.is_ascii_alphanumeric() || ch == '_' {
                    i += 1;
                } else {
                    break;
                }
            }
            let ident = &input[start..i];
            tokens.push(match ident {
                "true" => Token::True,
                "false" => Token::False,
                "and" => Token::OpAnd,
                "or" => Token::OpOr,
                "not" => Token::OpNot,
                _ => Token::Ident(ident.to_string()),
            });
            continue;
        }
        if c.is_ascii_digit() {
            let start = i;
            let mut saw_dot = false;
            while i < bytes.len() {
                let ch = bytes[i] as char;
                if ch.is_ascii_digit() {
                    i += 1;
                } else if ch == '.' && !saw_dot {
                    saw_dot = true;
                    i += 1;
                } else {
                    break;
                }
            }
            let lit = &input[start..i];
            tokens.push(if saw_dot {
                Token::Real(lit.to_string())
            } else {
                Token::Int(lit.to_string())
            });
            continue;
        }
        let next = bytes.get(i + 1).map(|b| *b as char);
        let pushed = match (c, next) {
            ('=', Some('=')) => {
                i += 2;
                Some(Token::OpEq)
            }
            ('!', Some('=')) => {
                i += 2;
                Some(Token::OpNeq)
            }
            ('<', Some('=')) => {
                i += 2;
                Some(Token::OpLe)
            }
            ('>', Some('=')) => {
                i += 2;
                Some(Token::OpGe)
            }
            ('&', Some('&')) => {
                i += 2;
                Some(Token::OpAnd)
            }
            ('|', Some('|')) => {
                i += 2;
                Some(Token::OpOr)
            }
            _ => None,
        };
        if let Some(tok) = pushed {
            tokens.push(tok);
            continue;
        }
        let tok = match c {
            '(' => Token::LParen,
            ')' => Token::RParen,
            ',' => Token::Comma,
            '<' => Token::OpLt,
            '>' => Token::OpGt,
            '+' => Token::OpPlus,
            '-' => Token::OpMinus,
            '*' => Token::OpStar,
            '/' => Token::OpSlash,
            '%' => Token::OpPercent,
            '!' => Token::OpNot,
            other => return Err(format!("unexpected character {other:?}")),
        };
        i += 1;
        tokens.push(tok);
    }
    Ok(tokens)
}

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    fn bump(&mut self) -> Option<Token> {
        let tok = self.tokens.get(self.pos).cloned();
        if tok.is_some() {
            self.pos += 1;
        }
        tok
    }

    fn parse_expr(&mut self, min_bp: u8) -> Result<Expr, String> {
        let mut lhs = self.parse_unary()?;
        loop {
            let (op, l_bp, r_bp) = match self.peek() {
                Some(Token::OpOr) => ("or", 10, 11),
                Some(Token::OpAnd) => ("and", 20, 21),
                Some(Token::OpEq) => ("=", 30, 31),
                Some(Token::OpNeq) => ("!=", 30, 31),
                Some(Token::OpLt) => ("<", 40, 41),
                Some(Token::OpLe) => ("<=", 40, 41),
                Some(Token::OpGt) => (">", 40, 41),
                Some(Token::OpGe) => (">=", 40, 41),
                Some(Token::OpPlus) => ("+", 50, 51),
                Some(Token::OpMinus) => ("-", 50, 51),
                Some(Token::OpStar) => ("*", 60, 61),
                Some(Token::OpSlash) => ("/", 60, 61),
                Some(Token::OpPercent) => ("mod", 60, 61),
                _ => break,
            };
            if l_bp < min_bp {
                break;
            }
            self.bump();
            let rhs = self.parse_expr(r_bp)?;
            lhs = Expr::Binary(op, Box::new(lhs), Box::new(rhs));
        }
        Ok(lhs)
    }

    fn parse_unary(&mut self) -> Result<Expr, String> {
        match self.peek() {
            Some(Token::OpNot) => {
                self.bump();
                let inner = self.parse_unary()?;
                Ok(Expr::Unary("not", Box::new(inner)))
            }
            Some(Token::OpMinus) => {
                self.bump();
                let inner = self.parse_unary()?;
                Ok(Expr::Unary("-", Box::new(inner)))
            }
            _ => self.parse_atom(),
        }
    }

    fn parse_atom(&mut self) -> Result<Expr, String> {
        let tok = self
            .bump()
            .ok_or_else(|| "unexpected end of expression".to_string())?;
        match tok {
            Token::LParen => {
                let inner = self.parse_expr(0)?;
                match self.bump() {
                    Some(Token::RParen) => Ok(inner),
                    other => Err(format!("expected ')', got {other:?}")),
                }
            }
            Token::Int(value) => Ok(Expr::IntLit(value)),
            Token::Real(value) => Ok(Expr::RealLit(value)),
            Token::True => Ok(Expr::BoolLit(true)),
            Token::False => Ok(Expr::BoolLit(false)),
            Token::Ident(name) => {
                if matches!(self.peek(), Some(Token::LParen)) {
                    self.bump();
                    let mut args = Vec::new();
                    if !matches!(self.peek(), Some(Token::RParen)) {
                        loop {
                            args.push(self.parse_expr(0)?);
                            match self.peek() {
                                Some(Token::Comma) => {
                                    self.bump();
                                }
                                Some(Token::RParen) => break,
                                other => {
                                    return Err(format!(
                                        "expected ',' or ')' in argument list, got {other:?}"
                                    ));
                                }
                            }
                        }
                    }
                    match self.bump() {
                        Some(Token::RParen) => {}
                        other => return Err(format!("expected ')', got {other:?}")),
                    }
                    Ok(Expr::Call(name, args))
                } else {
                    Ok(Expr::Var(name))
                }
            }
            other => Err(format!("unexpected token {other:?}")),
        }
    }
}

fn parse_expr(input: &str) -> Result<Expr, String> {
    let tokens = lex(input)?;
    let mut parser = Parser { tokens, pos: 0 };
    let expr = parser.parse_expr(0)?;
    if parser.pos != parser.tokens.len() {
        return Err(format!(
            "trailing tokens after expression near position {}",
            parser.pos
        ));
    }
    Ok(expr)
}

fn collect_vars(expr: &Expr, out: &mut HashSet<String>) {
    match expr {
        Expr::Var(name) => {
            out.insert(name.clone());
        }
        Expr::Unary(_, inner) => collect_vars(inner, out),
        Expr::Binary(_, lhs, rhs) => {
            collect_vars(lhs, out);
            collect_vars(rhs, out);
        }
        Expr::Call(_, args) => {
            for arg in args {
                collect_vars(arg, out);
            }
        }
        Expr::IntLit(_) | Expr::RealLit(_) | Expr::BoolLit(_) => {}
    }
}

fn expr_to_smt(expr: &Expr) -> Result<String, String> {
    match expr {
        Expr::Var(name) => Ok(name.clone()),
        Expr::IntLit(value) => Ok(value.clone()),
        Expr::RealLit(value) => Ok(value.clone()),
        Expr::BoolLit(value) => Ok(if *value {
            "true".into()
        } else {
            "false".into()
        }),
        Expr::Unary(op, inner) => Ok(format!("({op} {})", expr_to_smt(inner)?)),
        Expr::Binary(op, lhs, rhs) => {
            let lhs_s = expr_to_smt(lhs)?;
            let rhs_s = expr_to_smt(rhs)?;
            let smt_op = match *op {
                "!=" => {
                    return Ok(format!("(not (= {lhs_s} {rhs_s}))"));
                }
                "or" => "or",
                "and" => "and",
                "=" => "=",
                "<" => "<",
                "<=" => "<=",
                ">" => ">",
                ">=" => ">=",
                "+" => "+",
                "-" => "-",
                "*" => "*",
                "/" => "div",
                "mod" => "mod",
                other => return Err(format!("unsupported operator {other:?}")),
            };
            Ok(format!("({smt_op} {lhs_s} {rhs_s})"))
        }
        Expr::Call(name, args) => {
            let lname = name.to_ascii_lowercase();
            let n = args.len();
            match (lname.as_str(), n) {
                ("min", 2) => Ok(format!(
                    "(ite (<= {a} {b}) {a} {b})",
                    a = expr_to_smt(&args[0])?,
                    b = expr_to_smt(&args[1])?
                )),
                ("max", 2) => Ok(format!(
                    "(ite (>= {a} {b}) {a} {b})",
                    a = expr_to_smt(&args[0])?,
                    b = expr_to_smt(&args[1])?
                )),
                ("abs", 1) => {
                    let a = expr_to_smt(&args[0])?;
                    Ok(format!("(ite (>= {a} 0) {a} (- {a}))"))
                }
                _ => Err(format!(
                    "unsupported call {name}/{n}; supported: min(_,_), max(_,_), abs(_)"
                )),
            }
        }
    }
}

// ---------------------------------------------------------------------------
// annotation extraction
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct AnnotationBlock {
    file: String,
    start_line: usize,
    end_line: usize,
    decls: Vec<VarDecl>,
    assumes: Vec<AnnotatedExpr>,
    requires: Vec<AnnotatedExpr>,
    ensures: Vec<AnnotatedExpr>,
    invariants: Vec<AnnotatedExpr>,
    variants: Vec<AnnotatedExpr>,
    asserts: Vec<AnnotatedExpr>,
}

#[derive(Debug, Clone)]
struct VarDecl {
    name: String,
    sort: SortHint,
    #[allow(dead_code)]
    line: usize,
}

#[derive(Debug, Clone)]
struct AnnotatedExpr {
    raw: String,
    line: usize,
}

#[derive(Debug, Clone)]
struct ParsedSource {
    file: String,
    blocks: Vec<AnnotationBlock>,
    plain_lines: Vec<(usize, String)>,
}

fn strip_comment_prefix(line: &str) -> Option<&str> {
    let trimmed = line.trim_start();
    for prefix in ["//", "#", "--", ";;"] {
        if let Some(rest) = trimmed.strip_prefix(prefix) {
            return Some(rest);
        }
    }
    None
}

fn extract_annotation_directive(comment_body: &str) -> Option<(&'static str, &str)> {
    let body = comment_body.trim_start_matches(['/', '*', '!', ' ', '\t']);
    if !body.starts_with('@') {
        return None;
    }
    let rest = &body[1..];
    let directives: &[(&'static str, &'static str)] = &[
        ("var", "var"),
        ("requires", "requires"),
        ("ensures", "ensures"),
        ("assume", "assume"),
        ("invariant", "invariant"),
        ("variant", "variant"),
        ("assert", "assert"),
    ];
    for (kind, kw) in directives {
        if let Some(after) = rest.strip_prefix(kw) {
            if after.is_empty() {
                return Some((kind, ""));
            }
            let first = after.chars().next().unwrap();
            if first.is_whitespace() || first == ':' {
                return Some((kind, after.trim_start_matches([' ', '\t']).trim()));
            }
        }
    }
    None
}

fn parse_var_decl(body: &str, line: usize) -> Result<VarDecl, String> {
    let (name, sort) = match body.split_once(':') {
        Some((name, sort)) => (name.trim(), sort.trim()),
        None => (body.trim(), "Int"),
    };
    if name.is_empty() {
        return Err("missing variable name in @var".to_string());
    }
    if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        || name.chars().next().map_or(true, |c| c.is_ascii_digit())
    {
        return Err(format!("invalid @var name {name:?}"));
    }
    let sort = match sort.to_ascii_lowercase().as_str() {
        "int" | "integer" | "i64" | "i32" | "u64" | "u32" => SortHint::Int,
        "real" | "float" | "double" | "f64" | "f32" => SortHint::Real,
        "bool" | "boolean" => SortHint::Bool,
        other => {
            return Err(format!(
                "unsupported @var sort {other:?} (use Int|Real|Bool)"
            ))
        }
    };
    Ok(VarDecl {
        name: name.to_string(),
        sort,
        line,
    })
}

fn parse_annotations(file: &str, content: &str) -> ParsedSource {
    let mut blocks: Vec<AnnotationBlock> = Vec::new();
    let mut plain_lines: Vec<(usize, String)> = Vec::new();
    let mut current: Option<AnnotationBlock> = None;

    for (idx, line) in content.lines().enumerate() {
        let line_no = idx + 1;
        let comment = strip_comment_prefix(line);
        let directive = comment.and_then(extract_annotation_directive);

        if let Some((kind, body)) = directive {
            let block = current.get_or_insert_with(|| AnnotationBlock {
                file: file.to_string(),
                start_line: line_no,
                end_line: line_no,
                decls: Vec::new(),
                assumes: Vec::new(),
                requires: Vec::new(),
                ensures: Vec::new(),
                invariants: Vec::new(),
                variants: Vec::new(),
                asserts: Vec::new(),
            });
            block.end_line = line_no;
            match kind {
                "var" => match parse_var_decl(body, line_no) {
                    Ok(decl) => block.decls.push(decl),
                    Err(err) => {
                        plain_lines.push((line_no, format!("@var parse error: {err}")));
                    }
                },
                "requires" => block.requires.push(AnnotatedExpr {
                    raw: body.to_string(),
                    line: line_no,
                }),
                "ensures" => block.ensures.push(AnnotatedExpr {
                    raw: body.to_string(),
                    line: line_no,
                }),
                "assume" => block.assumes.push(AnnotatedExpr {
                    raw: body.to_string(),
                    line: line_no,
                }),
                "invariant" => block.invariants.push(AnnotatedExpr {
                    raw: body.to_string(),
                    line: line_no,
                }),
                "variant" => block.variants.push(AnnotatedExpr {
                    raw: body.to_string(),
                    line: line_no,
                }),
                "assert" => block.asserts.push(AnnotatedExpr {
                    raw: body.to_string(),
                    line: line_no,
                }),
                _ => {}
            }
        } else if comment.is_some() {
            // A comment line with no @-directive (a blank `//`, a `// some prose`,
            // a `# ----`, etc.) does NOT close the current annotation block — it
            // is part of the same visual span. Only a non-comment line ends the
            // block.
            if let Some(block) = current.as_mut() {
                block.end_line = line_no;
            } else {
                plain_lines.push((line_no, line.to_string()));
            }
        } else {
            if let Some(block) = current.take() {
                blocks.push(block);
            }
            plain_lines.push((line_no, line.to_string()));
        }
    }
    if let Some(block) = current.take() {
        blocks.push(block);
    }
    ParsedSource {
        file: file.to_string(),
        blocks,
        plain_lines,
    }
}

// ---------------------------------------------------------------------------
// SMT scripting
// ---------------------------------------------------------------------------

fn sort_string(sort: &SortHint) -> &'static str {
    match sort {
        SortHint::Real => "Real",
        SortHint::Bool => "Bool",
        SortHint::Int | SortHint::Unknown => "Int",
    }
}

fn declarations_for(decls: &[VarDecl]) -> String {
    let mut buf = String::new();
    for decl in decls {
        buf.push_str(&format!(
            "(declare-const {} {})\n",
            decl.name,
            sort_string(&decl.sort)
        ));
    }
    buf
}

fn declarations_with_extras(decls: &[VarDecl], extra_vars: &HashSet<String>) -> String {
    let declared: HashSet<&str> = decls.iter().map(|d| d.name.as_str()).collect();
    let mut buf = declarations_for(decls);
    for name in extra_vars {
        if !declared.contains(name.as_str()) {
            buf.push_str(&format!("(declare-const {name} Int)\n"));
        }
    }
    buf
}

#[derive(Debug, Clone)]
struct SmtResult {
    status: SmtStatus,
    model: BTreeMap<String, String>,
    raw: String,
}

#[derive(Debug, Clone, PartialEq)]
enum SmtStatus {
    Sat,
    Unsat,
    Unknown,
    Error,
}

async fn run_z3(config: &Config, script: &str) -> Result<SmtResult, String> {
    let mut child = Command::new(&config.z3_bin)
        .args(["-in", "-smt2", "-T:5"])
        .env_clear()
        .env(
            "PATH",
            "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin",
        )
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|error| format!("failed to spawn {}: {error}", config.z3_bin))?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(script.as_bytes())
            .await
            .map_err(|error| format!("failed to write to z3 stdin: {error}"))?;
        drop(stdin);
    }

    let output = match timeout(config.z3_timeout, child.wait_with_output()).await {
        Ok(Ok(out)) => out,
        Ok(Err(error)) => return Err(format!("z3 wait failed: {error}")),
        Err(_) => return Err(format!("z3 timed out after {:?}", config.z3_timeout)),
    };

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let raw = if stderr.trim().is_empty() {
        stdout.clone()
    } else {
        format!("{stdout}---\n{stderr}")
    };
    let trimmed = stdout.trim();
    let first_line = trimmed.lines().next().unwrap_or("").trim();
    let status = match first_line {
        "sat" => SmtStatus::Sat,
        "unsat" => SmtStatus::Unsat,
        "unknown" => SmtStatus::Unknown,
        _ => SmtStatus::Error,
    };
    let model = if status == SmtStatus::Sat {
        parse_model(trimmed)
    } else {
        BTreeMap::new()
    };
    Ok(SmtResult { status, model, raw })
}

fn parse_model(output: &str) -> BTreeMap<String, String> {
    // Z3 emits `(get-model)` results as one or more `(define-fun NAME () SORT VALUE)`
    // entries, often broken across multiple lines (e.g. `Int\n    0)`). We
    // walk the byte buffer once, locating each define-fun, then read a
    // paren-balanced VALUE until the matching closing paren of the binding.
    let mut model = BTreeMap::new();
    let bytes = output.as_bytes();
    let needle = b"(define-fun ";
    let mut i = 0usize;
    while i + needle.len() <= bytes.len() {
        if &bytes[i..i + needle.len()] != needle {
            i += 1;
            continue;
        }
        let mut j = i + needle.len();
        let name_start = j;
        while j < bytes.len() && !(bytes[j] as char).is_whitespace() {
            j += 1;
        }
        let name = output[name_start..j].to_string();
        while j < bytes.len() && (bytes[j] as char).is_whitespace() {
            j += 1;
        }
        if j + 2 <= bytes.len() && bytes[j] == b'(' && bytes[j + 1] == b')' {
            j += 2;
        }
        while j < bytes.len() && (bytes[j] as char).is_whitespace() {
            j += 1;
        }
        while j < bytes.len() && !(bytes[j] as char).is_whitespace() {
            j += 1;
        }
        while j < bytes.len() && (bytes[j] as char).is_whitespace() {
            j += 1;
        }
        let val_start = j;
        let mut depth: i32 = 0;
        while j < bytes.len() {
            let c = bytes[j];
            if c == b'(' {
                depth += 1;
                j += 1;
            } else if c == b')' {
                if depth == 0 {
                    break;
                }
                depth -= 1;
                j += 1;
            } else {
                j += 1;
            }
        }
        let raw_value = &output[val_start..j];
        let cleaned = raw_value.split_whitespace().collect::<Vec<_>>().join(" ");
        if !name.is_empty() && !cleaned.is_empty() {
            model.insert(name, cleaned);
        }
        i = j;
    }
    model
}

// ---------------------------------------------------------------------------
// verification engine
// ---------------------------------------------------------------------------

struct VerifyContext<'a> {
    config: &'a Config,
}

impl<'a> VerifyContext<'a> {
    fn finding(
        &self,
        kind: FindingKind,
        severity: Severity,
        file: &str,
        line: usize,
        end_line: usize,
        message: String,
        detail: Option<String>,
        goal: Option<String>,
        smt: Option<&SmtResult>,
        smt_query: Option<String>,
        reasoning: &'static str,
    ) -> Finding {
        Finding {
            kind,
            severity,
            file: file.to_string(),
            line,
            end_line,
            message,
            detail,
            goal,
            counterexample: smt.map(|r| r.model.clone()).filter(|m| !m.is_empty()),
            smt_query,
            solver_status: smt.map(|r| smt_status_label(&r.status).to_string()),
            reasoning: Some(reasoning),
        }
    }

    async fn check_unsat(&self, script: &str) -> SmtResult {
        match run_z3(self.config, script).await {
            Ok(result) => result,
            Err(message) => SmtResult {
                status: SmtStatus::Error,
                model: BTreeMap::new(),
                raw: message,
            },
        }
    }
}

fn smt_status_label(status: &SmtStatus) -> &'static str {
    match status {
        SmtStatus::Sat => "sat",
        SmtStatus::Unsat => "unsat",
        SmtStatus::Unknown => "unknown",
        SmtStatus::Error => "error",
    }
}

async fn verify_block(
    ctx: &VerifyContext<'_>,
    block: &AnnotationBlock,
    z3_calls: &AtomicU64,
    z3_failures: &AtomicU64,
) -> Vec<Finding> {
    let mut findings = Vec::new();

    let mut assumption_smt: Vec<String> = Vec::new();
    let mut assumption_vars: HashSet<String> = HashSet::new();

    let mut parsed_assume: Vec<(AnnotatedExpr, Expr)> = Vec::new();
    for ann in block.assumes.iter().chain(block.requires.iter()) {
        match parse_expr(&ann.raw) {
            Ok(expr) => {
                collect_vars(&expr, &mut assumption_vars);
                match expr_to_smt(&expr) {
                    Ok(smt) => {
                        assumption_smt.push(smt);
                        parsed_assume.push((ann.clone(), expr));
                    }
                    Err(err) => {
                        findings.push(Finding {
                            kind: FindingKind::UnsupportedExpression,
                            severity: Severity::Warning,
                            file: block.file.clone(),
                            line: ann.line,
                            end_line: ann.line,
                            message: format!("could not encode assumption: {err}"),
                            detail: Some(ann.raw.clone()),
                            goal: None,
                            counterexample: None,
                            smt_query: None,
                            solver_status: None,
                            reasoning: Some("encoding"),
                        });
                    }
                }
            }
            Err(err) => {
                findings.push(Finding {
                    kind: FindingKind::UnsupportedExpression,
                    severity: Severity::Warning,
                    file: block.file.clone(),
                    line: ann.line,
                    end_line: ann.line,
                    message: format!("could not parse expression: {err}"),
                    detail: Some(ann.raw.clone()),
                    goal: None,
                    counterexample: None,
                    smt_query: None,
                    solver_status: None,
                    reasoning: Some("parser"),
                });
            }
        }
    }

    // unsatisfiable preconditions: the function body is unreachable as specified.
    if !block.requires.is_empty() || !block.assumes.is_empty() {
        let mut script = String::new();
        script.push_str(&declarations_with_extras(&block.decls, &assumption_vars));
        for smt in &assumption_smt {
            script.push_str(&format!("(assert {smt})\n"));
        }
        script.push_str("(check-sat)\n");

        z3_calls.fetch_add(1, Ordering::Relaxed);
        let result = ctx.check_unsat(&script).await;
        if matches!(result.status, SmtStatus::Error) {
            z3_failures.fetch_add(1, Ordering::Relaxed);
        }
        if result.status == SmtStatus::Unsat {
            let last_line = block
                .requires
                .last()
                .or_else(|| block.assumes.last())
                .map(|a| a.line)
                .unwrap_or(block.end_line);
            findings.push(ctx.finding(
                FindingKind::UnsatisfiablePrecondition,
                Severity::Error,
                &block.file,
                block.start_line,
                last_line,
                "the conjunction of @requires/@assume is unsatisfiable; this contract can never be entered".to_string(),
                Some("Z3 proved that no values of the declared variables satisfy all assumptions.".to_string()),
                None,
                Some(&result),
                Some(script),
                "deduction: ⊢ ⊥ from ⋀ requires",
            ));
        }
    }

    // ensures and asserts: try to falsify the goal.
    let mut goal_units: Vec<(
        &'static str,
        FindingKind,
        Severity,
        &AnnotatedExpr,
        &'static str,
    )> = Vec::new();
    for ann in &block.ensures {
        goal_units.push((
            "ensures",
            FindingKind::PostconditionViolation,
            Severity::Error,
            ann,
            "deduction: search for ⋀ assumptions ∧ ¬ ensures",
        ));
    }
    for ann in &block.asserts {
        goal_units.push((
            "assert",
            FindingKind::AssertionViolation,
            Severity::Error,
            ann,
            "deduction: search for ⋀ assumptions ∧ ¬ assert",
        ));
    }

    for (label, kind, severity, ann, reasoning) in goal_units {
        let expr = match parse_expr(&ann.raw) {
            Ok(expr) => expr,
            Err(err) => {
                findings.push(Finding {
                    kind: FindingKind::UnsupportedExpression,
                    severity: Severity::Warning,
                    file: block.file.clone(),
                    line: ann.line,
                    end_line: ann.line,
                    message: format!("could not parse @{label}: {err}"),
                    detail: Some(ann.raw.clone()),
                    goal: None,
                    counterexample: None,
                    smt_query: None,
                    solver_status: None,
                    reasoning: Some("parser"),
                });
                continue;
            }
        };
        let mut vars = assumption_vars.clone();
        collect_vars(&expr, &mut vars);
        let goal_smt = match expr_to_smt(&expr) {
            Ok(smt) => smt,
            Err(err) => {
                findings.push(Finding {
                    kind: FindingKind::UnsupportedExpression,
                    severity: Severity::Warning,
                    file: block.file.clone(),
                    line: ann.line,
                    end_line: ann.line,
                    message: format!("could not encode @{label}: {err}"),
                    detail: Some(ann.raw.clone()),
                    goal: None,
                    counterexample: None,
                    smt_query: None,
                    solver_status: None,
                    reasoning: Some("encoding"),
                });
                continue;
            }
        };
        let mut script = String::new();
        script.push_str(&declarations_with_extras(&block.decls, &vars));
        for smt in &assumption_smt {
            script.push_str(&format!("(assert {smt})\n"));
        }
        script.push_str(&format!("(assert (not {goal_smt}))\n"));
        script.push_str("(check-sat)\n(get-model)\n");

        z3_calls.fetch_add(1, Ordering::Relaxed);
        let result = ctx.check_unsat(&script).await;
        match result.status {
            SmtStatus::Sat => {
                findings.push(ctx.finding(
                    kind.clone(),
                    severity.clone(),
                    &block.file,
                    ann.line,
                    ann.line,
                    format!("@{label} can be violated under the declared assumptions"),
                    Some("Z3 found a model that satisfies all @requires/@assume but falsifies the goal.".to_string()),
                    Some(ann.raw.clone()),
                    Some(&result),
                    Some(script),
                    reasoning,
                ));
            }
            SmtStatus::Unsat => {
                // proved -- intentionally no finding.
            }
            SmtStatus::Unknown => {
                findings.push(
                    ctx.finding(
                        FindingKind::SolverUnknown,
                        Severity::Info,
                        &block.file,
                        ann.line,
                        ann.line,
                        format!("solver returned unknown for @{label}"),
                        Some(
                            "Z3 could not prove or refute this goal within the configured budget."
                                .to_string(),
                        ),
                        Some(ann.raw.clone()),
                        Some(&result),
                        Some(script),
                        reasoning,
                    ),
                );
            }
            SmtStatus::Error => {
                z3_failures.fetch_add(1, Ordering::Relaxed);
                findings.push(ctx.finding(
                    FindingKind::SolverUnknown,
                    Severity::Warning,
                    &block.file,
                    ann.line,
                    ann.line,
                    format!("solver error while checking @{label}"),
                    Some(result.raw.chars().take(300).collect()),
                    Some(ann.raw.clone()),
                    Some(&result),
                    Some(script),
                    reasoning,
                ));
            }
        }
    }

    // loop invariants: prove that requires entails invariant (initialisation).
    for ann in &block.invariants {
        let expr = match parse_expr(&ann.raw) {
            Ok(expr) => expr,
            Err(err) => {
                findings.push(Finding {
                    kind: FindingKind::UnsupportedExpression,
                    severity: Severity::Warning,
                    file: block.file.clone(),
                    line: ann.line,
                    end_line: ann.line,
                    message: format!("could not parse @invariant: {err}"),
                    detail: Some(ann.raw.clone()),
                    goal: None,
                    counterexample: None,
                    smt_query: None,
                    solver_status: None,
                    reasoning: Some("parser"),
                });
                continue;
            }
        };
        let mut vars = assumption_vars.clone();
        collect_vars(&expr, &mut vars);
        let goal_smt = match expr_to_smt(&expr) {
            Ok(smt) => smt,
            Err(err) => {
                findings.push(Finding {
                    kind: FindingKind::UnsupportedExpression,
                    severity: Severity::Warning,
                    file: block.file.clone(),
                    line: ann.line,
                    end_line: ann.line,
                    message: format!("could not encode @invariant: {err}"),
                    detail: Some(ann.raw.clone()),
                    goal: None,
                    counterexample: None,
                    smt_query: None,
                    solver_status: None,
                    reasoning: Some("encoding"),
                });
                continue;
            }
        };
        let mut script = String::new();
        script.push_str(&declarations_with_extras(&block.decls, &vars));
        for smt in &assumption_smt {
            script.push_str(&format!("(assert {smt})\n"));
        }
        script.push_str(&format!("(assert (not {goal_smt}))\n"));
        script.push_str("(check-sat)\n(get-model)\n");

        z3_calls.fetch_add(1, Ordering::Relaxed);
        let result = ctx.check_unsat(&script).await;
        if matches!(result.status, SmtStatus::Error) {
            z3_failures.fetch_add(1, Ordering::Relaxed);
        }
        match result.status {
            SmtStatus::Sat => {
                findings.push(
                    ctx.finding(
                        FindingKind::LoopInvariantNotEstablished,
                        Severity::Error,
                        &block.file,
                        ann.line,
                        ann.line,
                        "loop @invariant does not follow from the preceding @requires/@assume"
                            .to_string(),
                        Some(
                            "Induction base step: the invariant must hold on loop entry."
                                .to_string(),
                        ),
                        Some(ann.raw.clone()),
                        Some(&result),
                        Some(script),
                        "induction: base-step refutation",
                    ),
                );
            }
            SmtStatus::Unknown => {
                findings.push(ctx.finding(
                    FindingKind::SolverUnknown,
                    Severity::Info,
                    &block.file,
                    ann.line,
                    ann.line,
                    "solver returned unknown for @invariant".to_string(),
                    None,
                    Some(ann.raw.clone()),
                    Some(&result),
                    Some(script),
                    "induction: base-step",
                ));
            }
            _ => {}
        }
    }

    // variant must be non-negative under invariants & assumptions; this is a
    // lightweight termination sanity check (full preservation step requires a
    // primed-state encoding which we don't infer from comments).
    for variant in &block.variants {
        let expr = match parse_expr(&variant.raw) {
            Ok(expr) => expr,
            Err(_) => continue,
        };
        let mut vars = assumption_vars.clone();
        collect_vars(&expr, &mut vars);
        let smt = match expr_to_smt(&expr) {
            Ok(smt) => smt,
            Err(_) => continue,
        };
        let mut invariant_smt = Vec::new();
        for inv in &block.invariants {
            if let Ok(parsed) = parse_expr(&inv.raw) {
                collect_vars(&parsed, &mut vars);
                if let Ok(s) = expr_to_smt(&parsed) {
                    invariant_smt.push(s);
                }
            }
        }
        let mut script = String::new();
        script.push_str(&declarations_with_extras(&block.decls, &vars));
        for s in assumption_smt.iter().chain(invariant_smt.iter()) {
            script.push_str(&format!("(assert {s})\n"));
        }
        script.push_str(&format!("(assert (< {smt} 0))\n"));
        script.push_str("(check-sat)\n(get-model)\n");

        z3_calls.fetch_add(1, Ordering::Relaxed);
        let result = ctx.check_unsat(&script).await;
        if result.status == SmtStatus::Sat {
            findings.push(
                ctx.finding(
                    FindingKind::LoopVariantNotDecreasing,
                    Severity::Warning,
                    &block.file,
                    variant.line,
                    variant.line,
                    "@variant can be negative under the declared invariants".to_string(),
                    Some(
                        "Termination measures must remain non-negative on entry to each iteration."
                            .to_string(),
                    ),
                    Some(variant.raw.clone()),
                    Some(&result),
                    Some(script),
                    "induction: termination measure",
                ),
            );
        }
    }

    findings
}

// ---------------------------------------------------------------------------
// heuristic checks over plain source lines (no annotations required)
// ---------------------------------------------------------------------------

fn if_condition_pattern() -> Regex {
    Regex::new(r"\bif\s*\(([^()]+(?:\([^()]*\)[^()]*)*)\)").expect("if regex")
}

fn current_indent(line: &str) -> usize {
    line.chars().take_while(|c| *c == ' ' || *c == '\t').count()
}

async fn heuristic_checks(
    ctx: &VerifyContext<'_>,
    parsed: &ParsedSource,
    decls_lookup: &HashMap<String, SortHint>,
    z3_calls: &AtomicU64,
    z3_failures: &AtomicU64,
) -> Vec<Finding> {
    let mut findings = Vec::new();
    let re = if_condition_pattern();

    // path conditions of currently open if-blocks: (indent, smt_condition, line)
    let mut stack: Vec<(usize, String, usize)> = Vec::new();

    for (line_no, raw_line) in &parsed.plain_lines {
        let line = raw_line.trim_end();
        let indent = current_indent(raw_line);

        while let Some((top_indent, _, _)) = stack.last() {
            if indent <= *top_indent && !line.is_empty() {
                stack.pop();
            } else {
                break;
            }
        }

        let cap = match re.captures(line) {
            Some(c) => c,
            None => continue,
        };
        let cond_raw = cap.get(1).unwrap().as_str().trim();
        if cond_raw.is_empty() {
            continue;
        }

        let expr = match parse_expr(cond_raw) {
            Ok(expr) => expr,
            Err(_) => continue,
        };
        let mut vars = HashSet::new();
        collect_vars(&expr, &mut vars);
        if vars.is_empty() {
            continue;
        }
        if !vars.iter().all(|v| decls_lookup.contains_key(v.as_str())) {
            continue;
        }
        let smt = match expr_to_smt(&expr) {
            Ok(s) => s,
            Err(_) => continue,
        };

        let decls: Vec<VarDecl> = decls_lookup
            .iter()
            .filter(|(name, _)| vars.contains(name.as_str()))
            .map(|(name, sort)| VarDecl {
                name: name.clone(),
                sort: sort.clone(),
                line: *line_no,
            })
            .collect();

        // always-true check: assert (not cond) and look for sat.
        let mut script = String::new();
        script.push_str(&declarations_for(&decls));
        for (_, parent, _) in &stack {
            script.push_str(&format!("(assert {parent})\n"));
        }
        script.push_str(&format!("(assert (not {smt}))\n"));
        script.push_str("(check-sat)\n");
        z3_calls.fetch_add(1, Ordering::Relaxed);
        let r_true = ctx.check_unsat(&script).await;
        if matches!(r_true.status, SmtStatus::Error) {
            z3_failures.fetch_add(1, Ordering::Relaxed);
        }
        let always_true = r_true.status == SmtStatus::Unsat;

        // always-false check: assert cond and look for sat.
        let mut script_false = String::new();
        script_false.push_str(&declarations_for(&decls));
        for (_, parent, _) in &stack {
            script_false.push_str(&format!("(assert {parent})\n"));
        }
        script_false.push_str(&format!("(assert {smt})\n"));
        script_false.push_str("(check-sat)\n");
        z3_calls.fetch_add(1, Ordering::Relaxed);
        let r_false = ctx.check_unsat(&script_false).await;
        if matches!(r_false.status, SmtStatus::Error) {
            z3_failures.fetch_add(1, Ordering::Relaxed);
        }
        let always_false = r_false.status == SmtStatus::Unsat;

        if always_true && !stack.is_empty() {
            findings.push(ctx.finding(
                FindingKind::TautologyAlwaysTrue,
                Severity::Warning,
                &parsed.file,
                *line_no,
                *line_no,
                format!("`if ({cond_raw})` is implied by the surrounding conditions"),
                Some(
                    "All enclosing if-branch conditions imply this one, so the test is redundant."
                        .to_string(),
                ),
                Some(cond_raw.to_string()),
                Some(&r_true),
                Some(script.clone()),
                "deduction: ⋀ outer ⊢ cond",
            ));
        } else if always_true {
            findings.push(ctx.finding(
                FindingKind::TautologyAlwaysTrue,
                Severity::Warning,
                &parsed.file,
                *line_no,
                *line_no,
                format!("`if ({cond_raw})` is always true for declared variables"),
                Some("This condition is a tautology over the declared variable sorts.".to_string()),
                Some(cond_raw.to_string()),
                Some(&r_true),
                Some(script.clone()),
                "deduction: ⊢ cond",
            ));
        } else if always_false && !stack.is_empty() {
            findings.push(ctx.finding(
                FindingKind::DeadNestedBranch,
                Severity::Error,
                &parsed.file,
                *line_no,
                *line_no,
                format!("nested `if ({cond_raw})` is unreachable from outer branch"),
                Some(
                    "The conjunction of outer path conditions contradicts this guard.".to_string(),
                ),
                Some(cond_raw.to_string()),
                Some(&r_false),
                Some(script_false.clone()),
                "deduction: ⋀ outer ∧ cond ⊢ ⊥",
            ));
        } else if always_false {
            findings.push(
                ctx.finding(
                    FindingKind::TautologyAlwaysFalse,
                    Severity::Warning,
                    &parsed.file,
                    *line_no,
                    *line_no,
                    format!("`if ({cond_raw})` is always false for declared variables"),
                    Some(
                        "This condition is a contradiction over the declared variable sorts."
                            .to_string(),
                    ),
                    Some(cond_raw.to_string()),
                    Some(&r_false),
                    Some(script_false.clone()),
                    "deduction: cond ⊢ ⊥",
                ),
            );
        }

        if !always_true && !always_false {
            stack.push((indent, smt, *line_no));
        }
    }

    findings
}

// ---------------------------------------------------------------------------
// scanning the working tree
// ---------------------------------------------------------------------------

fn extension_of(path: &Path) -> Option<String> {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|s| s.to_ascii_lowercase())
}

fn is_source_path(
    config: &Config,
    rel_path: &Path,
    languages_filter: &Option<HashSet<String>>,
) -> bool {
    let Some(ext) = extension_of(rel_path) else {
        return false;
    };
    if !config.allowed_extensions.contains(&ext) {
        return false;
    }
    if let Some(filter) = languages_filter {
        if !filter.contains(&ext) {
            return false;
        }
    }
    true
}

fn matches_paths(rel_path: &Path, filters: &Option<Vec<PathBuf>>) -> bool {
    let Some(filters) = filters else {
        return true;
    };
    if filters.is_empty() {
        return true;
    }
    filters.iter().any(|filter| {
        if filter.as_os_str().is_empty() || filter.as_os_str() == "." {
            return true;
        }
        rel_path.starts_with(filter)
    })
}

async fn analyze_tree(
    state: &AppState,
    root: &Path,
    languages_filter: &Option<HashSet<String>>,
    path_filter: &Option<Vec<PathBuf>>,
    heuristics_enabled: bool,
    log_path: &Path,
    z3_calls: &AtomicU64,
    z3_failures: &AtomicU64,
) -> (Vec<Finding>, usize) {
    let mut findings: Vec<Finding> = Vec::new();
    let mut files_scanned = 0usize;
    let ctx = VerifyContext {
        config: &state.config,
    };

    let walker = WalkDir::new(root)
        .max_depth(20)
        .follow_links(false)
        .into_iter()
        .filter_map(Result::ok);

    for entry in walker {
        if findings.len() >= state.config.max_findings_per_job {
            append_log(
                log_path,
                "max findings reached, stopping early\n",
                state.config.max_log_bytes,
            )
            .await;
            break;
        }
        if files_scanned >= state.config.max_files {
            append_log(
                log_path,
                "max file count reached, stopping early\n",
                state.config.max_log_bytes,
            )
            .await;
            break;
        }
        let path = entry.path();
        if !entry.file_type().is_file() {
            continue;
        }
        let rel = match path.strip_prefix(root) {
            Ok(rel) => rel,
            Err(_) => continue,
        };
        if rel
            .components()
            .any(|c| matches!(c, Component::Normal(name) if name == ".git" || name == "node_modules" || name == "target" || name == "build" || name == "dist" || name == ".venv" || name == "venv"))
        {
            continue;
        }
        if !is_source_path(&state.config, rel, languages_filter) {
            continue;
        }
        if !matches_paths(rel, path_filter) {
            continue;
        }
        let meta = match fs::metadata(path).await {
            Ok(meta) => meta,
            Err(_) => continue,
        };
        if meta.len() > state.config.max_file_bytes {
            continue;
        }
        let content = match fs::read_to_string(path).await {
            Ok(content) => content,
            Err(_) => continue,
        };
        files_scanned += 1;

        let file_label = rel.to_string_lossy().to_string();
        let parsed = parse_annotations(&file_label, &content);

        let mut decls_lookup: HashMap<String, SortHint> = HashMap::new();
        for block in &parsed.blocks {
            for decl in &block.decls {
                decls_lookup.insert(decl.name.clone(), decl.sort.clone());
            }
        }

        for block in &parsed.blocks {
            let mut block_findings = verify_block(&ctx, block, z3_calls, z3_failures).await;
            findings.append(&mut block_findings);
            if findings.len() >= state.config.max_findings_per_job {
                break;
            }
        }
        if findings.len() >= state.config.max_findings_per_job {
            break;
        }
        if heuristics_enabled && !decls_lookup.is_empty() {
            let mut h = heuristic_checks(&ctx, &parsed, &decls_lookup, z3_calls, z3_failures).await;
            findings.append(&mut h);
        }

        append_log(
            log_path,
            &format!(
                "scanned {} ({} blocks, {} decls)\n",
                file_label,
                parsed.blocks.len(),
                decls_lookup.len()
            ),
            state.config.max_log_bytes,
        )
        .await;
    }

    (findings, files_scanned)
}

// ---------------------------------------------------------------------------
// log writing
// ---------------------------------------------------------------------------

async fn append_log(path: &Path, message: &str, max_bytes: u64) {
    let current_len = fs::metadata(path).await.map(|meta| meta.len()).unwrap_or(0);
    if current_len >= max_bytes {
        return;
    }
    let remaining = (max_bytes - current_len) as usize;
    let bytes = message.as_bytes();
    let limit = remaining.min(bytes.len());
    if limit == 0 {
        return;
    }
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent).await;
    }
    if let Ok(mut file) = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .await
    {
        let _ = file.write_all(&bytes[..limit]).await;
    }
}

// ---------------------------------------------------------------------------
// job orchestration
// ---------------------------------------------------------------------------

fn job_id(counter: u64) -> String {
    format!("formal-{}-{counter}", now_ms())
}

async fn update_job<F>(state: &AppState, id: &str, mutate: F)
where
    F: FnOnce(&mut JobRecord),
{
    let mut jobs = state.jobs.write().await;
    if let Some(job) = jobs.get_mut(id) {
        mutate(job);
    }
}

async fn prune_jobs(state: &AppState) {
    let max_jobs = state.config.max_jobs;
    let mut jobs = state.jobs.write().await;
    if jobs.len() <= max_jobs {
        return;
    }
    let mut candidates = jobs
        .values()
        .filter(|job| !matches!(job.status, JobStatus::Queued | JobStatus::Running))
        .map(|job| (job.created_at_ms, job.id.clone()))
        .collect::<Vec<_>>();
    candidates.sort_by_key(|(created_at_ms, _)| *created_at_ms);
    for (_, id) in candidates
        .into_iter()
        .take(jobs.len().saturating_sub(max_jobs))
    {
        jobs.remove(&id);
    }
}

struct GitOutcome {
    status: std::process::ExitStatus,
    stdout: String,
    stderr: String,
}

async fn run_git(
    config: &Config,
    log_path: &Path,
    cwd: &Path,
    args: &[&str],
    timeout_dur: Duration,
    record_in_log: bool,
) -> Result<GitOutcome, String> {
    if record_in_log {
        append_log(
            log_path,
            &format!(
                "$ {} -C {} {}\n",
                config.git_bin,
                cwd.display(),
                args.join(" ")
            ),
            config.max_log_bytes,
        )
        .await;
    }
    let output = match timeout(
        timeout_dur,
        Command::new(&config.git_bin)
            .args(args)
            .current_dir(cwd)
            .env_clear()
            .env("HOME", cwd)
            .env(
                "PATH",
                "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin",
            )
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_ASKPASS", "/bin/false")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .output(),
    )
    .await
    {
        Ok(Ok(out)) => out,
        Ok(Err(error)) => return Err(format!("git failed to spawn: {error}")),
        Err(_) => return Err(format!("git timed out: {args:?}")),
    };
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    if record_in_log {
        if !stdout.is_empty() {
            append_log(log_path, &stdout, config.max_log_bytes).await;
        }
        if !stderr.is_empty() {
            append_log(log_path, &stderr, config.max_log_bytes).await;
        }
    }
    Ok(GitOutcome {
        status: output.status,
        stdout,
        stderr,
    })
}

async fn clone_repo(
    config: &Config,
    log_path: &Path,
    job_dir: &Path,
    repo_url: &str,
    git_ref: Option<&str>,
) -> Result<PathBuf, String> {
    let repo_dir = job_dir.join("repo");
    let mut clone_args: Vec<String> = vec!["clone".into(), "--depth".into(), "1".into()];
    if let Some(git_ref) = git_ref {
        clone_args.push("--branch".into());
        clone_args.push(git_ref.to_string());
    }
    clone_args.push(repo_url.to_string());
    clone_args.push(repo_dir.to_string_lossy().to_string());
    let arg_refs: Vec<&str> = clone_args.iter().map(String::as_str).collect();
    let outcome = run_git(
        config,
        log_path,
        job_dir,
        &arg_refs,
        config.job_timeout,
        true,
    )
    .await?;
    if !outcome.status.success() {
        return Err(format!("git clone exited with status {}", outcome.status));
    }
    Ok(repo_dir)
}

fn is_sha_like(value: &str) -> bool {
    let len = value.len();
    (4..=64).contains(&len) && value.chars().all(|c| c.is_ascii_hexdigit())
}

async fn clone_for_pr(
    config: &Config,
    log_path: &Path,
    job_dir: &Path,
    pr: &PullRequestRef,
) -> Result<(PathBuf, Option<Vec<String>>), String> {
    let repo_dir = job_dir.join("repo");
    fs::create_dir_all(&repo_dir)
        .await
        .map_err(|error| format!("failed to create pr repo dir: {error}"))?;

    if !is_sha_like(&pr.head_sha) {
        return Err("pull_request.head_sha must be a hex SHA".to_string());
    }
    if !is_sha_like(&pr.base_sha) {
        return Err("pull_request.base_sha must be a hex SHA".to_string());
    }
    validate_repo_url(&pr.head_clone_url)?;
    ensure_allowed_prefix(
        "pull_request.head_clone_url",
        &pr.head_clone_url,
        &config.allowed_repo_prefixes,
        "FORMAL_METHODS_ALLOWED_REPO_PREFIXES",
    )?;

    append_log(
        log_path,
        &format!(
            "{SERVICE_NAME} PR {}/{}#{}: head={} base={}\n",
            pr.owner, pr.repo, pr.number, pr.head_sha, pr.base_sha
        ),
        config.max_log_bytes,
    )
    .await;

    let init = run_git(
        config,
        log_path,
        &repo_dir,
        &["init", "-q"],
        Duration::from_secs(30),
        true,
    )
    .await?;
    if !init.status.success() {
        return Err(format!("git init failed: {}", init.stderr));
    }
    let remote = run_git(
        config,
        log_path,
        &repo_dir,
        &["remote", "add", "origin", &pr.head_clone_url],
        Duration::from_secs(15),
        true,
    )
    .await?;
    if !remote.status.success() {
        return Err(format!("git remote add failed: {}", remote.stderr));
    }
    let head_fetch = run_git(
        config,
        log_path,
        &repo_dir,
        &["fetch", "--depth", "1", "origin", &pr.head_sha],
        config.job_timeout,
        true,
    )
    .await?;
    if !head_fetch.status.success() {
        return Err(format!(
            "git fetch <head_sha> failed: {}",
            head_fetch.stderr
        ));
    }
    let checkout = run_git(
        config,
        log_path,
        &repo_dir,
        &["checkout", "-q", &pr.head_sha],
        Duration::from_secs(60),
        true,
    )
    .await?;
    if !checkout.status.success() {
        return Err(format!("git checkout failed: {}", checkout.stderr));
    }

    let depth = config.pr_base_fetch_depth.to_string();
    let base_fetch = run_git(
        config,
        log_path,
        &repo_dir,
        &["fetch", "--depth", &depth, "origin", &pr.base_sha],
        config.job_timeout,
        true,
    )
    .await?;
    let changed = if base_fetch.status.success() {
        let diff = run_git(
            config,
            log_path,
            &repo_dir,
            &["diff", "--name-only", &pr.base_sha, &pr.head_sha],
            Duration::from_secs(60),
            false,
        )
        .await?;
        if diff.status.success() {
            let paths: Vec<String> = diff
                .stdout
                .lines()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(ToString::to_string)
                .collect();
            append_log(
                log_path,
                &format!("PR diff produced {} changed paths\n", paths.len()),
                config.max_log_bytes,
            )
            .await;
            Some(paths)
        } else {
            append_log(
                log_path,
                &format!(
                    "PR diff base..head failed: {} (will scan whole tree)\n",
                    diff.stderr
                ),
                config.max_log_bytes,
            )
            .await;
            None
        }
    } else {
        append_log(
            log_path,
            &format!(
                "git fetch base_sha failed: {} (will scan whole tree)\n",
                base_fetch.stderr
            ),
            config.max_log_bytes,
        )
        .await;
        None
    };

    Ok((repo_dir, changed))
}

// ---------------------------------------------------------------------------
// GitHub: webhook HMAC + PR comment posting
// ---------------------------------------------------------------------------

type HmacSha256 = Hmac<Sha256>;

fn verify_github_signature(secret: &str, body: &[u8], header_value: &str) -> bool {
    let prefix = "sha256=";
    if !header_value.starts_with(prefix) {
        return false;
    }
    let provided_hex = &header_value[prefix.len()..];
    let Ok(provided) = hex::decode(provided_hex) else {
        return false;
    };
    let Ok(mut mac) = HmacSha256::new_from_slice(secret.as_bytes()) else {
        return false;
    };
    mac.update(body);
    mac.verify_slice(&provided).is_ok()
}

fn extract_pr_from_event(payload: &Value) -> Result<PullRequestRef, String> {
    let pr = payload
        .get("pull_request")
        .ok_or_else(|| "payload is missing pull_request".to_string())?;
    let repository = payload
        .get("repository")
        .ok_or_else(|| "payload is missing repository".to_string())?;

    let full_name = repository
        .get("full_name")
        .and_then(Value::as_str)
        .ok_or_else(|| "repository.full_name missing".to_string())?;
    let (owner, repo) = full_name
        .split_once('/')
        .ok_or_else(|| "repository.full_name is not owner/repo".to_string())?;
    let number = pr
        .get("number")
        .and_then(Value::as_u64)
        .ok_or_else(|| "pull_request.number missing".to_string())?;

    let head = pr
        .get("head")
        .ok_or_else(|| "pull_request.head missing".to_string())?;
    let base = pr
        .get("base")
        .ok_or_else(|| "pull_request.base missing".to_string())?;
    let head_sha = head
        .get("sha")
        .and_then(Value::as_str)
        .ok_or_else(|| "pull_request.head.sha missing".to_string())?;
    let base_sha = base
        .get("sha")
        .and_then(Value::as_str)
        .ok_or_else(|| "pull_request.base.sha missing".to_string())?;
    let head_repo = head
        .get("repo")
        .ok_or_else(|| "pull_request.head.repo missing".to_string())?;
    let head_clone_url = head_repo
        .get("clone_url")
        .and_then(Value::as_str)
        .ok_or_else(|| "pull_request.head.repo.clone_url missing".to_string())?;

    let title = pr.get("title").and_then(Value::as_str).map(String::from);
    let html_url = pr.get("html_url").and_then(Value::as_str).map(String::from);
    let head_ref = head.get("ref").and_then(Value::as_str).map(String::from);
    let base_ref = base.get("ref").and_then(Value::as_str).map(String::from);
    let sender = payload
        .get("sender")
        .and_then(|s| s.get("login"))
        .and_then(Value::as_str)
        .map(String::from);

    Ok(PullRequestRef {
        owner: owner.to_string(),
        repo: repo.to_string(),
        number,
        head_sha: head_sha.to_string(),
        base_sha: base_sha.to_string(),
        head_clone_url: head_clone_url.to_string(),
        head_ref,
        base_ref,
        title,
        html_url,
        sender,
    })
}

fn render_pr_comment_body(
    pr: &PullRequestRef,
    findings: &[Finding],
    job_id: &str,
    files_scanned: usize,
    z3_queries: u64,
    diff_only_paths: Option<usize>,
    config: &Config,
) -> String {
    let mut body = String::new();
    body.push_str(&format!(
        "**dd-formal-methods-server** — PR #{} ({}/{})\n\n",
        pr.number, pr.owner, pr.repo
    ));
    body.push_str(&format!(
        "- head: `{}`\n- base: `{}`\n- job: `{}`\n- files scanned: {}\n- Z3 queries: {}\n",
        pr.head_sha, pr.base_sha, job_id, files_scanned, z3_queries
    ));
    if let Some(n) = diff_only_paths {
        body.push_str(&format!(
            "- analysis scope: {n} changed paths (base..head diff)\n"
        ));
    } else {
        body.push_str("- analysis scope: whole tree (diff fallback)\n");
    }
    body.push_str("\n");

    if findings.is_empty() {
        body.push_str(
            "✅ No formal-methods findings. All declared `@requires` / `@ensures` / `@assert` / `@invariant` goals were discharged by Z3.\n",
        );
        return body;
    }

    let errors = findings
        .iter()
        .filter(|f| f.severity == Severity::Error)
        .count();
    let warnings = findings
        .iter()
        .filter(|f| f.severity == Severity::Warning)
        .count();
    let infos = findings
        .iter()
        .filter(|f| f.severity == Severity::Info)
        .count();
    body.push_str(&format!(
        "🔎 {} finding(s): {} error · {} warning · {} info\n\n",
        findings.len(),
        errors,
        warnings,
        infos
    ));

    body.push_str("| Severity | Kind | File | Line | Message |\n");
    body.push_str("| --- | --- | --- | --- | --- |\n");
    let max_rows = config.pr_comment_max_rows;
    for f in findings.iter().take(max_rows) {
        let sev = match f.severity {
            Severity::Error => "🔴 error",
            Severity::Warning => "🟠 warning",
            Severity::Info => "🔵 info",
        };
        let kind = format!("{:?}", f.kind);
        let msg = f.message.replace('|', "\\|");
        body.push_str(&format!(
            "| {} | `{}` | `{}` | {} | {} |\n",
            sev, kind, f.file, f.line, msg
        ));
    }
    if findings.len() > max_rows {
        body.push_str(&format!(
            "\n_…and {} more finding(s); see `GET /analyses/{}` for the full list._\n",
            findings.len() - max_rows,
            job_id
        ));
    }

    let with_models: Vec<&Finding> = findings
        .iter()
        .filter(|f| f.counterexample.as_ref().is_some_and(|m| !m.is_empty()))
        .take(5)
        .collect();
    if !with_models.is_empty() {
        body.push_str("\n<details><summary>Counterexamples</summary>\n\n");
        for f in with_models {
            body.push_str(&format!(
                "- **{}** at `{}:{}` (goal: `{}`):\n",
                format_args!("{:?}", f.kind),
                f.file,
                f.line,
                f.goal.as_deref().unwrap_or("")
            ));
            if let Some(model) = &f.counterexample {
                for (k, v) in model {
                    body.push_str(&format!("  - `{k} = {v}`\n"));
                }
            }
        }
        body.push_str("\n</details>\n");
    }

    body.push_str(
        "\n_Posted by dd-formal-methods-server. Goals come from `@requires` / `@ensures` / `@assert` / `@invariant` comments — see the project readme for the DSL._\n",
    );
    body
}

async fn post_pr_comment(state: &AppState, pr: &PullRequestRef, body: &str) -> Result<(), String> {
    let token = state
        .config
        .github_api_token
        .as_deref()
        .ok_or_else(|| "GITHUB_API_TOKEN is not configured".to_string())?;
    let url = format!(
        "{}/repos/{}/{}/issues/{}/comments",
        state.config.github_api_base, pr.owner, pr.repo, pr.number
    );
    let response = state
        .http
        .post(&url)
        .header("authorization", format!("Bearer {token}"))
        .header("accept", "application/vnd.github+json")
        .header("x-github-api-version", "2022-11-28")
        .header(
            "user-agent",
            format!("{SERVICE_NAME}/0.1 (+https://github.com/ORESoftware/k8s-cluster)"),
        )
        .json(&json!({ "body": body }))
        .send()
        .await
        .map_err(|error| format!("failed to POST PR comment: {error}"))?;
    let status = response.status();
    let text = response
        .text()
        .await
        .unwrap_or_else(|_| "<no body>".to_string());
    if !status.is_success() {
        return Err(format!(
            "PR comment POST returned HTTP {}: {}",
            status.as_u16(),
            text.chars().take(400).collect::<String>()
        ));
    }
    Ok(())
}

struct JobOutcome {
    findings: Vec<Finding>,
    files_scanned: usize,
    z3_queries: u64,
    changed_paths: Option<Vec<String>>,
}

async fn execute_job(state: &AppState, job: &JobRecord) -> Result<JobOutcome, String> {
    let config = state.config.as_ref();
    let request = &job.request;
    let job_dir = config.work_root.join(&job.id);
    let log_path = PathBuf::from(&job.log_path);

    fs::create_dir_all(&job_dir)
        .await
        .map_err(|error| format!("failed to create job dir: {error}"))?;
    append_log(
        &log_path,
        &format!(
            "{SERVICE_NAME} starting job={} repo={} inline={} pr={}\n",
            job.id,
            request.repo_url.as_deref().unwrap_or("<none>"),
            request.inline_source.is_some(),
            request
                .pull_request
                .as_ref()
                .map(|p| format!("{}/{}#{}", p.owner, p.repo, p.number))
                .unwrap_or_else(|| "<none>".to_string())
        ),
        config.max_log_bytes,
    )
    .await;

    let languages_filter: Option<HashSet<String>> = request.languages.as_ref().map(|langs| {
        langs
            .iter()
            .map(|l| l.trim_start_matches('.').to_ascii_lowercase())
            .collect()
    });
    let mut path_filter: Option<Vec<PathBuf>> = match request.paths.as_ref() {
        Some(paths) => {
            let mut clean = Vec::new();
            for path in paths {
                clean.push(validate_relative_path("paths[]", path)?);
            }
            Some(clean)
        }
        None => None,
    };

    let z3_calls = AtomicU64::new(0);
    let z3_failures = AtomicU64::new(0);
    let heuristics_enabled = request.heuristics.unwrap_or(true);

    let (findings, files_scanned, changed_paths) =
        if let Some(source) = request.inline_source.as_deref() {
            let file_label = request
                .inline_filename
                .as_deref()
                .unwrap_or("inline.txt")
                .to_string();
            let ctx = VerifyContext { config };
            let parsed = parse_annotations(&file_label, source);
            let mut decls_lookup = HashMap::new();
            for block in &parsed.blocks {
                for decl in &block.decls {
                    decls_lookup.insert(decl.name.clone(), decl.sort.clone());
                }
            }
            let mut findings = Vec::new();
            for block in &parsed.blocks {
                let mut block_findings = verify_block(&ctx, block, &z3_calls, &z3_failures).await;
                findings.append(&mut block_findings);
            }
            if heuristics_enabled && !decls_lookup.is_empty() {
                let mut h =
                    heuristic_checks(&ctx, &parsed, &decls_lookup, &z3_calls, &z3_failures).await;
                findings.append(&mut h);
            }
            (findings, 1usize, None)
        } else if let Some(pr) = request.pull_request.as_ref() {
            let (repo_dir, changed) = clone_for_pr(config, &log_path, &job_dir, pr).await?;
            if config.pr_diff_only && path_filter.is_none() {
                if let Some(changed_paths) = changed.as_ref() {
                    let cleaned: Vec<PathBuf> = changed_paths
                        .iter()
                        .filter_map(|p| validate_relative_path("changed_paths[]", p).ok())
                        .collect();
                    if !cleaned.is_empty() {
                        path_filter = Some(cleaned);
                    }
                }
            }
            let (findings, files_scanned) = analyze_tree(
                state,
                &repo_dir,
                &languages_filter,
                &path_filter,
                heuristics_enabled,
                &log_path,
                &z3_calls,
                &z3_failures,
            )
            .await;
            (findings, files_scanned, changed)
        } else {
            let repo_url = request
                .repo_url
                .as_deref()
                .ok_or_else(|| "repoUrl missing".to_string())?;
            let git_ref = clean_optional(request.git_ref.as_deref());
            let repo_dir =
                clone_repo(config, &log_path, &job_dir, repo_url, git_ref.as_deref()).await?;
            let (findings, files_scanned) = analyze_tree(
                state,
                &repo_dir,
                &languages_filter,
                &path_filter,
                heuristics_enabled,
                &log_path,
                &z3_calls,
                &z3_failures,
            )
            .await;
            (findings, files_scanned, None)
        };

    let z3_calls_final = z3_calls.load(Ordering::Relaxed);
    let z3_failures_final = z3_failures.load(Ordering::Relaxed);
    state
        .counters
        .z3_calls
        .fetch_add(z3_calls_final, Ordering::Relaxed);
    state
        .counters
        .z3_failures
        .fetch_add(z3_failures_final, Ordering::Relaxed);

    append_log(
        &log_path,
        &format!(
            "{SERVICE_NAME} completed job={} findings={} files={} z3_calls={}\n",
            job.id,
            findings.len(),
            files_scanned,
            z3_calls_final
        ),
        config.max_log_bytes,
    )
    .await;

    Ok(JobOutcome {
        findings,
        files_scanned,
        z3_queries: z3_calls_final,
        changed_paths,
    })
}

async fn run_job(state: AppState, id: String) {
    let permit = match state.semaphore.clone().acquire_owned().await {
        Ok(permit) => permit,
        Err(error) => {
            update_job(&state, &id, |job| {
                job.status = JobStatus::Failed;
                job.finished_at_ms = Some(now_ms());
                job.error = Some(format!("queue is closed: {error}"));
            })
            .await;
            return;
        }
    };
    state.counters.running.fetch_add(1, Ordering::Relaxed);
    update_job(&state, &id, |job| {
        job.status = JobStatus::Running;
        job.started_at_ms = Some(now_ms());
    })
    .await;
    let job_snapshot = {
        let jobs = state.jobs.read().await;
        jobs.get(&id).cloned()
    };
    let result = match job_snapshot.as_ref() {
        Some(job) => execute_job(&state, job).await,
        None => Err("job disappeared before execution".to_string()),
    };
    state.counters.running.fetch_sub(1, Ordering::Relaxed);
    drop(permit);

    match result {
        Ok(outcome) => {
            state.counters.succeeded.fetch_add(1, Ordering::Relaxed);
            state
                .counters
                .findings_total
                .fetch_add(outcome.findings.len() as u64, Ordering::Relaxed);

            let pr = job_snapshot.as_ref().and_then(|j| j.pull_request.clone());
            let log_path = job_snapshot.as_ref().map(|j| PathBuf::from(&j.log_path));

            let comment_status = if let Some(pr) = pr.as_ref() {
                if state.config.pr_comment_enabled && state.config.github_api_token.is_some() {
                    let body = render_pr_comment_body(
                        pr,
                        &outcome.findings,
                        &id,
                        outcome.files_scanned,
                        outcome.z3_queries,
                        outcome.changed_paths.as_ref().map(|c| c.len()),
                        &state.config,
                    );
                    match post_pr_comment(&state, pr, &body).await {
                        Ok(()) => {
                            state
                                .counters
                                .pr_comments_posted
                                .fetch_add(1, Ordering::Relaxed);
                            if let Some(log) = log_path.as_ref() {
                                append_log(
                                    log,
                                    &format!(
                                        "posted PR comment to {}/{}#{}\n",
                                        pr.owner, pr.repo, pr.number
                                    ),
                                    state.config.max_log_bytes,
                                )
                                .await;
                            }
                            Some("posted".to_string())
                        }
                        Err(error) => {
                            state
                                .counters
                                .pr_comments_failed
                                .fetch_add(1, Ordering::Relaxed);
                            if let Some(log) = log_path.as_ref() {
                                append_log(
                                    log,
                                    &format!("PR comment failed: {error}\n"),
                                    state.config.max_log_bytes,
                                )
                                .await;
                            }
                            Some(format!("failed: {error}"))
                        }
                    }
                } else {
                    Some("disabled".to_string())
                }
            } else {
                None
            };

            update_job(&state, &id, |job| {
                job.status = JobStatus::Succeeded;
                job.finished_at_ms = Some(now_ms());
                job.error = None;
                job.findings_count = outcome.findings.len();
                job.findings = outcome.findings;
                job.files_scanned = outcome.files_scanned;
                job.z3_queries = outcome.z3_queries;
                job.changed_paths = outcome.changed_paths;
                job.pr_comment_status = comment_status;
            })
            .await;
        }
        Err(error) => {
            state.counters.failed.fetch_add(1, Ordering::Relaxed);
            update_job(&state, &id, |job| {
                job.status = JobStatus::Failed;
                job.finished_at_ms = Some(now_ms());
                job.error = Some(error);
            })
            .await;
        }
    }
}

// ---------------------------------------------------------------------------
// HTTP handlers
// ---------------------------------------------------------------------------

async fn descriptor() -> impl IntoResponse {
    Json(json!({
        "service": SERVICE_NAME,
        "description": "Annotation-driven formal-methods analyser. Submit a repo, inline source, or a GitHub pull-request webhook event; get SMT-checked findings.",
        "schemaVersion": SCHEMA_VERSION,
        "endpoints": {
            "submit": "POST /analyses",
            "list": "GET /analyses",
            "status": "GET /analyses/<jobId>",
            "logs": "GET /analyses/<jobId>/logs",
            "validate": "POST /validate",
            "githubWebhook": "POST /webhooks/github (HMAC-verified, no x-server-auth)",
            "pullRequestStatus": "GET /pulls/<owner>/<repo>/<number>",
            "healthz": "GET /healthz",
            "metrics": "GET /metrics"
        },
        "annotationDsl": {
            "decl": "// @var name: Int|Real|Bool",
            "assume": "// @assume <expr>",
            "requires": "// @requires <expr>",
            "ensures": "// @ensures <expr>",
            "invariant": "// @invariant <expr>",
            "variant": "// @variant <int-expr>",
            "assert": "// @assert <expr>"
        },
        "supportedOperators": [
            "||", "&&", "!", "==", "!=", "<", "<=", ">", ">=",
            "+", "-", "*", "/", "%",
            "min(_,_)", "max(_,_)", "abs(_)"
        ],
        "reasoningModes": [
            "deduction (SMT refutation of negated goals)",
            "induction (loop @invariant base step + @variant non-negativity)",
            "path-condition propagation through nested `if (...)` branches"
        ],
        "githubWebhook": {
            "url": "POST /webhooks/github",
            "signatureHeader": "X-Hub-Signature-256",
            "secretEnv": "GITHUB_WEBHOOK_SECRET",
            "events": ["pull_request"],
            "actions": ["opened", "synchronize", "reopened", "ready_for_review"]
        }
    }))
}

async fn healthz(State(state): State<AppState>) -> impl IntoResponse {
    let jobs = state.jobs.read().await;
    let queued = jobs
        .values()
        .filter(|job| matches!(job.status, JobStatus::Queued))
        .count();
    let mut allowed_repo_prefixes = state.config.allowed_repo_prefixes.clone();
    allowed_repo_prefixes.sort();
    let mut allowed_extensions: Vec<String> =
        state.config.allowed_extensions.iter().cloned().collect();
    allowed_extensions.sort();
    let z3_available = which_exists(&state.config.z3_bin).await;
    Json(HealthResponse {
        ok: true,
        service: SERVICE_NAME,
        schema_version: SCHEMA_VERSION,
        auth_configured: state.config.server_auth_secret.is_some(),
        z3_available,
        github_webhook_configured: state.config.github_webhook_secret.is_some(),
        github_comments_enabled: state.config.pr_comment_enabled
            && state.config.github_api_token.is_some(),
        pr_diff_only: state.config.pr_diff_only,
        allowed_repo_prefixes,
        allowed_extensions,
        queued,
        running: state.counters.running.load(Ordering::Relaxed),
    })
}

async fn which_exists(bin: &str) -> bool {
    if bin.contains('/') {
        return fs::metadata(bin).await.is_ok();
    }
    let paths = env::var("PATH").unwrap_or_default();
    for dir in paths.split(':') {
        if dir.is_empty() {
            continue;
        }
        let candidate = Path::new(dir).join(bin);
        if fs::metadata(&candidate).await.is_ok() {
            return true;
        }
    }
    false
}

async fn metrics(State(state): State<AppState>) -> impl IntoResponse {
    let jobs = state.jobs.read().await;
    let queued = jobs
        .values()
        .filter(|job| matches!(job.status, JobStatus::Queued))
        .count();
    let body = format!(
        "# HELP dd_formal_methods_jobs_submitted_total Analysis jobs accepted.\n\
         # TYPE dd_formal_methods_jobs_submitted_total counter\n\
         dd_formal_methods_jobs_submitted_total {}\n\
         # HELP dd_formal_methods_jobs_running Current running jobs.\n\
         # TYPE dd_formal_methods_jobs_running gauge\n\
         dd_formal_methods_jobs_running {}\n\
         # HELP dd_formal_methods_jobs_queued Current queued jobs.\n\
         # TYPE dd_formal_methods_jobs_queued gauge\n\
         dd_formal_methods_jobs_queued {}\n\
         # HELP dd_formal_methods_jobs_succeeded_total Analyses that completed successfully.\n\
         # TYPE dd_formal_methods_jobs_succeeded_total counter\n\
         dd_formal_methods_jobs_succeeded_total {}\n\
         # HELP dd_formal_methods_jobs_failed_total Analyses that failed.\n\
         # TYPE dd_formal_methods_jobs_failed_total counter\n\
         dd_formal_methods_jobs_failed_total {}\n\
         # HELP dd_formal_methods_requests_rejected_total Requests rejected before queueing.\n\
         # TYPE dd_formal_methods_requests_rejected_total counter\n\
         dd_formal_methods_requests_rejected_total {}\n\
         # HELP dd_formal_methods_findings_total Findings emitted across all analyses.\n\
         # TYPE dd_formal_methods_findings_total counter\n\
         dd_formal_methods_findings_total {}\n\
         # HELP dd_formal_methods_z3_calls_total Z3 invocations.\n\
         # TYPE dd_formal_methods_z3_calls_total counter\n\
         dd_formal_methods_z3_calls_total {}\n\
         # HELP dd_formal_methods_z3_failures_total Z3 invocations that errored.\n\
         # TYPE dd_formal_methods_z3_failures_total counter\n\
         dd_formal_methods_z3_failures_total {}\n\
         # HELP dd_formal_methods_webhooks_received_total GitHub webhooks accepted.\n\
         # TYPE dd_formal_methods_webhooks_received_total counter\n\
         dd_formal_methods_webhooks_received_total {}\n\
         # HELP dd_formal_methods_webhooks_rejected_total GitHub webhooks rejected (bad HMAC or shape).\n\
         # TYPE dd_formal_methods_webhooks_rejected_total counter\n\
         dd_formal_methods_webhooks_rejected_total {}\n\
         # HELP dd_formal_methods_pr_jobs_queued_total PR-driven analysis jobs queued.\n\
         # TYPE dd_formal_methods_pr_jobs_queued_total counter\n\
         dd_formal_methods_pr_jobs_queued_total {}\n\
         # HELP dd_formal_methods_pr_comments_posted_total PR comments successfully posted to GitHub.\n\
         # TYPE dd_formal_methods_pr_comments_posted_total counter\n\
         dd_formal_methods_pr_comments_posted_total {}\n\
         # HELP dd_formal_methods_pr_comments_failed_total PR comment POSTs that failed.\n\
         # TYPE dd_formal_methods_pr_comments_failed_total counter\n\
         dd_formal_methods_pr_comments_failed_total {}\n",
        state.counters.submitted.load(Ordering::Relaxed),
        state.counters.running.load(Ordering::Relaxed),
        queued,
        state.counters.succeeded.load(Ordering::Relaxed),
        state.counters.failed.load(Ordering::Relaxed),
        state.counters.rejected.load(Ordering::Relaxed),
        state.counters.findings_total.load(Ordering::Relaxed),
        state.counters.z3_calls.load(Ordering::Relaxed),
        state.counters.z3_failures.load(Ordering::Relaxed),
        state.counters.webhooks_received.load(Ordering::Relaxed),
        state.counters.webhooks_rejected.load(Ordering::Relaxed),
        state.counters.pr_jobs_queued.load(Ordering::Relaxed),
        state.counters.pr_comments_posted.load(Ordering::Relaxed),
        state.counters.pr_comments_failed.load(Ordering::Relaxed),
    );
    (
        [(
            header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )],
        body,
    )
}

async fn enqueue_job(state: &AppState, request: AnalyzeRequest) -> JobRecord {
    let counter = state.counters.submitted.fetch_add(1, Ordering::Relaxed) + 1;
    let id = job_id(counter);
    let job_dir = state.config.work_root.join(&id);
    let log_path = job_dir.join("analysis.log");
    let pull_request = request.pull_request.clone();
    let record = JobRecord {
        id: id.clone(),
        status: JobStatus::Queued,
        request,
        created_at_ms: now_ms(),
        started_at_ms: None,
        finished_at_ms: None,
        log_path: log_path.to_string_lossy().to_string(),
        error: None,
        findings_count: 0,
        findings: Vec::new(),
        files_scanned: 0,
        z3_queries: 0,
        pull_request,
        changed_paths: None,
        pr_comment_status: None,
    };
    {
        let mut jobs = state.jobs.write().await;
        jobs.insert(id.clone(), record.clone());
    }
    prune_jobs(state).await;
    let task_state = state.clone();
    tokio::spawn(async move {
        run_job(task_state, id).await;
    });
    record
}

async fn submit_analysis(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<AnalyzeRequest>,
) -> Response {
    if let Err(response) = require_auth(&headers, &state) {
        return response;
    }
    if let Err(error) = validate_analyze_request(&state.config, &request) {
        state.counters.rejected.fetch_add(1, Ordering::Relaxed);
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": error }))).into_response();
    }
    let record = enqueue_job(&state, request).await;
    (StatusCode::ACCEPTED, Json(record)).into_response()
}

async fn github_webhook(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    state
        .counters
        .webhooks_received
        .fetch_add(1, Ordering::Relaxed);
    let Some(secret) = state.config.github_webhook_secret.as_deref() else {
        state
            .counters
            .webhooks_rejected
            .fetch_add(1, Ordering::Relaxed);
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "GITHUB_WEBHOOK_SECRET is not configured" })),
        )
            .into_response();
    };
    let signature = match headers
        .get("x-hub-signature-256")
        .and_then(|v| v.to_str().ok())
    {
        Some(value) => value.to_string(),
        None => {
            state
                .counters
                .webhooks_rejected
                .fetch_add(1, Ordering::Relaxed);
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({ "error": "missing X-Hub-Signature-256 header" })),
            )
                .into_response();
        }
    };
    if !verify_github_signature(secret, &body, &signature) {
        state
            .counters
            .webhooks_rejected
            .fetch_add(1, Ordering::Relaxed);
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "invalid X-Hub-Signature-256" })),
        )
            .into_response();
    }
    let event = headers
        .get("x-github-event")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    match event {
        "ping" => Json(json!({
            "ok": true,
            "service": SERVICE_NAME,
            "event": "ping",
        }))
        .into_response(),
        "pull_request" => handle_pull_request_event(state, &body).await,
        other => Json(json!({
            "ignored": true,
            "event": other,
        }))
        .into_response(),
    }
}

async fn handle_pull_request_event(state: AppState, body: &[u8]) -> Response {
    let payload: Value = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(error) => {
            state
                .counters
                .webhooks_rejected
                .fetch_add(1, Ordering::Relaxed);
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": format!("invalid JSON: {error}") })),
            )
                .into_response();
        }
    };
    let action = payload.get("action").and_then(Value::as_str).unwrap_or("");
    if !matches!(
        action,
        "opened" | "synchronize" | "reopened" | "ready_for_review"
    ) {
        return Json(json!({
            "ignored": true,
            "action": action,
        }))
        .into_response();
    }
    let pr = match extract_pr_from_event(&payload) {
        Ok(pr) => pr,
        Err(error) => {
            state
                .counters
                .webhooks_rejected
                .fetch_add(1, Ordering::Relaxed);
            return (StatusCode::BAD_REQUEST, Json(json!({ "error": error }))).into_response();
        }
    };
    if let Err(error) = validate_repo_url(&pr.head_clone_url) {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": error }))).into_response();
    }
    if let Err(error) = ensure_allowed_prefix(
        "pull_request.head_clone_url",
        &pr.head_clone_url,
        &state.config.allowed_repo_prefixes,
        "FORMAL_METHODS_ALLOWED_REPO_PREFIXES",
    ) {
        return (StatusCode::FORBIDDEN, Json(json!({ "error": error }))).into_response();
    }
    let request = AnalyzeRequest {
        schema_version: Some(SCHEMA_VERSION.to_string()),
        repo_url: Some(pr.head_clone_url.clone()),
        git_ref: Some(pr.head_sha.clone()),
        paths: None,
        languages: None,
        inline_source: None,
        inline_filename: None,
        heuristics: Some(true),
        pull_request: Some(pr.clone()),
    };
    let record = enqueue_job(&state, request).await;
    state
        .counters
        .pr_jobs_queued
        .fetch_add(1, Ordering::Relaxed);
    (StatusCode::ACCEPTED, Json(record)).into_response()
}

async fn get_pull_request_status(
    State(state): State<AppState>,
    AxumPath((owner, repo, number)): AxumPath<(String, String, u64)>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = require_auth(&headers, &state) {
        return response;
    }
    let jobs = state.jobs.read().await;
    let mut matched: Vec<JobRecord> = jobs
        .values()
        .filter(|job| {
            job.pull_request.as_ref().is_some_and(|pr| {
                pr.owner.eq_ignore_ascii_case(&owner)
                    && pr.repo.eq_ignore_ascii_case(&repo)
                    && pr.number == number
            })
        })
        .cloned()
        .collect();
    if matched.is_empty() {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "no analysis jobs found for that pull request" })),
        )
            .into_response();
    }
    matched.sort_by(|a, b| b.created_at_ms.cmp(&a.created_at_ms));
    let latest = matched.first().cloned();
    Json(json!({
        "owner": owner,
        "repo": repo,
        "number": number,
        "latest": latest,
        "jobs": matched,
    }))
    .into_response()
}

async fn list_analyses(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(response) = require_auth(&headers, &state) {
        return response;
    }
    let mut jobs = state
        .jobs
        .read()
        .await
        .values()
        .cloned()
        .collect::<Vec<_>>();
    jobs.sort_by(|a, b| b.created_at_ms.cmp(&a.created_at_ms));
    Json(jobs).into_response()
}

async fn get_analysis(
    State(state): State<AppState>,
    AxumPath(job_id): AxumPath<String>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = require_auth(&headers, &state) {
        return response;
    }
    let jobs = state.jobs.read().await;
    match jobs.get(&job_id) {
        Some(job) => Json(job).into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "analysis job not found" })),
        )
            .into_response(),
    }
}

async fn get_analysis_logs(
    State(state): State<AppState>,
    AxumPath(job_id): AxumPath<String>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = require_auth(&headers, &state) {
        return response;
    }
    let log_path = {
        let jobs = state.jobs.read().await;
        match jobs.get(&job_id) {
            Some(job) => PathBuf::from(&job.log_path),
            None => {
                return (
                    StatusCode::NOT_FOUND,
                    Json(json!({ "error": "analysis job not found" })),
                )
                    .into_response();
            }
        }
    };
    match fs::read_to_string(&log_path).await {
        Ok(body) => ([(header::CONTENT_TYPE, "text/plain; charset=utf-8")], body).into_response(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "analysis log not found" })),
        )
            .into_response(),
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": format!("failed to read analysis log: {error}") })),
        )
            .into_response(),
    }
}

async fn validate_inline(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<ValidateRequest>,
) -> Response {
    if let Err(response) = require_auth(&headers, &state) {
        return response;
    }
    if let Some(version) = clean_optional(request.schema_version.as_deref()) {
        if version != SCHEMA_VERSION {
            state.counters.rejected.fetch_add(1, Ordering::Relaxed);
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": format!("schemaVersion must be {SCHEMA_VERSION}") })),
            )
                .into_response();
        }
    }
    if request.source.len() > state.config.max_inline_source_bytes {
        state.counters.rejected.fetch_add(1, Ordering::Relaxed);
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "error": format!(
                    "source must be {} bytes or fewer",
                    state.config.max_inline_source_bytes
                )
            })),
        )
            .into_response();
    }

    let z3_calls = AtomicU64::new(0);
    let z3_failures = AtomicU64::new(0);
    let ctx = VerifyContext {
        config: &state.config,
    };
    let file_label = request
        .filename
        .clone()
        .unwrap_or_else(|| "inline.txt".to_string());
    let parsed = parse_annotations(&file_label, &request.source);
    let mut decls_lookup = HashMap::new();
    for block in &parsed.blocks {
        for decl in &block.decls {
            decls_lookup.insert(decl.name.clone(), decl.sort.clone());
        }
    }
    let mut findings = Vec::new();
    for block in &parsed.blocks {
        let mut block_findings = verify_block(&ctx, block, &z3_calls, &z3_failures).await;
        findings.append(&mut block_findings);
    }
    if request.heuristics.unwrap_or(true) && !decls_lookup.is_empty() {
        let mut h = heuristic_checks(&ctx, &parsed, &decls_lookup, &z3_calls, &z3_failures).await;
        findings.append(&mut h);
    }
    let z3_calls_final = z3_calls.load(Ordering::Relaxed);
    state
        .counters
        .z3_calls
        .fetch_add(z3_calls_final, Ordering::Relaxed);
    state
        .counters
        .z3_failures
        .fetch_add(z3_failures.load(Ordering::Relaxed), Ordering::Relaxed);
    state
        .counters
        .findings_total
        .fetch_add(findings.len() as u64, Ordering::Relaxed);
    Json(ValidateResponse {
        schema_version: SCHEMA_VERSION,
        findings_count: findings.len(),
        findings,
        z3_queries: z3_calls_final,
    })
    .into_response()
}

// ---------------------------------------------------------------------------
// JSON helpers
// ---------------------------------------------------------------------------

// Pretty-rendered JSON for ad-hoc debugging endpoints (unused but kept for
// completeness so consumers can opt into pretty output if desired).
#[allow(dead_code)]
fn pretty(value: &Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
}

// ---------------------------------------------------------------------------
// main
// ---------------------------------------------------------------------------

async fn api_docs_html() -> axum::response::Html<&'static str> {
    axum::response::Html(include_str!("../generated/api-docs.html"))
}

async fn api_docs_json() -> impl axum::response::IntoResponse {
    (
        [("content-type", "application/json; charset=utf-8")],
        include_str!("../generated/api-docs.json"),
    )
}

#[tokio::main]
async fn main() {
    let config = Arc::new(config_from_env());
    let host = env_value("HOST", "0.0.0.0");
    let port = env_u64("PORT", DEFAULT_PORT as u64) as u16;
    let max_concurrent = env_usize("FORMAL_METHODS_MAX_CONCURRENT", 2);

    if let Err(error) = fs::create_dir_all(&config.work_root).await {
        panic!("failed to create formal-methods work root: {error}");
    }

    let http = reqwest::Client::builder()
        .user_agent(format!(
            "{SERVICE_NAME}/0.1 (+https://github.com/ORESoftware/k8s-cluster)"
        ))
        .timeout(Duration::from_secs(30))
        .build()
        .expect("failed to build reqwest client");

    let state = AppState {
        config,
        http,
        jobs: Arc::new(RwLock::new(HashMap::new())),
        semaphore: Arc::new(Semaphore::new(max_concurrent)),
        counters: Arc::new(Counters::default()),
    };

    let app = Router::new()
        .route("/", get(descriptor))
        .route("/healthz", get(healthz))
        .route("/docs/api", get(api_docs_html))
        .route("/api/docs", get(api_docs_html))
        .route("/api/docs.json", get(api_docs_json))
        .route("/metrics", get(metrics))
        .route("/analyses", get(list_analyses).post(submit_analysis))
        .route("/analyses/:job_id", get(get_analysis))
        .route("/analyses/:job_id/logs", get(get_analysis_logs))
        .route("/validate", post(validate_inline))
        .route("/webhooks/github", post(github_webhook))
        .route("/pulls/:owner/:repo/:number", get(get_pull_request_status))
        .with_state(state)
        .merge(dd_runtime_config_client::router());

    tokio::spawn(dd_runtime_config_client::register_with_control_plane());

    let address: SocketAddr = format!("{host}:{port}")
        .parse()
        .expect("failed to parse bind address");
    println!("{SERVICE_NAME} listening on http://{address}");

    let listener = tokio::net::TcpListener::bind(address)
        .await
        .expect("failed to bind tcp listener");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .expect("axum server crashed");
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };
    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => {}
        _ = terminate => {}
    }
}

// ---------------------------------------------------------------------------
// tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lexer_handles_basic_tokens() {
        let toks = lex("x >= 0 && y != -3").unwrap();
        assert!(matches!(toks[0], Token::Ident(ref n) if n == "x"));
        assert!(matches!(toks[1], Token::OpGe));
        assert!(matches!(toks[2], Token::Int(ref n) if n == "0"));
        assert!(matches!(toks[3], Token::OpAnd));
    }

    #[test]
    fn parser_respects_precedence() {
        let expr = parse_expr("a + b * c >= d").unwrap();
        let smt = expr_to_smt(&expr).unwrap();
        assert_eq!(smt, "(>= (+ a (* b c)) d)");
    }

    #[test]
    fn parser_handles_neq_as_not_eq() {
        let expr = parse_expr("a != b").unwrap();
        assert_eq!(expr_to_smt(&expr).unwrap(), "(not (= a b))");
    }

    #[test]
    fn parser_handles_unary() {
        let expr = parse_expr("!(x > 0) || y == -1").unwrap();
        let smt = expr_to_smt(&expr).unwrap();
        assert_eq!(smt, "(or (not (> x 0)) (= y (- 1)))");
    }

    #[test]
    fn parser_handles_min_max_abs() {
        let expr = parse_expr("min(a, max(b, c)) > abs(d)").unwrap();
        let smt = expr_to_smt(&expr).unwrap();
        assert!(smt.contains("ite"));
    }

    #[test]
    fn annotation_extractor_keeps_block_across_blank_comment_lines() {
        let src = "\
// @var x: Int
// @requires x > 0
// @assume x < 100
//
// @ensures x + 1 > 0
fn f(x: i64) -> i64 { x + 1 }
";
        let parsed = parse_annotations("t.rs", src);
        assert_eq!(parsed.blocks.len(), 1, "blank `//` must not split a block");
        let block = &parsed.blocks[0];
        assert_eq!(block.decls.len(), 1);
        assert_eq!(block.requires.len(), 1);
        assert_eq!(block.assumes.len(), 1);
        assert_eq!(block.ensures.len(), 1);
    }

    #[test]
    fn annotation_extractor_keeps_block_across_prose_comment_lines() {
        let src = "\
// @var x: Int
// @requires x > 0
// Some explanatory prose between requires and ensures.
// @ensures x + 1 > 0
fn f(x: i64) -> i64 { x + 1 }
";
        let parsed = parse_annotations("t.rs", src);
        assert_eq!(parsed.blocks.len(), 1);
        assert_eq!(parsed.blocks[0].ensures.len(), 1);
    }

    #[test]
    fn annotation_extractor_groups_block() {
        let src = "\
// @var x: Int
// @var y: Int
// @requires x > 0
// @requires y >= x
// @ensures x + y > 0
fn add(x: i64, y: i64) -> i64 { x + y }
";
        let parsed = parse_annotations("t.rs", src);
        assert_eq!(parsed.blocks.len(), 1);
        let block = &parsed.blocks[0];
        assert_eq!(block.decls.len(), 2);
        assert_eq!(block.requires.len(), 2);
        assert_eq!(block.ensures.len(), 1);
    }

    #[test]
    fn verify_github_signature_accepts_valid_hmac() {
        let secret = "It's a Secret to Everybody";
        let body = b"Hello, World!";
        // Pre-computed HMAC-SHA256 from GitHub's docs example.
        let expected = "sha256=757107ea0eb2509fc211221cce984b8a37570b6d7586c22c46f4379c8b043e17";
        assert!(verify_github_signature(secret, body, expected));
        assert!(!verify_github_signature(secret, b"tampered", expected));
        assert!(!verify_github_signature("wrong", body, expected,));
        assert!(!verify_github_signature(secret, body, "sha256=deadbeef"));
        assert!(!verify_github_signature(secret, body, "not-a-prefix"));
    }

    #[test]
    fn extract_pr_from_event_parses_minimal_payload() {
        let payload = serde_json::json!({
            "action": "opened",
            "pull_request": {
                "number": 42,
                "title": "Fix everything",
                "html_url": "https://github.com/example/repo/pull/42",
                "head": {
                    "sha": "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef",
                    "ref": "feature/x",
                    "repo": { "clone_url": "https://github.com/example/repo.git" }
                },
                "base": {
                    "sha": "cafebabecafebabecafebabecafebabecafebabe",
                    "ref": "main"
                }
            },
            "repository": { "full_name": "example/repo" },
            "sender": { "login": "octocat" }
        });
        let pr = extract_pr_from_event(&payload).unwrap();
        assert_eq!(pr.owner, "example");
        assert_eq!(pr.repo, "repo");
        assert_eq!(pr.number, 42);
        assert_eq!(pr.head_sha.len(), 40);
        assert_eq!(pr.base_sha.len(), 40);
        assert_eq!(pr.head_clone_url, "https://github.com/example/repo.git");
        assert_eq!(pr.head_ref.as_deref(), Some("feature/x"));
        assert_eq!(pr.sender.as_deref(), Some("octocat"));
    }

    #[test]
    fn is_sha_like_matches_short_and_long_hex() {
        assert!(is_sha_like("deadbeef"));
        assert!(is_sha_like("deadbeefdeadbeefdeadbeefdeadbeefdeadbeef"));
        assert!(!is_sha_like("not-a-sha"));
        assert!(!is_sha_like("abc"));
        assert!(!is_sha_like(""));
    }

    #[test]
    fn parse_model_handles_multiline_z3_output() {
        let raw = "sat\n(\n  (define-fun y () Int\n    0)\n  (define-fun x () Int\n    (- 3))\n)\n";
        let model = parse_model(raw);
        assert_eq!(model.get("y").map(String::as_str), Some("0"));
        assert_eq!(model.get("x").map(String::as_str), Some("(- 3)"));
    }

    #[test]
    fn collect_vars_picks_up_identifiers() {
        let expr = parse_expr("(x + y) * z > 0 && flag").unwrap();
        let mut vars = HashSet::new();
        collect_vars(&expr, &mut vars);
        assert!(vars.contains("x"));
        assert!(vars.contains("y"));
        assert!(vars.contains("z"));
        assert!(vars.contains("flag"));
    }
}
