#!/usr/bin/env python3
"""Contract tests for the bounded newer-organization repository publisher."""

from __future__ import annotations

import base64
import copy
import hashlib
import json
import os
import pathlib
import tempfile
import unittest
from collections.abc import Iterable, Mapping
from contextlib import redirect_stderr, redirect_stdout
from io import StringIO
from typing import Any

import publish_new_org_repository_fleet as publisher
from new_org_repository_templates import files_for_repository

ROOT = pathlib.Path(__file__).resolve().parent
MANIFEST_PATH = ROOT / "new_org_repository_fleet.json"


class NoNetworkApi:
    def __init__(self) -> None:
        self.calls: list[tuple[str, str]] = []

    def request(
        self,
        method: str,
        path: str,
        payload: Mapping[str, Any] | list[Any] | None = None,
        *,
        allowed_statuses: Iterable[int] = (200,),
    ) -> publisher.ApiResponse:
        del payload, allowed_statuses
        self.calls.append((method, path))
        raise AssertionError(f"unexpected network request: {method} {path}")


class FakeGitHubApi:
    """Small stateful GitHub REST model covering the publisher's API surface."""

    def __init__(self, *, preserve_existing: bool = False, mutate_preserved_refs: bool = False) -> None:
        self.preserve_existing = preserve_existing
        self.mutate_preserved_refs = mutate_preserved_refs
        self.calls: list[tuple[str, str, Any]] = []
        self.write_calls: list[tuple[str, str]] = []
        self.repositories: dict[str, dict[str, Any]] = {}
        self.files: dict[str, dict[str, str]] = {}
        self.main_shas: dict[str, str] = {}
        self.matching_ref_reads: dict[str, int] = {}
        self._next_id = 10_000
        self._next_object = 1

    @staticmethod
    def _decode_repo(path: str) -> tuple[str, str]:
        parts = path.split("?")[0].strip("/").split("/")
        if len(parts) < 3 or parts[0] != "repos":
            raise AssertionError(f"cannot decode repository path: {path}")
        return parts[1], parts[2]

    @staticmethod
    def _full_name(path: str) -> str:
        owner, name = FakeGitHubApi._decode_repo(path)
        return f"{owner}/{name}"

    def _sha(self, label: str) -> str:
        counter = self._next_object
        self._next_object += 1
        return hashlib.sha1(f"{label}:{counter}".encode("utf-8"), usedforsecurity=False).hexdigest()

    def _ensure_existing(self, full_name: str) -> dict[str, Any]:
        repository = self.repositories.get(full_name)
        if repository is not None:
            return repository
        owner, name = full_name.split("/", 1)
        visibility = "public" if name == ".github" or owner == "unreal-unity-poc" else "private"
        repository = {
            "id": self._next_id,
            "full_name": full_name,
            "visibility": visibility,
            "private": visibility == "private",
            "default_branch": "main" if self.preserve_existing else "",
        }
        self._next_id += 1
        self.repositories[full_name] = repository
        if self.preserve_existing:
            self.main_shas[full_name] = hashlib.sha1(
                f"preserved:{full_name}".encode("utf-8"), usedforsecurity=False
            ).hexdigest()
        return repository

    def request(
        self,
        method: str,
        path: str,
        payload: Mapping[str, Any] | list[Any] | None = None,
        *,
        allowed_statuses: Iterable[int] = (200,),
    ) -> publisher.ApiResponse:
        allowed = set(allowed_statuses)
        self.calls.append((method, path, copy.deepcopy(payload)))
        if method in {"POST", "PUT", "PATCH", "DELETE"}:
            self.write_calls.append((method, path))

        status: int
        data: Any

        if method == "GET" and path.startswith("/repos/") and "/git/" not in path and "/contents/" not in path:
            full_name = self._full_name(path)
            if self.preserve_existing:
                data = copy.deepcopy(self._ensure_existing(full_name))
                status = 200
            elif full_name in self.repositories:
                data = copy.deepcopy(self.repositories[full_name])
                status = 200
            else:
                data = {"message": "Not Found"}
                status = 404

        elif method == "POST" and path.startswith("/orgs/") and path.endswith("/repos"):
            assert isinstance(payload, Mapping)
            owner = path.split("/")[2]
            name = str(payload["name"])
            full_name = f"{owner}/{name}"
            if full_name in self.repositories:
                status = 422
                data = {"message": "name already exists"}
            else:
                visibility = str(payload["visibility"])
                repository = {
                    "id": self._next_id,
                    "full_name": full_name,
                    "visibility": visibility,
                    "private": bool(payload["private"]),
                    "default_branch": "",
                }
                self._next_id += 1
                self.repositories[full_name] = repository
                status = 201
                data = copy.deepcopy(repository)

        elif method == "GET" and path.endswith("/git/matching-refs/heads/"):
            full_name = self._full_name(path)
            count = self.matching_ref_reads.get(full_name, 0) + 1
            self.matching_ref_reads[full_name] = count
            if full_name not in self.main_shas:
                status = 409
                data = {"message": "Git Repository is empty."}
            else:
                sha = self.main_shas[full_name]
                if self.mutate_preserved_refs and count > 1:
                    sha = hashlib.sha1(
                        f"mutated:{full_name}".encode("utf-8"), usedforsecurity=False
                    ).hexdigest()
                status = 200
                data = [{"ref": "refs/heads/main", "object": {"sha": sha, "type": "commit"}}]

        elif method == "POST" and path.endswith("/git/trees"):
            assert isinstance(payload, Mapping)
            full_name = self._full_name(path)
            entries = payload.get("tree")
            assert isinstance(entries, list) and entries
            file_map: dict[str, str] = {}
            for entry in entries:
                assert isinstance(entry, Mapping)
                assert entry.get("mode") == "100644"
                assert entry.get("type") == "blob"
                file_map[str(entry["path"])] = str(entry["content"])
            self.files[full_name] = file_map
            status = 201
            data = {"sha": self._sha(f"tree:{full_name}")}

        elif method == "POST" and path.endswith("/git/commits"):
            assert isinstance(payload, Mapping)
            assert payload.get("parents") == []
            assert payload.get("author") == publisher.COMMIT_IDENTITY
            assert payload.get("committer") == publisher.COMMIT_IDENTITY
            full_name = self._full_name(path)
            status = 201
            data = {"sha": self._sha(f"commit:{full_name}")}

        elif method == "POST" and path.endswith("/git/refs"):
            assert isinstance(payload, Mapping)
            assert payload.get("ref") == "refs/heads/main"
            full_name = self._full_name(path)
            self.main_shas[full_name] = str(payload["sha"])
            status = 201
            data = {"ref": "refs/heads/main", "object": {"sha": payload["sha"]}}

        elif method == "PATCH" and path.count("/") == 3 and path.startswith("/repos/"):
            assert isinstance(payload, Mapping)
            full_name = self._full_name(path)
            repository = self.repositories[full_name]
            repository.update(payload)
            status = 200
            data = copy.deepcopy(repository)

        elif method == "PUT" and path.endswith("/topics"):
            assert isinstance(payload, Mapping)
            names = payload.get("names")
            assert isinstance(names, list) and "new-org-core-v1" in names
            status = 200
            data = {"names": names}

        elif method == "PUT" and path.endswith(("/vulnerability-alerts", "/automated-security-fixes")):
            status = 204
            data = None

        elif method == "GET" and path.endswith("/git/ref/heads/main"):
            full_name = self._full_name(path)
            status = 200
            data = {"ref": "refs/heads/main", "object": {"sha": self.main_shas[full_name]}}

        elif method == "GET" and "/contents/repo-relationships.json" in path:
            full_name = self._full_name(path)
            content = self.files[full_name]["repo-relationships.json"]
            status = 200
            data = {
                "encoding": "base64",
                "content": base64.b64encode(content.encode("utf-8")).decode("ascii"),
            }

        else:
            raise AssertionError(f"unmodeled GitHub request: {method} {path}")

        if status not in allowed:
            raise publisher.FleetError(f"fake GitHub returned HTTP {status} for {method} {path}")
        return publisher.ApiResponse(status=status, data=data, headers={})


class FleetManifestTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.manifest = json.loads(MANIFEST_PATH.read_text(encoding="utf-8"))
        cls.flattened = publisher.validate_manifest(cls.manifest)

    def test_manifest_is_exactly_bounded(self) -> None:
        self.assertEqual(len(self.flattened), publisher.EXPECTED_REPOSITORY_COUNT)
        owners = {str(org["owner"]) for org, _ in self.flattened}
        self.assertEqual(owners, publisher.ALLOWED_ORGANIZATIONS)
        full_names = [f"{org['owner']}/{repo['name']}" for org, repo in self.flattened]
        self.assertEqual(len(full_names), len(set(name.casefold() for name in full_names)))

    def test_manifest_hash_is_stable_and_canonical(self) -> None:
        canonical = publisher.canonical_manifest_bytes(MANIFEST_PATH)
        self.assertEqual(
            hashlib.sha256(canonical).hexdigest(),
            "cd4408e2227440d0356604decf36dbb63f544b4f7b73ceb2fa4285abf42acb48",
        )
        self.assertEqual(canonical, publisher.canonical_manifest_bytes(MANIFEST_PATH))

    def test_each_org_has_one_public_governance_and_one_mcp_repository(self) -> None:
        for organization in self.manifest["organizations"]:
            managed = organization["repositories"]
            governance = [repo for repo in managed if repo["role"] == "governance"]
            mcp = [repo for repo in managed if repo["role"] == "mcp"]
            self.assertEqual(governance, [next(repo for repo in managed if repo["name"] == ".github")])
            self.assertEqual(governance[0]["visibility"], "public")
            self.assertEqual(len(mcp), 1)
            self.assertTrue(mcp[0]["name"].endswith("-mcp-server.rs"))

    def test_manifest_rejects_scope_and_visibility_drift(self) -> None:
        bad_owner = copy.deepcopy(self.manifest)
        bad_owner["organizations"][0]["owner"] = "unexpected-org"
        with self.assertRaisesRegex(publisher.FleetError, "bounded allowlist"):
            publisher.validate_manifest(bad_owner)

        bad_visibility = copy.deepcopy(self.manifest)
        product_repo = bad_visibility["organizations"][0]["repositories"][1]
        product_repo["visibility"] = "public"
        with self.assertRaisesRegex(publisher.FleetError, "visibility must match"):
            publisher.validate_manifest(bad_visibility)

        bad_count = copy.deepcopy(self.manifest)
        bad_count["organizations"][0]["repositories"].pop()
        with self.assertRaisesRegex(publisher.FleetError, "expected 65"):
            publisher.validate_manifest(bad_count)


class SeedContractTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.manifest = json.loads(MANIFEST_PATH.read_text(encoding="utf-8"))
        cls.flattened = publisher.validate_manifest(cls.manifest)

    def test_all_seed_files_are_deterministic_safe_and_complete(self) -> None:
        generated_count = 0
        for organization, repository in self.flattened:
            first = files_for_repository(organization, repository)
            second = files_for_repository(organization, repository)
            self.assertEqual(first, second)
            self.assertIn("README.md", first)
            self.assertIn("AGENTS.md", first)
            self.assertIn("repo-relationships.json", first)
            self.assertTrue(any(path.startswith(".github/workflows/") for path in first))
            marker = json.loads(first["repo-relationships.json"])
            full_name = f"{organization['owner']}/{repository['name']}"
            self.assertEqual(marker["fleet_id"], publisher.EXPECTED_FLEET_ID)
            self.assertEqual(marker["repository"], full_name)
            for path, content in first.items():
                self.assertNotIn("ghp_", content, f"credential marker in {full_name}:{path}")
                self.assertNotIn("github_pat_", content, f"credential marker in {full_name}:{path}")
                self.assertNotIn("-----BEGIN PRIVATE KEY-----", content)
                for line in content.splitlines():
                    if "uses:" in line:
                        reference = line.split("uses:", 1)[1].strip().split()[0]
                        self.assertRegex(reference, r"@[0-9a-f]{40}$", f"unpinned action in {full_name}:{path}")
            generated_count += len(first)
        self.assertEqual(generated_count, 750)

    def test_role_specific_contracts(self) -> None:
        by_role: dict[str, tuple[Mapping[str, Any], Mapping[str, Any]]] = {}
        for organization, repository in self.flattened:
            by_role.setdefault(str(repository["role"]), (organization, repository))

        mcp_org, mcp_repo = by_role["mcp"]
        mcp = files_for_repository(mcp_org, mcp_repo)
        self.assertIn("rmcp = { version = \"=3.1.0\"", mcp["Cargo.toml"])
        self.assertIn("org_map", mcp["src/server.rs"])
        self.assertIn("list_repositories", mcp["src/server.rs"])
        self.assertIn("health", mcp["src/server.rs"])
        self.assertNotIn("reqwest", "\n".join(mcp.values()))
        self.assertNotIn("std::process::Command", "\n".join(mcp.values()))

        server_org, server_repo = by_role["server"]
        server = files_for_repository(server_org, server_repo)
        server_text = "\n".join(server.values())
        for contract in ("/healthz", "/readyz", "/metrics", "16 * 1024", "set_read_timeout"):
            self.assertIn(contract, server_text)
        self.assertIn("gcr.io/distroless/cc-debian12:nonroot", server["Dockerfile"])

        infra_org, infra_repo = by_role["infra"]
        infra = files_for_repository(infra_org, infra_repo)
        deployment = infra["k8s/base/deployment.yaml"]
        for contract in (
            "automountServiceAccountToken: false",
            "runAsNonRoot: true",
            "readOnlyRootFilesystem: true",
            "allowPrivilegeEscalation: false",
            "drop: [\"ALL\"]",
            "livenessProbe:",
            "readinessProbe:",
            "resources:",
        ):
            self.assertIn(contract, deployment)

        clients_org, clients_repo = by_role["clients"]
        clients = files_for_repository(clients_org, clients_repo)
        required_client_roots = {
            "clients/rust/Cargo.toml",
            "clients/typescript/package.json",
            "clients/dart/pubspec.yaml",
            "clients/go/go.mod",
            "clients/gleam/gleam.toml",
            "clients/java/pom.xml",
            "clients/swift/Package.swift",
            "clients/wasm/world.wit",
        }
        self.assertTrue(required_client_roots.issubset(clients))


class PublisherBehaviorTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.manifest = json.loads(MANIFEST_PATH.read_text(encoding="utf-8"))
        cls.manifest_sha = hashlib.sha256(publisher.canonical_manifest_bytes(MANIFEST_PATH)).hexdigest()

    def test_dry_run_makes_zero_github_requests(self) -> None:
        api = NoNetworkApi()
        summary = publisher.RepositoryPublisher(api, execute=False).publish(
            self.manifest, manifest_sha256=self.manifest_sha
        )
        self.assertEqual(api.calls, [])
        self.assertEqual(len(summary["planned"]), publisher.EXPECTED_REPOSITORY_COUNT)
        self.assertEqual(summary["created"], [])
        self.assertEqual(summary["preserved"], [])

    def test_create_flow_initializes_every_repository_atomically_and_verifies_markers(self) -> None:
        api = FakeGitHubApi()
        with redirect_stdout(StringIO()):
            summary = publisher.RepositoryPublisher(api, execute=True).publish(
                self.manifest, manifest_sha256=self.manifest_sha
            )
        self.assertEqual(len(summary["created"]), publisher.EXPECTED_REPOSITORY_COUNT)
        self.assertEqual(summary["initialized"], [])
        self.assertEqual(summary["preserved"], [])
        self.assertEqual(summary["observed_repository_count"], publisher.EXPECTED_REPOSITORY_COUNT)
        self.assertEqual(len(api.repositories), publisher.EXPECTED_REPOSITORY_COUNT)
        self.assertEqual(len(api.main_shas), publisher.EXPECTED_REPOSITORY_COUNT)
        self.assertEqual(len(api.files), publisher.EXPECTED_REPOSITORY_COUNT)

        create_calls = [call for call in api.write_calls if call[0] == "POST" and call[1].startswith("/orgs/")]
        tree_calls = [call for call in api.write_calls if call[0] == "POST" and call[1].endswith("/git/trees")]
        commit_calls = [call for call in api.write_calls if call[0] == "POST" and call[1].endswith("/git/commits")]
        ref_calls = [call for call in api.write_calls if call[0] == "POST" and call[1].endswith("/git/refs")]
        self.assertEqual(len(create_calls), publisher.EXPECTED_REPOSITORY_COUNT)
        self.assertEqual(len(tree_calls), publisher.EXPECTED_REPOSITORY_COUNT)
        self.assertEqual(len(commit_calls), publisher.EXPECTED_REPOSITORY_COUNT)
        self.assertEqual(len(ref_calls), publisher.EXPECTED_REPOSITORY_COUNT)

        self.assertEqual(api.repositories["channelsiege/.github"]["visibility"], "public")
        self.assertEqual(api.repositories["channelsiege/channelsiege-mcp-server.rs"]["visibility"], "private")
        self.assertEqual(api.repositories["unreal-unity-poc/unreal-unity-mcp-server.rs"]["visibility"], "public")

    def test_existing_histories_are_preserved_with_zero_writes(self) -> None:
        api = FakeGitHubApi(preserve_existing=True)
        with redirect_stdout(StringIO()):
            summary = publisher.RepositoryPublisher(api, execute=True).publish(
                self.manifest, manifest_sha256=self.manifest_sha
            )
        self.assertEqual(summary["created"], [])
        self.assertEqual(summary["initialized"], [])
        self.assertEqual(len(summary["preserved"]), publisher.EXPECTED_REPOSITORY_COUNT)
        self.assertEqual(api.write_calls, [])
        for result in summary["preserved"]:
            self.assertEqual(result["reason"], "existing history preserved without writes")

    def test_preservation_detects_a_concurrent_branch_change(self) -> None:
        api = FakeGitHubApi(preserve_existing=True, mutate_preserved_refs=True)
        with self.assertRaisesRegex(publisher.FleetError, "existing branch refs changed"):
            with redirect_stdout(StringIO()):
                publisher.RepositoryPublisher(api, execute=True).publish(
                    self.manifest, manifest_sha256=self.manifest_sha
                )
        self.assertEqual(api.write_calls, [])

    def test_execute_requires_exact_confirmations_and_token(self) -> None:
        original = os.environ.pop(publisher.TOKEN_ENV, None)
        try:
            with tempfile.TemporaryDirectory() as directory:
                summary_path = pathlib.Path(directory) / "summary.json"
                stderr = StringIO()
                with redirect_stderr(stderr), redirect_stdout(StringIO()):
                    rc = publisher.main(
                        [
                            "--manifest",
                            str(MANIFEST_PATH),
                            "--execute",
                            "--confirm-fleet",
                            "wrong",
                            "--confirm-repository-count",
                            str(publisher.EXPECTED_REPOSITORY_COUNT),
                            "--summary-file",
                            str(summary_path),
                        ]
                    )
                self.assertEqual(rc, 1)
                self.assertIn("--confirm-fleet", stderr.getvalue())
                self.assertFalse(summary_path.exists())

                stderr = StringIO()
                with redirect_stderr(stderr), redirect_stdout(StringIO()):
                    rc = publisher.main(
                        [
                            "--manifest",
                            str(MANIFEST_PATH),
                            "--execute",
                            "--confirm-fleet",
                            publisher.EXPECTED_FLEET_ID,
                            "--confirm-repository-count",
                            str(publisher.EXPECTED_REPOSITORY_COUNT),
                        ]
                    )
                self.assertEqual(rc, 1)
                self.assertIn(publisher.TOKEN_ENV, stderr.getvalue())
        finally:
            if original is not None:
                os.environ[publisher.TOKEN_ENV] = original


if __name__ == "__main__":
    unittest.main(verbosity=2)
