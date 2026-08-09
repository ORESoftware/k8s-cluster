#!/usr/bin/env python3
"""Transparent source templates for generated organization MCP servers."""
from string import Template

LICENSE_TEXT = """MIT License

Copyright (c) 2026 ORESoftware contributors

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
"""

SECURITY_TEXT = """# Security policy

Report vulnerabilities privately through GitHub Security Advisories. Do not
open public issues containing credentials, tokens, clipboard or transcription
content, registry secrets, database URLs, or other user data.
"""

CARGO_TEMPLATE = Template(r'''[package]
name = "$binary_name"
version = "0.1.0"
edition = "2021"
rust-version = "$msrv"
description = "$description"
license = "MIT OR Apache-2.0"
repository = "https://github.com/$full_name"
publish = false

[lib]
name = "$crate_name"
path = "src/lib.rs"

[[bin]]
name = "$binary_name"
path = "src/main.rs"

[dependencies]
anyhow = "1"
ore-mcp-safety = { git = "$shared_repository", rev = "$shared_revision", package = "ore-mcp-safety" }
rmcp = { version = "=$rmcp_version", features = ["server", "macros", "schemars", "transport-io"] }
schemars = "1"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
tokio = { version = "1", features = ["macros", "rt-multi-thread", "time"] }
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter", "fmt"] }

[dev-dependencies]
ore-mcp-testkit = { git = "$shared_repository", rev = "$shared_revision", package = "ore-mcp-testkit" }
''')

LIB_TEMPLATE = r'''//! Organization-specific read-only MCP server.
#![forbid(unsafe_code)]

pub mod domain;
pub mod runtime;
pub mod server;
'''

MAIN_TEMPLATE = Template(r'''#[tokio::main]
async fn main() -> anyhow::Result<()> {
    $crate_name::runtime::run_stdio().await
}
''')

RUNTIME_TEMPLATE = Template(r'''use rmcp::{transport::stdio, ServiceExt};
use tracing_subscriber::{layer::SubscriberExt as _, util::SubscriberInitExt as _};

use crate::server::$server_type;

pub async fn run_stdio() -> anyhow::Result<()> {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| "info,rmcp=warn".into());
    tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer().with_writer(std::io::stderr))
        .init();
    tracing::info!(transport = "stdio", stdout = "mcp-only", "starting read-only MCP server");
    let service = $server_type::new().serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}
''')

DOMAIN_TEMPLATE = Template(r'''use rmcp::ErrorData;
use serde_json::{json, Value};

pub const ORGANIZATION: &str = "$owner";
pub const REPOSITORY: &str = "$full_name";
pub const ISSUE: &str = "$issue";
pub const TEMPLATE_DIGEST: &str = "$template_digest";

pub fn project_overview() -> Value {
    json!({
        "schema_version": 1,
        "organization": ORGANIZATION,
        "repository": REPOSITORY,
        "tracking_issue": ISSUE,
        "transport": "stdio",
        "protocol": "$protocol",
        "read_only": true,
        "template_digest": TEMPLATE_DIGEST,
        "description": "$description"
    })
}

pub fn repository_map() -> Value {
    json!({"schema_version": 1, "organization": ORGANIZATION, "repositories": $repositories})
}

pub fn domain_contract() -> Value { json!($domain_contract) }

pub fn safety_contract() -> Value {
    json!({
        "schema_version": 1,
        "read_only": true,
        "network_access": false,
        "subprocess_access": false,
        "filesystem_writes": false,
        "credentials_accepted": false,
        "prohibited_capabilities": ["publish", "tag", "write", "delete", "execute", "remote mutation"],
        "stdout": "MCP frames only",
        "diagnostics": "bounded structured stderr"
    })
}

pub fn validate_identifier(field: &str, value: &str, max: usize) -> Result<(), ErrorData> {
    if value.is_empty() || value.len() > max || value.starts_with('-') || value.chars().any(char::is_control)
        || !value.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':' | b'@'))
    {
        return Err(ErrorData::invalid_params(format!("{field} is invalid"), None));
    }
    Ok(())
}

pub fn validate_language_tag(value: &str) -> Result<(), ErrorData> {
    if value.is_empty() || value.len() > 35 || value.starts_with('-') || value.ends_with('-')
        || !value.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return Err(ErrorData::invalid_params("language_tag is invalid", None));
    }
    Ok(())
}

pub fn validate_revision(value: &str) -> Result<(), ErrorData> {
    if value.len() != 40 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(ErrorData::invalid_params("revision must be a 40-character hexadecimal Git commit", None));
    }
    Ok(())
}

pub fn validate_package_name(value: &str) -> Result<(), ErrorData> {
    if value.is_empty() || value.len() > 128 || (value.starts_with('-') || value.starts_with('.')) || value.ends_with('.')
        || !value.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return Err(ErrorData::invalid_params("package_name is invalid", None));
    }
    Ok(())
}

pub fn validate_relative_path(value: &str) -> Result<(), ErrorData> {
    if value.is_empty() || value.len() > 512 || value.starts_with('/') || value.contains("\\")
        || value.split('/').any(|segment| segment.is_empty() || matches!(segment, "." | ".."))
        || value.chars().any(char::is_control)
    {
        return Err(ErrorData::invalid_params("relative_path is invalid", None));
    }
    Ok(())
}
''')

