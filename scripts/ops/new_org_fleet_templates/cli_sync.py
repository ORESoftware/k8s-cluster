#!/usr/bin/env python3
from __future__ import annotations

import re
from collections.abc import Mapping
from typing import Any

from .core import (
    MCP_RUST_TOOLCHAIN,
    RUST_TOOLCHAIN,
    _all_repository_entries,
    common_files,
    json_text,
    python_ci,
    relationship_document,
    rust_ci,
    rust_ident,
    rust_type,
    simple_cargo_lock,
    slug,
)


def cli_files(org: Mapping[str, Any], repo: Mapping[str, Any]) -> dict[str, str]:
    package = slug(str(repo["name"]))
    env_prefix = re.sub(r"[^A-Z0-9]+", "_", str(org["prefix"]).upper())
    files = common_files(org, repo)
    files.update(
        {
            "README.md": f"""# {repo['name']}

Dependency-free Rust CLI for inspecting {org['product']} repository relationships and health endpoints.

```text
{package} describe
{package} repositories
{package} health --base-url http://127.0.0.1:8080
```

Flags are mirrored to environment variables through `.cli-flags.toml`.
""",
            "Cargo.toml": f"""[package]
name = "{package}"
version = "0.1.0"
edition = "2021"
rust-version = "1.85"
publish = false

[lints.rust]
unsafe_code = "forbid"

[lints.clippy]
all = {{ level = "warn", priority = -1 }}
""",
            "Cargo.lock": simple_cargo_lock(package),
            ".cli-flags.toml": f"""[flags.base-url]
env = "{env_prefix}_BASE_URL"
default = "http://127.0.0.1:8080"

[flags.format]
env = "{env_prefix}_OUTPUT_FORMAT"
default = "json"
""",
            "src/main.rs": f"""#![forbid(unsafe_code)]

use std::env;
use std::io::{{Read, Write}};
use std::net::TcpStream;
use std::time::Duration;

const RELATIONSHIPS: &str = include_str!("../repo-relationships.json");

fn main() {{
    if let Err(error) = run() {{
        eprintln!("{package}: {{error}}");
        std::process::exit(1);
    }}
}}

fn run() -> Result<(), String> {{
    let mut args = env::args().skip(1).peekable();
    let command = args.next().unwrap_or_else(|| "describe".to_owned());
    let mut base_url = env::var("{env_prefix}_BASE_URL")
        .unwrap_or_else(|_| "http://127.0.0.1:8080".to_owned());
    while let Some(argument) = args.next() {{
        match argument.as_str() {{
            "--base-url" => base_url = args.next().ok_or("--base-url requires a value")?,
            unknown => return Err(format!("unknown argument: {{unknown}}")),
        }}
    }}

    match command.as_str() {{
        "describe" | "repositories" => {{ print!("{{RELATIONSHIPS}}"); Ok(()) }}
        "health" => health(&base_url),
        _ => Err(format!("unknown command: {{command}}")),
    }}
}}

fn health(base_url: &str) -> Result<(), String> {{
    let (host, port) = parse_http_base_url(base_url)?;
    let mut stream = TcpStream::connect((host.as_str(), port)).map_err(|error| error.to_string())?;
    stream.set_read_timeout(Some(Duration::from_secs(5))).map_err(|error| error.to_string())?;
    stream.set_write_timeout(Some(Duration::from_secs(5))).map_err(|error| error.to_string())?;
    let request = format!("GET /healthz HTTP/1.1\r\nHost: {{host}}\r\nConnection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).map_err(|error| error.to_string())?;
    let mut response = String::new();
    stream.read_to_string(&mut response).map_err(|error| error.to_string())?;
    let (headers, body) = response.split_once("\r\n\r\n").ok_or("invalid HTTP response")?;
    if !headers.starts_with("HTTP/1.1 200") {{
        return Err(format!("health endpoint returned: {{}}", headers.lines().next().unwrap_or("unknown")));
    }}
    println!("{{body}}");
    Ok(())
}}

fn parse_http_base_url(value: &str) -> Result<(String, u16), String> {{
    let authority = value.strip_prefix("http://").ok_or("only http:// URLs are supported by this bootstrap CLI")?;
    let authority = authority.split('/').next().unwrap_or(authority);
    let (host, port) = match authority.rsplit_once(':') {{
        Some((host, port)) => (host, port.parse::<u16>().map_err(|_| "invalid port")?),
        None => (authority, 80),
    }};
    if host.is_empty() {{ return Err("base URL host is empty".to_owned()); }}
    Ok((host.to_owned(), port))
}}

#[cfg(test)]
mod tests {{
    use super::parse_http_base_url;

    #[test]
    fn parses_explicit_and_default_ports() {{
        assert_eq!(parse_http_base_url("http://localhost:8080").unwrap(), ("localhost".to_owned(), 8080));
        assert_eq!(parse_http_base_url("http://example.test/path").unwrap(), ("example.test".to_owned(), 80));
    }}

    #[test]
    fn rejects_https_until_tls_is_explicitly_supported() {{
        assert!(parse_http_base_url("https://example.test").is_err());
    }}
}}
""",
            ".github/workflows/ci.yml": rust_ci(),
        }
    )
    return files


