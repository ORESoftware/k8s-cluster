#!/usr/bin/env python3
from __future__ import annotations

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


def clients_files(org: Mapping[str, Any], repo: Mapping[str, Any]) -> dict[str, str]:
    namespace = rust_ident(str(org["prefix"]))
    java_type = rust_type(str(org["product"])) + "Client"
    files = common_files(org, repo)
    files.update(
        {
            "README.md": f"""# {repo['name']}

Cross-language client contract seed for {org['product']}.

Supported roots: Rust, TypeScript, Dart, Go, Gleam, Java, Swift, and WASM/WIT. Every client centralizes base-URL validation and health/repository endpoint construction; authentication and generated models should be added from the interfaces package without changing these roots.
""",
            ".zpkg.toml": f"""[package]
name = "{slug(str(repo['name']))}"
version = "0.1.0"
license = "UNLICENSED"

[exports]
clients = "clients"
""",
            ".zpkg.lock": "version = 1\npackages = []\n",
            "clients/rust/Cargo.toml": f"""[package]
name = "{namespace}-client"
version = "0.1.0"
edition = "2021"
rust-version = "1.85"
publish = false

[lints.rust]
unsafe_code = "forbid"
""",
            "clients/rust/src/lib.rs": """#![forbid(unsafe_code)]

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Client {
    base_url: String,
}

impl Client {
    pub fn new(base_url: impl Into<String>) -> Result<Self, &'static str> {
        let base_url = base_url.into().trim_end_matches('/').to_owned();
        if !(base_url.starts_with("http://") || base_url.starts_with("https://")) {
            return Err("base URL must use http or https");
        }
        Ok(Self { base_url })
    }

    pub fn health_url(&self) -> String {
        format!("{}/healthz", self.base_url)
    }

    pub fn repositories_url(&self) -> String {
        format!("{}/v1/repositories", self.base_url)
    }
}

#[cfg(test)]
mod tests {
    use super::Client;

    #[test]
    fn normalizes_endpoint_paths() {
        let client = Client::new("https://example.test/").expect("valid URL");
        assert_eq!(client.health_url(), "https://example.test/healthz");
    }
}
""",
            "clients/typescript/package.json": f"""{{
  "name": "@oresoftware/{slug(str(repo['name']))}-typescript",
  "version": "0.1.0",
  "private": true,
  "type": "module",
  "exports": "./src/index.ts",
  "scripts": {{
    "typecheck": "tsc --noEmit"
  }}
}}
""",
            "clients/typescript/tsconfig.json": """{
  "compilerOptions": {
    "target": "ES2022",
    "module": "NodeNext",
    "moduleResolution": "NodeNext",
    "strict": true,
    "noEmit": true
  },
  "include": ["src/**/*.ts"]
}
""",
            "clients/typescript/src/index.ts": """export class Client {
  readonly baseUrl: URL;

  constructor(baseUrl: string) {
    const parsed = new URL(baseUrl);
    if (parsed.protocol !== "http:" && parsed.protocol !== "https:") {
      throw new Error("base URL must use http or https");
    }
    this.baseUrl = parsed;
  }

  healthUrl(): URL { return new URL("/healthz", this.baseUrl); }
  repositoriesUrl(): URL { return new URL("/v1/repositories", this.baseUrl); }
}
""",
            "clients/dart/pubspec.yaml": f"""name: {namespace}_client
version: 0.1.0
publish_to: none
environment:
  sdk: '>=3.5.0 <4.0.0'
""",
            "clients/dart/lib/client.dart": """class Client {
  Client(String baseUrl) : baseUrl = Uri.parse(baseUrl) {
    if (!{'http', 'https'}.contains(this.baseUrl.scheme)) {
      throw ArgumentError('base URL must use http or https');
    }
  }

  final Uri baseUrl;
  Uri get healthUrl => baseUrl.resolve('/healthz');
  Uri get repositoriesUrl => baseUrl.resolve('/v1/repositories');
}
""",
            "clients/go/go.mod": f"module github.com/{org['owner']}/{repo['name']}/clients/go\n\ngo 1.23\n",
            "clients/go/client.go": """package client

import (
    "fmt"
    "net/url"
)

type Client struct { BaseURL *url.URL }

func New(raw string) (*Client, error) {
    parsed, err := url.Parse(raw)
    if err != nil || (parsed.Scheme != "http" && parsed.Scheme != "https") {
        return nil, fmt.Errorf("base URL must use http or https")
    }
    return &Client{BaseURL: parsed}, nil
}

func (c *Client) HealthURL() string { return c.BaseURL.ResolveReference(&url.URL{Path: "/healthz"}).String() }
func (c *Client) RepositoriesURL() string { return c.BaseURL.ResolveReference(&url.URL{Path: "/v1/repositories"}).String() }
""",
            "clients/gleam/gleam.toml": f"name = \"{namespace}_client\"\nversion = \"0.1.0\"\ntarget = \"erlang\"\n",
            "clients/gleam/src/client.gleam": """pub fn health_path() -> String { "/healthz" }
pub fn repositories_path() -> String { "/v1/repositories" }
""",
            "clients/java/pom.xml": f"""<?xml version="1.0" encoding="UTF-8"?>
<project xmlns="http://maven.apache.org/POM/4.0.0"
         xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"
         xsi:schemaLocation="http://maven.apache.org/POM/4.0.0 https://maven.apache.org/xsd/maven-4.0.0.xsd">
  <modelVersion>4.0.0</modelVersion>
  <groupId>dev.oresoftware.{namespace}</groupId>
  <artifactId>{slug(str(repo['name']))}-java</artifactId>
  <version>0.1.0</version>
  <properties>
    <maven.compiler.release>17</maven.compiler.release>
    <project.build.sourceEncoding>UTF-8</project.build.sourceEncoding>
  </properties>
</project>
""",
            f"clients/java/src/main/java/dev/oresoftware/{namespace}/{java_type}.java": f"""package dev.oresoftware.{namespace};

import java.net.URI;

public final class {java_type} {{
  private final URI baseUri;

  public {java_type}(String baseUrl) {{
    this.baseUri = URI.create(baseUrl);
    String scheme = this.baseUri.getScheme();
    if (!"http".equals(scheme) && !"https".equals(scheme)) {{
      throw new IllegalArgumentException("base URL must use http or https");
    }}
  }}

  public URI healthUrl() {{ return this.baseUri.resolve("/healthz"); }}
  public URI repositoriesUrl() {{ return this.baseUri.resolve("/v1/repositories"); }}
}}
""",
            "clients/swift/Package.swift": f"""// swift-tools-version: 5.9
import PackageDescription

let package = Package(
    name: "{rust_type(str(org['product']))}Client",
    products: [.library(name: "Client", targets: ["Client"])],
    targets: [.target(name: "Client")]
)
""",
            "clients/swift/Sources/Client/Client.swift": """import Foundation

public struct Client {
    public let baseURL: URL

    public init(baseURL: URL) throws {
        guard baseURL.scheme == "http" || baseURL.scheme == "https" else {
            throw URLError(.unsupportedURL)
        }
        self.baseURL = baseURL
    }

    public var healthURL: URL { URL(string: "/healthz", relativeTo: baseURL)! }
    public var repositoriesURL: URL { URL(string: "/v1/repositories", relativeTo: baseURL)! }
}
""",
            "clients/wasm/world.wit": """package oresoftware:client@0.1.0;

interface endpoints {
  health-path: func() -> string;
  repositories-path: func() -> string;
}

world client {
  export endpoints;
}
""",
            ".github/workflows/ci.yml": python_ci(
                "python3 -m unittest discover -s tests -p 'test_*.py' -v && cargo test --manifest-path clients/rust/Cargo.toml && (cd clients/go && go test ./...)"
            ),
            "tests/test_layout.py": """import pathlib
import unittest


class ClientLayoutTest(unittest.TestCase):
    def test_required_language_roots_exist(self) -> None:
        expected = {"rust", "typescript", "dart", "go", "gleam", "java", "swift", "wasm"}
        observed = {path.name for path in pathlib.Path("clients").iterdir() if path.is_dir()}
        self.assertEqual(observed, expected)

    def test_clients_expose_health_and_repository_paths(self) -> None:
        text = "\\n".join(
            path.read_text(encoding="utf-8")
            for path in pathlib.Path("clients").rglob("*")
            if path.is_file()
        )
        self.assertIn("/healthz", text)
        self.assertIn("/v1/repositories", text)


if __name__ == "__main__":
    unittest.main()
""",
        }
    )
    return files
