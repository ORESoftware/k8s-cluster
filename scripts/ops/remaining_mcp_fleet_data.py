#!/usr/bin/env python3
"""Reviewed allowlists and domain contracts for the remaining MCP fleet."""
from __future__ import annotations

from dataclasses import dataclass
from typing import Any

RMCP_VERSION = "3.1.0"
RUST_VERSION = "1.97.1"
MSRV = "1.88.0"
STABLE_PROTOCOL = "2025-11-25"
SHARED_REPOSITORY = "https://github.com/ORESoftware/mcp-rust-libs"
SHARED_REVISION = "b9e34bc9983f33a4286f271c510957d24b963c8d"
TEMPLATE_VERSION = 2
MAX_OUTPUT_BYTES = 256 * 1024


@dataclass(frozen=True)
class RepositorySpec:
    owner: str
    name: str
    visibility: str
    issue: str
    crate_name: str
    binary_name: str
    server_type: str
    server_title: str
    description: str
    validator_tool: str
    validator_request: str
    domain_contract: dict[str, Any]
    repositories: tuple[tuple[str, str], ...]
    rust_types: str
    rust_validation: str
    valid_arguments: dict[str, Any]
    forbidden_argument: tuple[str, Any]

    @property
    def full_name(self) -> str:
        return f"{self.owner}/{self.name}"

    @property
    def branch(self) -> str:
        return f"agent/{self.issue.lower()}-bootstrap"


@dataclass(frozen=True)
class MonorepoSpec:
    owner: str
    name: str
    visibility: str
    issue: str
    existing: bool
    repositories: tuple[tuple[str, str], ...]

    @property
    def full_name(self) -> str:
        return f"{self.owner}/{self.name}"

    @property
    def branch(self) -> str:
        return f"agent/{self.issue.lower()}-wire-mcp"


def _repos(owner: str, names: tuple[str, ...]) -> tuple[tuple[str, str], ...]:
    return tuple((name, f"https://github.com/{owner}/{name}.git") for name in names)