SERVER_TEMPLATE = Template(r'''use rmcp::{
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{Implementation, ServerCapabilities, ServerInfo, ToolAnnotations},
    tool, tool_handler, tool_router, ErrorData, ServerHandler,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::domain;

$rust_types

#[derive(Clone)]
pub struct $server_type { tool_router: ToolRouter<Self> }

impl $server_type {
    pub fn new() -> Self {
        let mut tool_router = Self::tool_router();
        for route in tool_router.map.values_mut() {
            route.attr.annotations = Some(
                ToolAnnotations::new()
                    .read_only(true)
                    .destructive(false)
                    .idempotent(true)
                    .open_world(false),
            );
        }
        Self { tool_router }
    }
}

impl Default for $server_type { fn default() -> Self { Self::new() } }

fn render(value: &Value) -> Result<String, ErrorData> {
    let text = serde_json::to_string_pretty(value)
        .map_err(|_| ErrorData::internal_error("tool result serialization failed", None))?;
    let bound = ore_mcp_safety::Bounds::new($max_output_bytes, $max_output_bytes)
        .map_err(|_| ErrorData::internal_error("invalid output bound", None))?;
    if text.len() > bound.max_json_bytes {
        return Err(ErrorData::internal_error("tool result exceeded the configured limit", None));
    }
    Ok(text)
}

#[tool_router]
impl $server_type {
    #[tool(description = "Return the bounded read-only project purpose, protocol, and repository identity.")]
    fn project_overview(&self) -> Result<String, ErrorData> { render(&domain::project_overview()) }

    #[tool(description = "Return the reviewed organization repository map without contacting GitHub or exposing credentials.")]
    fn repository_map(&self) -> Result<String, ErrorData> { render(&domain::repository_map()) }

    #[tool(description = "Return the organization-specific domain and compatibility contract. No live data is read.")]
    fn domain_contract(&self) -> Result<String, ErrorData> { render(&domain::domain_contract()) }

    #[tool(description = "Validate a bounded organization-specific diagnostic plan. This tool never executes, writes, publishes, or contacts a service.")]
    fn $validator_tool(&self, Parameters(request): Parameters<$validator_request>) -> Result<String, ErrorData> {
        $rust_validation
    }

    #[tool(description = "Return the fixed read-only safety boundary and prohibited capabilities for this MCP server.")]
    fn safety_contract(&self) -> Result<String, ErrorData> { render(&domain::safety_contract()) }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for $server_type {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("$binary_name", env!("CARGO_PKG_VERSION")))
            .with_instructions("$description. All tools are read-only, offline, bounded, and credential-free.")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_catalog_is_closed_and_read_only() {
        let server = $server_type::new();
        assert_eq!(server.tool_router.map.len(), 5);
        for route in server.tool_router.map.values() {
            assert!(route.attr.description.as_deref().is_some_and(|value| !value.is_empty()));
            let annotation = route.attr.annotations.as_ref().expect("annotations");
            assert_eq!(annotation.read_only_hint, Some(true));
            assert_eq!(annotation.destructive_hint, Some(false));
            assert_eq!(annotation.idempotent_hint, Some(true));
            assert_eq!(annotation.open_world_hint, Some(false));
            for forbidden in ["publish", "tag", "write", "delete", "execute", "create", "update"] {
                assert!(!route.attr.name.contains(forbidden), "forbidden tool name: {}", route.attr.name);
            }
        }
    }

    #[test]
    fn normal_domain_payloads_fit_the_shared_output_bound() {
        for value in [domain::project_overview(), domain::repository_map(), domain::domain_contract(), domain::safety_contract()] {
            assert!(render(&value).is_ok());
        }
    }
}
''')

