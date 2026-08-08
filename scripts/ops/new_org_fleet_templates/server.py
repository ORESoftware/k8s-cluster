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


def server_files(org: Mapping[str, Any], repo: Mapping[str, Any]) -> dict[str, str]:
    package = slug(str(repo["name"]))
    crate_name = rust_ident(package)
    relationship_json = json_text(relationship_document(org)).rstrip()
    env_prefix = re.sub(r"[^A-Z0-9]+", "_", str(org["prefix"]).upper())
    files = common_files(org, repo)
    files.update(
        {
            "README.md": f"""# {repo['name']}

Bounded dependency-free Rust HTTP server for {org['product']}.

Endpoints: `/healthz`, `/readyz`, `/v1/repositories`, and `/metrics`. The bootstrap server accepts only GET/HEAD, caps request headers at 16 KiB, applies read/write timeouts, emits security headers, and closes each connection after one response.
""",
            "Cargo.toml": f"""[package]
name = "{package}"
version = "0.1.0"
edition = "2021"
rust-version = "1.85"
publish = false

[lib]
name = "{crate_name}"

[lints.rust]
unsafe_code = "forbid"

[lints.clippy]
all = {{ level = "warn", priority = -1 }}
""",
            "Cargo.lock": simple_cargo_lock(package),
            "src/lib.rs": f'''#![forbid(unsafe_code)]

use std::str;

pub const MAX_REQUEST_BYTES: usize = 16 * 1024;
pub const REPOSITORY_MAP: &str = r#"{relationship_json}"#;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Method {{ Get, Head }}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RequestError {{ InvalidUtf8, Incomplete, UnsupportedMethod, InvalidTarget }}

pub fn parse_request(request: &[u8]) -> Result<(Method, &str), RequestError> {{
    if !request.windows(4).any(|window| window == b"\r\n\r\n") {{ return Err(RequestError::Incomplete); }}
    let text = str::from_utf8(request).map_err(|_| RequestError::InvalidUtf8)?;
    let line = text.lines().next().ok_or(RequestError::Incomplete)?;
    let mut fields = line.split_whitespace();
    let method = match fields.next() {{ Some("GET") => Method::Get, Some("HEAD") => Method::Head, _ => return Err(RequestError::UnsupportedMethod) }};
    let target = fields.next().ok_or(RequestError::InvalidTarget)?;
    if fields.next() != Some("HTTP/1.1") || !target.starts_with('/') || target.contains("..") {{ return Err(RequestError::InvalidTarget); }}
    Ok((method, target))
}}

pub fn response_for(method: Method, target: &str) -> Vec<u8> {{
    let (status, content_type, body) = match target.split('?').next().unwrap_or(target) {{
        "/healthz" | "/readyz" => ("200 OK", "application/json", r#"{{"status":"ok","service":"{package}","version":"0.1.0"}}"#),
        "/v1/repositories" => ("200 OK", "application/json", REPOSITORY_MAP),
        "/metrics" => ("200 OK", "text/plain; version=0.0.4", "{crate_name}_up 1\n"),
        _ => ("404 Not Found", "application/json", r#"{{"error":"not_found"}}"#),
    }};
    let body_bytes = if method == Method::Head {{ &[][..] }} else {{ body.as_bytes() }};
    let headers = format!(
        "HTTP/1.1 {{status}}\r\nContent-Type: {{content_type}}\r\nContent-Length: {{}}\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nContent-Security-Policy: default-src 'none'\r\nConnection: close\r\n\r\n",
        body_bytes.len()
    );
    let mut response = headers.into_bytes();
    response.extend_from_slice(body_bytes);
    response
}}

pub fn error_response(error: RequestError) -> Vec<u8> {{
    let status = match error {{ RequestError::UnsupportedMethod => "405 Method Not Allowed", _ => "400 Bad Request" }};
    let body = r#"{{"error":"bad_request"}}"#;
    format!("HTTP/1.1 {{status}}\r\nContent-Type: application/json\r\nContent-Length: {{}}\r\nConnection: close\r\n\r\n{{body}}", body.len()).into_bytes()
}}

#[cfg(test)]
mod tests {{
    use super::*;

    #[test]
    fn parses_bounded_get_and_head_requests() {{
        assert_eq!(parse_request(b"GET /healthz HTTP/1.1\r\nHost: x\r\n\r\n"), Ok((Method::Get, "/healthz")));
        assert_eq!(parse_request(b"HEAD /metrics HTTP/1.1\r\nHost: x\r\n\r\n"), Ok((Method::Head, "/metrics")));
    }}

    #[test]
    fn rejects_mutating_methods_and_traversal() {{
        assert_eq!(parse_request(b"POST /healthz HTTP/1.1\r\n\r\n"), Err(RequestError::UnsupportedMethod));
        assert_eq!(parse_request(b"GET /../secret HTTP/1.1\r\n\r\n"), Err(RequestError::InvalidTarget));
    }}

    #[test]
    fn health_response_has_security_headers() {{
        let response = String::from_utf8(response_for(Method::Get, "/healthz")).unwrap();
        assert!(response.starts_with("HTTP/1.1 200 OK"));
        assert!(response.contains("X-Content-Type-Options: nosniff"));
        assert!(response.ends_with(r#"{{"status":"ok","service":"{package}","version":"0.1.0"}}"#));
    }}
}}
''',
            "src/main.rs": f'''#![forbid(unsafe_code)]

use std::env;
use std::io::{{Read, Write}};
use std::net::{{TcpListener, TcpStream}};
use std::time::Duration;

use {crate_name}::{{error_response, parse_request, response_for, MAX_REQUEST_BYTES}};

fn main() {{
    if let Err(error) = run() {{
        eprintln!("{package} failed: {{error}}");
        std::process::exit(1);
    }}
}}

fn run() -> std::io::Result<()> {{
    let address = env::var("{env_prefix}_BIND_ADDR").unwrap_or_else(|_| "127.0.0.1:8080".to_owned());
    let listener = TcpListener::bind(&address)?;
    eprintln!("{package} listening on {{address}}");
    for stream in listener.incoming() {{
        match stream.and_then(handle_connection) {{
            Ok(()) => {{}},
            Err(error) => eprintln!("connection failed: {{error}}"),
        }}
    }}
    Ok(())
}}

fn handle_connection(mut stream: TcpStream) -> std::io::Result<()> {{
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    let mut request = Vec::with_capacity(1024);
    let mut buffer = [0_u8; 1024];
    loop {{
        let read = stream.read(&mut buffer)?;
        if read == 0 {{ break; }}
        request.extend_from_slice(&buffer[..read]);
        if request.len() > MAX_REQUEST_BYTES {{ break; }}
        if request.windows(4).any(|window| window == b"\r\n\r\n") {{ break; }}
    }}
    let response = if request.len() > MAX_REQUEST_BYTES {{
        b"HTTP/1.1 431 Request Header Fields Too Large\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec()
    }} else {{
        match parse_request(&request) {{ Ok((method, target)) => response_for(method, target), Err(error) => error_response(error) }}
    }};
    stream.write_all(&response)?;
    stream.flush()
}}
''',
            "Dockerfile": f"""FROM rust:1.85.0-bookworm AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release --locked

FROM gcr.io/distroless/cc-debian12:nonroot
COPY --from=build /src/target/release/{package} /usr/local/bin/{package}
USER nonroot:nonroot
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/{package}"]
""",
            ".github/workflows/ci.yml": rust_ci(),
        }
    )
    return files