SERVER_SPECS: tuple[RepositorySpec, ...] = (
    RepositorySpec(
        owner="cliptown",
        name="cliptown-mcp-server.rs",
        visibility="public",
        issue="DEN-162",
        crate_name="cliptown_mcp_server",
        binary_name="cliptown-mcp-server",
        server_type="ClipTownMcp",
        server_title="ClipTown MCP Server",
        description="Read-only ClipTown clipboard metadata, synchronization, repository, client, and deployment diagnostics",
        validator_tool="validate_sync_metadata",
        validator_request="SyncMetadataRequest",
        domain_contract={
            "schema_version": 1,
            "domain": "encrypted cross-device clipboard synchronization",
            "privacy": [
                "no clipboard content accepted or returned",
                "metadata-only validation",
                "device identifiers are bounded opaque values",
            ],
            "sync": [
                "local queue",
                "encrypted remote replication",
                "offline retry",
                "conflict-safe deduplication",
            ],
            "clients": ["Flutter", "browser extension", "CLI", "typed clients"],
        },
        repositories=_repos(
            "cliptown",
            (
                "cliptown-rust-backend.rs",
                "cliptown-flutter",
                "cliptown-clients",
                "cliptown-interfaces",
                "cliptown-infra",
                "cliptown-cli",
                "cliptown-extension",
                "cliptown.github.io",
                "homebrew-cliptown",
                "cliptown-monorepo",
                "cliptown-mcp-server.rs",
            ),
        ),
        rust_types=r'''
#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum ContentKind { TextMetadata, UrlMetadata, ImageMetadata, FileMetadata, OtherMetadata }

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SyncMetadataRequest {
    device_id: String,
    content_kind: ContentKind,
    byte_length: u64,
    encrypted: bool,
    pinned: bool,
}
''',
        rust_validation=r'''
domain::validate_identifier("device_id", &request.device_id, 128)?;
if request.byte_length > 16 * 1024 * 1024 {
    return Err(ErrorData::invalid_params("byte_length exceeds the 16 MiB metadata ceiling", None));
}
if !request.encrypted {
    return Err(ErrorData::invalid_params("encrypted must be true for remote synchronization", None));
}
render(&json!({
    "accepted": true,
    "content_kind": request.content_kind,
    "byte_length": request.byte_length,
    "pinned": request.pinned,
    "content_received": false,
    "next_checks": ["deduplication", "retention", "device authorization"]
}))
''',
        valid_arguments={
            "device_id": "device-01",
            "content_kind": "text_metadata",
            "byte_length": 4096,
            "encrypted": True,
            "pinned": False,
        },
        forbidden_argument=("content", "private clipboard text"),
    ),
    RepositorySpec(
        owner="opto-sync",
        name="opto-sync-mcp-server.rs",
        visibility="public",
        issue="DEN-163",
        crate_name="opto_sync_mcp_server",
        binary_name="opto-sync-mcp-server",
        server_type="OptoSyncMcp",
        server_title="Opto Sync MCP Server",
        description="Read-only Opto Sync consistency, storage, parity, background lifecycle, repository, and E2E diagnostics",
        validator_tool="validate_sync_plan",
        validator_request="SyncPlanRequest",
        domain_contract={
            "schema_version": 1,
            "domain": "background-first offline synchronization",
            "storage": ["IndexedDB", "SQLite", "Postgres", "Supabase"],
            "consistency": ["local_first", "remote_first", "strong", "eventual"],
            "engines": ["Rust", "C"],
            "safety": [
                "read-only planning",
                "no database credentials",
                "no live synchronization",
            ],
        },
        repositories=_repos(
            "opto-sync",
            (
                "syncer.c",
                "syncer.rs",
                "opto-sync-clients",
                "opto-sync-e2e",
                "opto-sync-mcp-server.rs",
            ),
        ),
        rust_types=r'''
#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum StorageBackend { IndexedDb, Sqlite, Postgres, Supabase }
#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum ConsistencyMode { LocalFirst, RemoteFirst, Strong, Eventual }
#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum ConflictPolicy { LastWriteWins, Crdt, ServerAuthoritative, Manual }

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SyncPlanRequest {
    storage: StorageBackend,
    consistency: ConsistencyMode,
    conflict_policy: ConflictPolicy,
    schema_version: String,
    batch_size: u32,
    retry_limit: u8,
    background: bool,
}
''',
        rust_validation=r'''
domain::validate_identifier("schema_version", &request.schema_version, 64)?;
if !(1..=10_000).contains(&request.batch_size) {
    return Err(ErrorData::invalid_params("batch_size must be within 1..=10000", None));
}
if request.retry_limit > 20 {
    return Err(ErrorData::invalid_params("retry_limit must be at most 20", None));
}
render(&json!({
    "accepted": true,
    "storage": request.storage,
    "consistency": request.consistency,
    "conflict_policy": request.conflict_policy,
    "background": request.background,
    "execution_performed": false,
    "required_evidence": ["offline recovery", "Rust/C parity", "idempotent replay", "lifecycle compliance"]
}))
''',
        valid_arguments={
            "storage": "indexed_db",
            "consistency": "local_first",
            "conflict_policy": "crdt",
            "schema_version": "v1",
            "batch_size": 256,
            "retry_limit": 5,
            "background": True,
        },
        forbidden_argument=("database_url", "postgres://secret"),
    ),
    RepositorySpec(
        owner="voxletra",
        name="vxl-mcp-server.rs",
        visibility="private",
        issue="DEN-164",
        crate_name="vxl_mcp_server",
        binary_name="vxl-mcp-server",
        server_type="VoxletraMcp",
        server_title="Voxletra MCP Server",
        description="Read-only Voxletra architecture, client, transcription metadata, synchronization, test, release, and deployment diagnostics",
        validator_tool="validate_transcription_metadata",
        validator_request="TranscriptionMetadataRequest",
        domain_contract={
            "schema_version": 1,
            "domain": "private multi-client voice and transcription platform",
            "surfaces": [
                "API",
                "web",
                "Flutter",
                "Chrome extension",
                "typed clients",
                "sync",
            ],
            "privacy": [
                "no raw audio",
                "no transcript text",
                "encrypted source required",
                "opaque media identifiers",
            ],
            "deployment": [
                "private repositories",
                "credential-free diagnostics",
                "bounded metadata",
            ],
        },
        repositories=_repos(
            "voxletra",
            (
                "vxl-api-server.rs",
                "vxl-web-server.rs",
                "vxl-interfaces",
                "vxl-clients",
                "vxl-flutter",
                "vxl-chrome-extension",
                "vxl-infra",
                "voxletra-sync",
                "voxletra-e2e",
                "vxl-mcp-server.rs",
            ),
        ),
        rust_types=r'''
#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum ClientSurface { Api, Web, Flutter, ChromeExtension, TypedClient, SyncWorker }

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct TranscriptionMetadataRequest {
    media_id: String,
    language_tag: String,
    duration_ms: u64,
    encrypted_source: bool,
    diarization: bool,
    client: ClientSurface,
}
''',
        rust_validation=r'''
domain::validate_identifier("media_id", &request.media_id, 128)?;
domain::validate_language_tag(&request.language_tag)?;
if request.duration_ms > 24 * 60 * 60 * 1000 {
    return Err(ErrorData::invalid_params("duration_ms exceeds the 24-hour ceiling", None));
}
if !request.encrypted_source {
    return Err(ErrorData::invalid_params("encrypted_source must be true", None));
}
render(&json!({
    "accepted": true,
    "language_tag": request.language_tag,
    "duration_ms": request.duration_ms,
    "diarization": request.diarization,
    "client": request.client,
    "audio_received": false,
    "transcript_received": false
}))
''',
        valid_arguments={
            "media_id": "media-01",
            "language_tag": "en-US",
            "duration_ms": 60000,
            "encrypted_source": True,
            "diarization": True,
            "client": "flutter",
        },
        forbidden_argument=("transcript", "private words"),
    ),
    RepositorySpec(
        owner="zed-pkg",
        name="zed-mcp-server.rs",
        visibility="public",
        issue="DEN-165",
        crate_name="zed_mcp_server",
        binary_name="zed-mcp-server",
        server_type="ZedMcp",
        server_title="Zed Package MCP Server",
        description="Read-only Zed manifest, lockfile, package graph, registry compatibility, submodule, publication-plan, test, and CI diagnostics",
        validator_tool="validate_package_plan",
        validator_request="PackagePlanRequest",
        domain_contract={
            "schema_version": 1,
            "domain": "polyglot deterministic package and submodule management",
            "manifests": [".zpkg.toml", ".zpkg.lock", ".gitmodules"],
            "languages": [
                "Rust",
                "TypeScript",
                "Dart",
                "Go",
                "Gleam",
                "Erlang",
                "WASM",
                "Shell",
            ],
            "safety": [
                "publication planning only",
                "no tags",
                "no registry writes",
                "exact revision required",
            ],
        },
        repositories=_repos(
            "zed-pkg",
            (
                "zed-interfaces",
                "zed-cli",
                "zed-api-server.rs",
                "zed-web-server.rs",
                "zed-clients",
                "zed-sync",
                "zed-infra",
                "zed-pkg.github.io",
                "zed-e2e",
                "zed-monorepo",
                "zed-mcp-server.rs",
            ),
        ),
        rust_types=r'''
#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum PackageLanguage { Rust, TypeScript, Dart, Go, Gleam, Erlang, Wasm, Shell }
#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum RegistryTarget { Git, Zed, Cargo, Npm, Pub }

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct PackagePlanRequest {
    package_name: String,
    language: PackageLanguage,
    revision: String,
    registry: RegistryTarget,
    dependency_count: u32,
    includes_git_submodules: bool,
    plan_only: bool,
}
''',
        rust_validation=r'''
domain::validate_package_name(&request.package_name)?;
domain::validate_revision(&request.revision)?;
if request.dependency_count > 10_000 {
    return Err(ErrorData::invalid_params("dependency_count exceeds 10000", None));
}
if !request.plan_only {
    return Err(ErrorData::invalid_params("plan_only must be true; this server cannot publish", None));
}
render(&json!({
    "accepted": true,
    "package_name": request.package_name,
    "language": request.language,
    "registry": request.registry,
    "revision": request.revision,
    "includes_git_submodules": request.includes_git_submodules,
    "publication_performed": false,
    "checks": ["manifest", "lockfile", "dependency placement", "registry compatibility", "submodule parity"]
}))
''',
        valid_arguments={
            "package_name": "example-package",
            "language": "rust",
            "revision": "0123456789abcdef0123456789abcdef01234567",
            "registry": "zed",
            "dependency_count": 8,
            "includes_git_submodules": True,
            "plan_only": True,
        },
        forbidden_argument=("publish_token", "secret"),
    ),
    RepositorySpec(
        owner="zed-pkg-test",
        name="zed-pkg-test-mcp-server.rs",
        visibility="public",
        issue="DEN-166",
        crate_name="zed_pkg_test_mcp_server",
        binary_name="zed-pkg-test-mcp-server",
        server_type="ZedPkgTestMcp",
        server_title="Zed Package Test MCP Server",
        description="Read-only Zed fixture inventory, negative compatibility, package graph, lockfile, publication-plan, and CI diagnostics",
        validator_tool="validate_fixture",
        validator_request="FixtureRequest",
        domain_contract={
            "schema_version": 1,
            "domain": "negative and compatibility fixtures for Zed packages",
            "negative_fixtures": [
                "invalid_manifest",
                "invalid_lockfile",
                "dependency_cycle",
                "path_traversal",
                "unsupported_language",
                "ambiguous_registry",
                "publication_denied",
            ],
            "safety": [
                "no live registry",
                "no fixture rewriting",
                "no tags",
                "bounded hermetic execution metadata",
            ],
        },
        repositories=_repos(
            "zed-pkg-test",
            ("zed-pkg-e2e", "zed-pkg-test-mcp-server.rs"),
        ),
        rust_types=r'''
#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum FixtureScenario { ValidManifest, InvalidManifest, InvalidLockfile, DependencyCycle, PathTraversal, UnsupportedLanguage, AmbiguousRegistry, PublicationDenied }
#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum FixtureLanguage { Rust, TypeScript, Dart, Go, Gleam, Erlang, Wasm, Shell, Unsupported }
#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum FixtureRegistry { Git, Zed, Cargo, Npm, Pub, Ambiguous }

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct FixtureRequest {
    fixture_name: String,
    scenario: FixtureScenario,
    language: FixtureLanguage,
    registry: FixtureRegistry,
    expected_error: String,
    relative_path: String,
    timeout_seconds: u16,
    live_registry: bool,
}
''',
        rust_validation=r'''
domain::validate_identifier("fixture_name", &request.fixture_name, 128)?;
domain::validate_identifier("expected_error", &request.expected_error, 128)?;
domain::validate_relative_path(&request.relative_path)?;
if !(1..=300).contains(&request.timeout_seconds) {
    return Err(ErrorData::invalid_params("timeout_seconds must be within 1..=300", None));
}
if request.live_registry {
    return Err(ErrorData::invalid_params("live_registry must be false", None));
}
render(&json!({
    "accepted": true,
    "fixture_name": request.fixture_name,
    "scenario": request.scenario,
    "language": request.language,
    "registry": request.registry,
    "expected_error": request.expected_error,
    "relative_path": request.relative_path,
    "registry_contacted": false,
    "fixture_rewritten": false
}))
''',
        valid_arguments={
            "fixture_name": "invalid-manifest-01",
            "scenario": "invalid_manifest",
            "language": "rust",
            "registry": "zed",
            "expected_error": "manifest_invalid",
            "relative_path": "fixtures/invalid-manifest",
            "timeout_seconds": 30,
            "live_registry": False,
        },
        forbidden_argument=("registry_token", "secret"),
    ),
)