PROCESS_TEST_TEMPLATE = Template(r'''use std::{
    io::{BufRead, BufReader, Read, Write},
    process::{Command, Stdio},
    sync::{mpsc, Arc, Mutex},
    thread,
    time::{Duration, Instant},
};
use serde_json::{json, Value};

fn send(stdin: &mut impl Write, value: &Value) {
    serde_json::to_writer(&mut *stdin, value).expect("serialize request");
    stdin.write_all(b"\n").expect("write newline");
    stdin.flush().expect("flush request");
}

fn receive(receiver: &mpsc::Receiver<String>, frames: &Arc<Mutex<Vec<String>>>) -> Value {
    let line = receiver.recv_timeout(Duration::from_secs(15)).expect("server response");
    frames.lock().unwrap().push(line.clone());
    serde_json::from_str(&line).expect("one JSON protocol frame")
}

#[test]
fn real_binary_satisfies_stdio_and_domain_contracts() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_$binary_name"))
        .env("RUST_LOG", "warn")
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped())
        .spawn().expect("spawn MCP server");
    let mut stdin = child.stdin.take().expect("stdin");
    let stdout = child.stdout.take().expect("stdout");
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            match line { Ok(line) if sender.send(line).is_ok() => {}, _ => break }
        }
    });
    let frames = Arc::new(Mutex::new(Vec::<String>::new()));

    send(&mut stdin, &json!({"jsonrpc":"2.0","id":"init-string","method":"initialize","params":{"protocolVersion":"$protocol","capabilities":{},"clientInfo":{"name":"fleet-test","version":"1"}}}));
    let initialized = receive(&receiver, &frames);
    assert_eq!(initialized["id"], "init-string");
    assert_eq!(initialized["result"]["protocolVersion"], "$protocol");

    send(&mut stdin, &json!({"jsonrpc":"2.0","method":"notifications/initialized","params":{}}));
    assert!(matches!(receiver.recv_timeout(Duration::from_millis(200)), Err(mpsc::RecvTimeoutError::Timeout)));

    send(&mut stdin, &json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}));
    let tools = receive(&receiver, &frames);
    assert_eq!(tools["id"], 2);
    let mut names = Vec::new();
    for tool in tools["result"]["tools"].as_array().expect("tools") {
        names.push(tool["name"].as_str().unwrap().to_owned());
        assert!(tool["description"].as_str().is_some_and(|value| !value.is_empty()));
        assert_eq!(tool["inputSchema"]["type"], "object");
        assert_eq!(tool["annotations"]["readOnlyHint"], true);
        assert_eq!(tool["annotations"]["destructiveHint"], false);
        assert_eq!(tool["annotations"]["idempotentHint"], true);
        assert_eq!(tool["annotations"]["openWorldHint"], false);
    }
    names.sort();
    assert_eq!(names, $tool_names);

    send(&mut stdin, &json!({"jsonrpc":"2.0","id":"domain","method":"tools/call","params":{"name":"$validator_tool","arguments":$valid_arguments}}));
    let valid = receive(&receiver, &frames);
    assert_eq!(valid["id"], "domain");
    assert_eq!(valid["result"]["isError"], false);
    let text = valid["result"]["content"][0]["text"].as_str().expect("text");
    let payload: Value = serde_json::from_str(text).expect("bounded JSON result");
    assert_eq!(payload["accepted"], true);

    let mut invalid_arguments = json!($valid_arguments);
    invalid_arguments["$forbidden_key"] = json!($forbidden_value);
    send(&mut stdin, &json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"$validator_tool","arguments":invalid_arguments}}));
    let invalid = receive(&receiver, &frames);
    assert_eq!(invalid["id"], 4);
    assert!(invalid.get("error").is_some() || invalid["result"]["isError"] == true);

    send(&mut stdin, &json!({"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"safety_contract","arguments":{}}}));
    let recovered = receive(&receiver, &frames);
    assert_eq!(recovered["id"], 5);
    assert_eq!(recovered["result"]["isError"], false);

    drop(stdin);
    let deadline = Instant::now() + Duration::from_secs(5);
    let status = loop {
        if let Some(status) = child.try_wait().expect("try wait") { break status; }
        if Instant::now() >= deadline { let _ = child.kill(); panic!("server did not exit after EOF"); }
        thread::sleep(Duration::from_millis(25));
    };
    assert!(status.success());

    let joined = frames.lock().unwrap().join("\n") + "\n";
    let audit = ore_mcp_testkit::audit_stdio_stdout(joined.as_bytes()).expect("stdout purity");
    assert_eq!(audit.notification_count, 0);
    assert!(audit.response_count >= 5);

    let mut stderr = String::new();
    child.stderr.take().unwrap().read_to_string(&mut stderr).unwrap();
    assert!(!stderr.contains("2024-11-05"));
    assert!(!stderr.to_ascii_lowercase().contains("token="));
}
''')