def sync_files(org: Mapping[str, Any], repo: Mapping[str, Any]) -> dict[str, str]:
    package = slug(str(repo["name"]))
    files = common_files(org, repo)
    files.update(
        {
            "README.md": f"""# {repo['name']}

Bounded, deterministic synchronization primitives for {org['product']}.

The initial crate enforces monotonic sequence numbers, bounded payloads, bounded retention, and idempotency keys. Persistence and transport adapters should remain outside the core state machine.
""",
            "Cargo.toml": f"""[package]
name = "{package}"
version = "0.1.0"
edition = "2021"
rust-version = "1.85"
publish = false

[lints.rust]
unsafe_code = "forbid"

[lints.clippy]
all = {{ level = "warn", priority = -1 }}
""",
            "Cargo.lock": simple_cargo_lock(package),
            "src/lib.rs": """#![forbid(unsafe_code)]

use std::collections::{HashSet, VecDeque};

pub const MAX_PAYLOAD_BYTES: usize = 64 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Event {
    pub sequence: u64,
    pub idempotency_key: String,
    pub payload: Vec<u8>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApplyOutcome {
    Applied,
    Duplicate,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ApplyError {
    EmptyKey,
    PayloadTooLarge,
    NonMonotonic { last: u64, incoming: u64 },
}

#[derive(Debug)]
pub struct EventLog {
    capacity: usize,
    last_sequence: u64,
    seen_keys: HashSet<String>,
    events: VecDeque<Event>,
}

impl EventLog {
    pub fn new(capacity: usize) -> Result<Self, &'static str> {
        if capacity == 0 { return Err("capacity must be positive"); }
        Ok(Self { capacity, last_sequence: 0, seen_keys: HashSet::new(), events: VecDeque::new() })
    }

    pub fn apply(&mut self, event: Event) -> Result<ApplyOutcome, ApplyError> {
        if event.idempotency_key.trim().is_empty() { return Err(ApplyError::EmptyKey); }
        if event.payload.len() > MAX_PAYLOAD_BYTES { return Err(ApplyError::PayloadTooLarge); }
        if self.seen_keys.contains(&event.idempotency_key) { return Ok(ApplyOutcome::Duplicate); }
        if event.sequence <= self.last_sequence {
            return Err(ApplyError::NonMonotonic { last: self.last_sequence, incoming: event.sequence });
        }

        self.last_sequence = event.sequence;
        self.seen_keys.insert(event.idempotency_key.clone());
        self.events.push_back(event);
        while self.events.len() > self.capacity {
            if let Some(removed) = self.events.pop_front() {
                self.seen_keys.remove(&removed.idempotency_key);
            }
        }
        Ok(ApplyOutcome::Applied)
    }

    pub fn events(&self) -> impl ExactSizeIterator<Item = &Event> { self.events.iter() }
    pub const fn last_sequence(&self) -> u64 { self.last_sequence }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(sequence: u64, key: &str) -> Event {
        Event { sequence, idempotency_key: key.to_owned(), payload: vec![1, 2, 3] }
    }

    #[test]
    fn applies_once_and_rejects_sequence_regression() {
        let mut log = EventLog::new(2).unwrap();
        assert_eq!(log.apply(event(1, "a")), Ok(ApplyOutcome::Applied));
        assert_eq!(log.apply(event(2, "a")), Ok(ApplyOutcome::Duplicate));
        assert_eq!(log.apply(event(1, "b")), Err(ApplyError::NonMonotonic { last: 1, incoming: 1 }));
    }

    #[test]
    fn retention_is_bounded() {
        let mut log = EventLog::new(2).unwrap();
        for sequence in 1..=3 { log.apply(event(sequence, &sequence.to_string())).unwrap(); }
        assert_eq!(log.events().len(), 2);
        assert_eq!(log.last_sequence(), 3);
    }
}
""",
            ".github/workflows/ci.yml": rust_ci(),
        }
    )
    return files
