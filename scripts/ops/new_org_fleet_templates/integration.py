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


def e2e_files(org: Mapping[str, Any], repo: Mapping[str, Any]) -> dict[str, str]:
    repositories = [item["full_name"] for item in relationship_document(org)["repositories"]]
    files = common_files(org, repo)
    files.update(
        {
            "README.md": f"""# {repo['name']}

Cross-repository contract and browser-test home for {org['product']}.

`tests/contract` runs on every change. Browser suites are separated under Playwright, Puppeteer, and Selenium so product-specific journeys can be added without coupling frameworks.
""",
            "fixtures/repositories.json": json_text({"repositories": repositories}),
            "tests/browser/playwright/README.md": "Add Playwright journeys here; each test must use an explicit BASE_URL and bounded timeout.\n",
            "tests/browser/puppeteer/README.md": "Add Puppeteer journeys here; each test must use an explicit BASE_URL and bounded timeout.\n",
            "tests/browser/selenium/README.md": "Add Selenium journeys here; each test must use an explicit BASE_URL and bounded timeout.\n",
            "tests/test_contract.py": """import json
import pathlib
import unittest


class FleetContractTest(unittest.TestCase):
    def test_repository_fixture_is_unique_and_qualified(self) -> None:
        document = json.loads(pathlib.Path("fixtures/repositories.json").read_text(encoding="utf-8"))
        repositories = document["repositories"]
        self.assertEqual(len(repositories), len(set(repositories)))
        self.assertGreaterEqual(len(repositories), 8)
        self.assertTrue(all(name.count("/") == 1 for name in repositories))

    def test_browser_framework_boundaries_exist(self) -> None:
        for framework in ("playwright", "puppeteer", "selenium"):
            self.assertTrue((pathlib.Path("tests/browser") / framework).is_dir())


if __name__ == "__main__":
    unittest.main()
""",
            ".github/workflows/ci.yml": python_ci(),
        }
    )
    return files


def mcp_files(org: Mapping[str, Any], repo: Mapping[str, Any]) -> dict[str, str]:
    package = slug(str(repo["name"])).removesuffix("-rs")
    crate_name = rust_ident(package)
    type_name = rust_type(str(org["product"])) + "Mcp"
    map_json = json_text(relationship_document(org)).rstrip()
    files = common_files(org, repo)
    files.update(
        {
            "README.md": f"""# {repo['name']}

Read-only stdio MCP server for {org['product']} organization and repository metadata.

Tools:

- `org_map` — canonical repository roles and relationships;
- `list_repositories` — bounded repository list;
- `health` — static readiness metadata.

The initial tool surface performs no shell execution, network calls, filesystem writes, or mutations.
""",
            "Cargo.toml": f"""[package]
name = "{package}"
version = "0.1.0"
edition = "2021"
rust-version = "1.88"
description = "Read-only MCP server for {org['product']} organization metadata"
license = "UNLICENSED"
publish = false
repository = "https://github.com/{org['owner']}/{repo['name']}"

[dependencies]
anyhow = "1"
rmcp = {{ version = "=3.1.0", features = ["server", "macros", "schemars", "transport-io"] }}
schemars = "1"
serde = {{ version = "1", features = ["derive"] }}
serde_json = "1"
tokio = {{ version = "1", features = ["macros", "rt-multi-thread"] }}

[lints.rust]
unsafe_code = "forbid"

[lints.clippy]
all = {{ level = "warn", priority = -1 }}
module_name_repetitions = "allow"
""",
            "src/lib.rs": f"""//! Read-only {org['product']} MCP server library.

#![forbid(unsafe_code)]

pub mod payloads;
pub mod runtime;
pub mod server;

pub use server::{type_name};
""",
            "src/main.rs": f"""//! Stdio bootstrap for the read-only {org['product']} MCP server.

#[tokio::main]
async fn main() -> anyhow::Result<()> {{
    {crate_name}::runtime::run_stdio().await
}}
""",
            "src/runtime.rs": f"""//! Stdio transport lifecycle. Stdout is reserved for protocol frames.

use rmcp::{{transport::stdio, ServiceExt}};

use crate::server::{type_name};

pub async fn run_stdio() -> anyhow::Result<()> {{
    let service = {type_name}::default().serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}}
""",
            "src/payloads.rs": f'''//! Deterministic organization payloads.

use serde_json::{{json, Value}};

const ORGANIZATION_MAP: &str = r#"{map_json}"#;

pub fn org_map() -> Result<Value, String> {{
    serde_json::from_str(ORGANIZATION_MAP).map_err(|error| error.to_string())
}}

pub fn repositories() -> Result<Value, String> {{
    let document = org_map()?;
    let repositories = document.get("repositories").cloned().ok_or("repository map is missing repositories")?;
    Ok(json!({{"repositories": repositories}}))
}}

pub fn health() -> Value {{
    json!({{"status": "ok", "service": "{package}", "version": "0.1.0", "read_only": true}})
}}

#[cfg(test)]
mod tests {{
    use super::*;

    #[test]
    fn payload_is_bounded_and_contains_mcp_repository() {{
        let value = org_map().unwrap();
        let repositories = value["repositories"].as_array().unwrap();
        assert!(repositories.len() >= 8);
        assert!(repositories.iter().any(|item| item["role"] == "mcp"));
    }}
}}
''',
            "src/server.rs": f"""//! Typed read-only MCP tool surface.

use rmcp::{{tool, tool_handler, tool_router, ServerHandler}};

use crate::payloads;

#[derive(Clone, Default)]
pub struct {type_name};

fn render(value: serde_json::Value) -> Result<String, String> {{
    serde_json::to_string_pretty(&value).map_err(|error| error.to_string())
}}

#[tool_router]
impl {type_name} {{
    #[tool(description = "Describe the organization, repository responsibilities, dependencies, and reverse dependencies. Static and read-only.")]
    fn org_map(&self) -> Result<String, String> {{ render(payloads::org_map()?) }}

    #[tool(description = "List the bounded canonical repository fleet for this organization. Static and read-only.")]
    fn list_repositories(&self) -> Result<String, String> {{ render(payloads::repositories()?) }}

    #[tool(description = "Return static MCP server readiness metadata. No external network call is performed.")]
    fn health(&self) -> Result<String, String> {{ render(payloads::health()) }}
}}

#[tool_handler(
    name = "{package}",
    version = "0.1.0",
    instructions = "Read-only organization visibility. Tools perform no shell execution, network calls, filesystem writes, credential reads, or state mutation."
)]
impl ServerHandler for {type_name} {{}}
""",
            ".github/workflows/ci.yml": rust_ci(MCP_RUST_TOOLCHAIN, locked=False),
        }
    )
    return files