CI_TEMPLATE = Template(r'''name: ci

on:
  push:
  pull_request:
  workflow_dispatch:

permissions:
  contents: read

concurrency:
  group: $${{ github.workflow }}-$${{ github.ref }}
  cancel-in-progress: true

jobs:
  validate:
    runs-on: ubuntu-24.04
    timeout-minutes: 35
    steps:
      - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7
        with:
          persist-credentials: false
      - name: Validate workflow syntax
        uses: docker://rhysd/actionlint@sha256:b1934ee5f1c509618f2508e6eb47ee0d3520686341fec936f3b79331f9315667
        with:
          args: .github/workflows/ci.yml
      - name: Require deterministic dependency state
        shell: bash
        run: |
          set -euo pipefail
          test -f Cargo.lock
          ! git check-ignore -q Cargo.lock
          python3 - <<'PY'
          import pathlib, tomllib
          manifest = tomllib.loads(pathlib.Path('Cargo.toml').read_text())
          assert manifest['dependencies']['rmcp']['version'] == '=$rmcp_version'
          assert manifest['dependencies']['ore-mcp-safety']['rev'] == '$shared_revision'
          lock = tomllib.loads(pathlib.Path('Cargo.lock').read_text())
          versions = {p['name']: p['version'] for p in lock['package'] if p['name'] in {'rmcp','rmcp-macros'}}
          assert versions == {'rmcp':'$rmcp_version','rmcp-macros':'$rmcp_version'}, versions
          shared = {p['name']: p.get('source','') for p in lock['package'] if p['name'] in {'ore-mcp-safety','ore-mcp-testkit'}}
          assert shared and all('$shared_revision' in source for source in shared.values()), shared
          PY
      - uses: dtolnay/rust-toolchain@4be7066ada62dd38de10e7b70166bc74ed198c30
        with:
          toolchain: '$rust_version'
          components: rustfmt, clippy
      - uses: Swatinem/rust-cache@e18b497796c12c097a38f9edb9d0641fb99eee32 # v2
      - name: Format
        run: cargo fmt --all -- --check
      - name: Strict Clippy
        run: cargo clippy --locked --all-targets -- -D warnings
      - name: Unit and real-process protocol tests
        run: cargo test --locked --all-targets
      - name: Documentation
        env:
          RUSTDOCFLAGS: -D warnings
        run: cargo doc --locked --no-deps
      - name: Release build
        run: cargo build --locked --release
      - name: Enforce read-only offline surface
        shell: bash
        run: |
          set -euo pipefail
          if git grep -nE '\b(print|println)!\s*\(' -- src; then
            echo 'stdout application writes are forbidden' >&2; exit 1
          fi
          if git grep -nE 'reqwest|tokio::process|std::process::Command|Command::new|File::create|OpenOptions' -- src; then
            echo 'network, subprocess, or filesystem mutation surface detected' >&2; exit 1
          fi
          git grep -q 'ToolAnnotations::new()' -- src/server.rs
          git grep -q 'read_only(true)' -- src/server.rs
          git grep -q 'CARGO_BIN_EXE_$binary_name' -- tests/stdio_protocol.rs
          git grep -q '$protocol' -- tests/stdio_protocol.rs
          git diff --check
      - uses: taiki-e/install-action@c44f6b046f1c29ae5918b1e0bfdbb2f1813836fd # v2
        with:
          tool: cargo-audit@0.22.2
      - name: Dependency audit
        run: cargo audit --deny warnings

  msrv:
    runs-on: ubuntu-24.04
    timeout-minutes: 25
    steps:
      - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7
        with:
          persist-credentials: false
      - uses: dtolnay/rust-toolchain@4be7066ada62dd38de10e7b70166bc74ed198c30
        with:
          toolchain: '$msrv'
      - uses: Swatinem/rust-cache@e18b497796c12c097a38f9edb9d0641fb99eee32 # v2
      - run: cargo check --locked --all-targets
''')

