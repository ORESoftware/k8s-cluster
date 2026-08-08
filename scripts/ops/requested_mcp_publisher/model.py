"""Exact repository allowlist and deterministic bootstrap content."""

from __future__ import annotations

import dataclasses
import re
from typing import Iterable

COMMIT_DATE = "2026-08-04T12:00:00Z"
COMMIT_AUTHOR_NAME = "ORESoftware repository bootstrap"
COMMIT_AUTHOR_EMAIL = "bot@oresoftware.dev"


@dataclasses.dataclass(frozen=True)
class RepositorySpec:
    owner: str
    name: str
    description: str
    private: bool
    product: str

    @property
    def full_name(self) -> str:
        return f"{self.owner}/{self.name}"

    @property
    def visibility(self) -> str:
        return "private" if self.private else "public"


REPOSITORIES: tuple[RepositorySpec, ...] = (
    RepositorySpec(
        owner="cliptown",
        name="cliptown-mcp-server.rs",
        description="Read-only Rust MCP server for ClipTown clipboard, vault, and device-sync contracts",
        private=False,
        product="ClipTown",
    ),
    RepositorySpec(
        owner="opto-sync",
        name="opto-sync-mcp-server.rs",
        description="Read-only Rust MCP server for Opto Sync reconciliation and conflict-policy contracts",
        private=False,
        product="Opto Sync",
    ),
    RepositorySpec(
        owner="voxletra",
        name="vxl-mcp-server.rs",
        description="Read-only Rust MCP server for Voxletra transcription, media, and sync contracts",
        private=True,
        product="Voxletra",
    ),
    RepositorySpec(
        owner="zed-pkg",
        name="zed-mcp-server.rs",
        description="Read-only Rust MCP server for Zed manifests, lockfiles, resolution, and provenance",
        private=False,
        product="Zed Package Manager",
    ),
    RepositorySpec(
        owner="zed-pkg-test",
        name="zed-pkg-test-mcp-server.rs",
        description="Read-only Rust MCP conformance server for Zed package fixtures and compatibility tests",
        private=False,
        product="Zed Package Test",
    ),
)

_OWNER_RE = re.compile(r"^[A-Za-z0-9](?:[A-Za-z0-9-]{0,38})$")
_REPO_RE = re.compile(r"^[A-Za-z0-9._-]{1,100}$")


class PublisherError(RuntimeError):
    """Fail-closed publication error with no provider response body."""


def validate_specs(specs: Iterable[RepositorySpec] = REPOSITORIES) -> tuple[RepositorySpec, ...]:
    values = tuple(specs)
    if len(values) != 5:
        raise PublisherError(f"repository allowlist must contain exactly five entries, got {len(values)}")

    expected_names = {
        "cliptown/cliptown-mcp-server.rs",
        "opto-sync/opto-sync-mcp-server.rs",
        "voxletra/vxl-mcp-server.rs",
        "zed-pkg/zed-mcp-server.rs",
        "zed-pkg-test/zed-pkg-test-mcp-server.rs",
    }
    observed_names = {spec.full_name for spec in values}
    if observed_names != expected_names:
        raise PublisherError(f"repository allowlist drift: {sorted(observed_names)}")

    expected_visibility = {
        "cliptown/cliptown-mcp-server.rs": "public",
        "opto-sync/opto-sync-mcp-server.rs": "public",
        "voxletra/vxl-mcp-server.rs": "private",
        "zed-pkg/zed-mcp-server.rs": "public",
        "zed-pkg-test/zed-pkg-test-mcp-server.rs": "public",
    }
    for spec in values:
        if not _OWNER_RE.fullmatch(spec.owner):
            raise PublisherError(f"invalid owner in allowlist: {spec.owner!r}")
        if not _REPO_RE.fullmatch(spec.name):
            raise PublisherError(f"invalid repository name in allowlist: {spec.name!r}")
        if spec.visibility != expected_visibility[spec.full_name]:
            raise PublisherError(f"visibility drift for {spec.full_name}: {spec.visibility}")
        if not spec.description.strip() or "\n" in spec.description:
            raise PublisherError(f"invalid description for {spec.full_name}")
    return values


def bootstrap_files(spec: RepositorySpec) -> dict[str, str]:
    readme = f"""# {spec.name}

Canonical Rust Model Context Protocol server for **{spec.product}**.

This repository is intentionally bootstrapped with a minimal reviewed root.
The implementation lands through a normal pull request with Rust formatting,
Clippy, tests, release-build, stdio-framing, read-only safety, and bounded-output
checks. Product tools must use the product's interfaces/API contracts and must
not introduce a parallel database or credential path.

## Security boundary

- MCP JSON-RPC owns stdout; diagnostics belong on stderr.
- Initial tools are read-only and deny unknown request fields.
- Tool output is bounded and versioned.
- No arbitrary URL, filesystem path, shell command, credential, or mutation is
  accepted from an MCP caller.
- Secrets, clipboard plaintext, media payloads, package credentials, and user
  content are never attached to telemetry.

## Repository

`https://github.com/{spec.full_name}`
"""
    security = """# Security policy

Report vulnerabilities privately through GitHub Security Advisories for this
repository. Do not open public issues containing credentials, tokens, private
clipboard content, transcription/media data, package-registry credentials, or
other user data.
"""
    license_text = """MIT License

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
    return {
        ".gitignore": "/target\n.env\n.env.*\n!.env.example\n",
        "LICENSE": license_text,
        "README.md": readme,
        "SECURITY.md": security,
    }