MONOREPO_SPECS: tuple[MonorepoSpec, ...] = (
    MonorepoSpec(
        "cliptown",
        "cliptown-monorepo",
        "public",
        "DEN-162",
        True,
        _repos("cliptown", ("cliptown-mcp-server.rs",)),
    ),
    MonorepoSpec(
        "opto-sync",
        "opto-sync-monorepo",
        "public",
        "DEN-163",
        False,
        _repos(
            "opto-sync",
            (
                "syncer.c",
                "syncer.rs",
                "opto-sync-clients",
                "opto-sync-e2e",
                "opto-sync-mcp-server.rs",
            ),
        ),
    ),
    MonorepoSpec(
        "voxletra",
        "vxl-monorepo",
        "private",
        "DEN-164",
        False,
        _repos(
            "voxletra",
            (
                "vxl-api-server.rs",
                "vxl-web-server.rs",
                "vxl-interfaces",
                "vxl-clients",
                "vxl-flutter",
                "vxl-chrome-extension",
                "vxl-infra",
                "voxletra-sync",
                "voxletra-e2e",
                "vxl-mcp-server.rs",
            ),
        ),
    ),
    MonorepoSpec(
        "zed-pkg",
        "zed-monorepo",
        "public",
        "DEN-165",
        True,
        _repos("zed-pkg", ("zed-mcp-server.rs",)),
    ),
    MonorepoSpec(
        "zed-pkg-test",
        "zed-pkg-test-monorepo",
        "public",
        "DEN-166",
        False,
        _repos("zed-pkg-test", ("zed-pkg-e2e", "zed-pkg-test-mcp-server.rs")),
    ),
)