README_TEMPLATE = Template(r'''# $server_title

$description.

## Safety boundary

This server is **read-only, offline, bounded, and credential-free**. It does not
read clipboard/audio/content records, connect to databases or registries, spawn
processes, write files, publish packages, create tags, or mutate remote services.
Stdout is reserved exclusively for MCP JSON-RPC frames; diagnostics use stderr.

## Tools

- `project_overview`
- `repository_map`
- `domain_contract`
- `$validator_tool`
- `safety_contract`

The typed validator accepts metadata/planning inputs only and uses
`#[serde(deny_unknown_fields)]` so raw content, credentials, and hidden mutation
arguments fail closed.

## Shared implementation

Generic bounds and stdio conformance auditing are consumed from
`ORESoftware/mcp-rust-libs` at immutable revision `$shared_revision`. Domain
schemas and policy remain local to this repository.

## Validate

```sh
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
RUSTDOCFLAGS='-D warnings' cargo doc --locked --no-deps
cargo build --locked --release
cargo audit --deny warnings
```

Tracking: `$issue`.
''')

AGENTS_TEMPLATE = Template(r'''# Agent contract

- Preserve stdout for MCP protocol frames only.
- Keep the tool surface read-only and offline unless a separately reviewed issue
  adds an exact authorization, confirmation, idempotency, and denial contract.
- Do not accept credentials, raw private content, database URLs, registry tokens,
  or path traversal.
- Keep `rmcp` pinned exactly to `$rmcp_version` and shared crates pinned to
  `$shared_revision` until a reviewed migration updates tests and lockfile.
- Every product change enters through a feature branch and pull request.
- Run the full locked format, Clippy, test, Rustdoc, release, and audit matrix.
''')
