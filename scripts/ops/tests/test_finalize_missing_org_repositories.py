from __future__ import annotations

import base64
import importlib.util
import json
import os
import sys
import unittest
from pathlib import Path
from unittest import mock

MODULE_PATH = Path(__file__).resolve().parents[1] / "finalize_missing_org_repositories.py"
SPEC = importlib.util.spec_from_file_location("fleet_finalizer", MODULE_PATH)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError(f"unable to load {MODULE_PATH}")
finalizer = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = finalizer
SPEC.loader.exec_module(finalizer)


def sha(index: int) -> str:
    return f"{index + 1:040x}"


def fleet_manifest() -> dict[str, object]:
    repositories: list[dict[str, object]] = []
    for org, count in (("hypesiege", 15), ("streempilot", 17)):
        for index in range(count):
            name = f"{org}-repo-{index + 1:02d}"
            repositories.append(
                {
                    "org": org,
                    "name": name,
                    "full_name": f"{org}/{name}",
                    "kind": "monorepo" if index == count - 1 else "rust-lib",
                    "commit": sha(len(repositories)),
                    "files": 1,
                    "remote": f"https://github.com/{org}/{name}.git",
                    "description": name,
                    "visibility": "public",
                    "default_branch": "main",
                    "gitlinks": 0,
                }
            )
    return {
        "schema_version": 2,
        "generated_at": "2026-07-31T00:00:00-04:00",
        "generator_sha256": finalizer.EXPECTED_GENERATOR_SHA256,
        "default_branch": "main",
        "repository_count": 32,
        "total_tracked_files": 888,
        "total_gitlinks": 30,
        "organizations": {"hypesiege": 15, "streempilot": 17},
        "repositories": repositories,
    }


def content_record(value: object) -> dict[str, str]:
    encoded = base64.b64encode(
        (json.dumps(value, separators=(",", ":")) + "\n").encode("utf-8")
    ).decode("ascii")
    return {"content": encoded, "sha": "f" * 40}


def repository_record(
    full_name: str,
    *,
    visibility: str = "public",
    identifier: int = 1,
) -> dict[str, object]:
    owner, name = full_name.split("/", 1)
    return {
        "id": identifier,
        "name": name,
        "full_name": full_name,
        "owner": {"login": owner},
        "private": visibility == "private",
        "visibility": visibility,
        "default_branch": "main",
        "fork": False,
        "archived": False,
        "disabled": False,
    }


class FinalizerContractTests(unittest.TestCase):
    def test_token_rejects_whitespace(self) -> None:
        with mock.patch.dict(
            os.environ,
            {"GITHUB_REPOSITORY_ADMIN_TOKEN": " token"},
            clear=True,
        ):
            with self.assertRaisesRegex(finalizer.PublicationError, "whitespace"):
                finalizer.token()

    def test_decoded_content_rejects_invalid_base64(self) -> None:
        with self.assertRaisesRegex(finalizer.PublicationError, "invalid base64"):
            finalizer.decoded_content({"content": "not@@base64"})

    def test_pinned_manifest_requires_public_visibility(self) -> None:
        manifest = fleet_manifest()
        with mock.patch.object(
            finalizer,
            "get_content",
            return_value=content_record(manifest),
        ):
            loaded = finalizer.load_fleet_manifest("credential")
        self.assertEqual(loaded["repository_count"], 32)

        repositories = manifest["repositories"]
        assert isinstance(repositories, list)
        first = repositories[0]
        assert isinstance(first, dict)
        first["visibility"] = "private"
        with mock.patch.object(
            finalizer,
            "get_content",
            return_value=content_record(manifest),
        ):
            with self.assertRaisesRegex(
                finalizer.PublicationError,
                "must remain public",
            ):
                finalizer.load_fleet_manifest("credential")

    def test_verify_repository_requires_exact_visibility_and_sha(self) -> None:
        slug = "hypesiege/hypesiege-api-server.rs"
        expected_sha = "a" * 40
        repository = repository_record(slug, visibility="public", identifier=42)
        with mock.patch.object(
            finalizer,
            "api",
            return_value={"object": {"sha": expected_sha}},
        ):
            verified = finalizer.verify_repository(
                slug,
                repository,
                "credential",
                expected_visibility="public",
                expected_sha=expected_sha,
            )
        self.assertEqual(verified["id"], 42)
        self.assertEqual(verified["main_sha"], expected_sha)
        self.assertFalse(verified["private"])

        private = repository_record(slug, visibility="private", identifier=42)
        with self.assertRaisesRegex(finalizer.PublicationError, "private=True"):
            finalizer.verify_repository(
                slug,
                private,
                "credential",
                expected_visibility="public",
                expected_sha=expected_sha,
            )

        with mock.patch.object(
            finalizer,
            "api",
            return_value={"object": {"sha": "b" * 40}},
        ):
            with self.assertRaisesRegex(finalizer.PublicationError, "approved"):
                finalizer.verify_repository(
                    slug,
                    repository,
                    "credential",
                    expected_visibility="public",
                    expected_sha=expected_sha,
                )

    def test_equal_count_wrong_inventory_is_rejected(self) -> None:
        manifest = fleet_manifest()
        by_org: dict[str, list[dict[str, object]]] = {
            "hypesiege": [],
            "streempilot": [],
        }
        repositories = manifest["repositories"]
        assert isinstance(repositories, list)
        for identifier, record in enumerate(repositories, start=1):
            assert isinstance(record, dict)
            org = str(record["org"])
            by_org[org].append(
                repository_record(
                    str(record["full_name"]),
                    identifier=identifier,
                )
            )
        by_org["hypesiege"][-1] = repository_record(
            "hypesiege/unapproved-replacement",
            identifier=999,
        )

        with mock.patch.object(
            finalizer,
            "org_repositories",
            side_effect=lambda org, _credential: by_org[org],
        ):
            with self.assertRaisesRegex(
                finalizer.PublicationError,
                "inventory differs",
            ):
                finalizer.verify_public_fleet(manifest, "credential")

    def test_exact_public_fleet_records_repository_ids_and_shas(self) -> None:
        manifest = fleet_manifest()
        by_org: dict[str, list[dict[str, object]]] = {
            "hypesiege": [],
            "streempilot": [],
        }
        ref_by_slug: dict[str, str] = {}
        repositories = manifest["repositories"]
        assert isinstance(repositories, list)
        for identifier, record in enumerate(repositories, start=100):
            assert isinstance(record, dict)
            slug = str(record["full_name"])
            by_org[str(record["org"])].append(
                repository_record(slug, identifier=identifier)
            )
            ref_by_slug[slug] = str(record["commit"])

        def fake_api(
            method: str,
            path: str,
            _credential: str,
            *_args: object,
            **_kwargs: object,
        ) -> object:
            self.assertEqual(method, "GET")
            prefix = "/repos/"
            suffix = "/git/ref/heads/main"
            self.assertTrue(path.startswith(prefix) and path.endswith(suffix))
            slug = path[len(prefix) : -len(suffix)]
            return {"object": {"sha": ref_by_slug[slug]}}

        with (
            mock.patch.object(
                finalizer,
                "org_repositories",
                side_effect=lambda org, _credential: by_org[org],
            ),
            mock.patch.object(finalizer, "api", side_effect=fake_api),
        ):
            verified = finalizer.verify_public_fleet(manifest, "credential")

        self.assertEqual(verified["hypesiege"]["count"], 15)
        self.assertEqual(verified["StreemPilot"]["count"], 17)
        sample = verified["hypesiege"]["repositories"][0]
        self.assertGreater(sample["id"], 0)
        self.assertEqual(sample["visibility"], "public")
        self.assertEqual(sample["main_sha"], ref_by_slug[sample["slug"]])

    def test_extracted_visibility_contract_is_explicit(self) -> None:
        self.assertEqual(
            finalizer.EXTRACTED["meta-agents-demo/meta-agent-control-plane.rs"][
                "visibility"
            ],
            "public",
        )
        self.assertEqual(
            finalizer.EXTRACTED["file-tunnel/ftnl-mcp-server.rs"]["visibility"],
            "private",
        )

    def test_markdown_distinguishes_public_and_private_repositories(self) -> None:
        report = {
            "source": {
                "repository": finalizer.FLEET_SOURCE_REPOSITORY,
                "sha": finalizer.FLEET_SOURCE_SHA,
            },
            "organizations": {
                "hypesiege": {
                    "count": 1,
                    "expected": 1,
                    "repositories": [
                        {
                            "slug": "hypesiege/example",
                            "id": 1,
                            "main_sha": "a" * 40,
                        }
                    ],
                },
                "StreemPilot": {
                    "count": 1,
                    "expected": 1,
                    "repositories": [
                        {
                            "slug": "StreemPilot/example",
                            "id": 2,
                            "main_sha": "b" * 40,
                        }
                    ],
                },
                "unreal-unity-poc": {"count": 25},
            },
            "extracted_repositories": {
                "meta-agents-demo/meta-agent-control-plane.rs": {
                    "id": 3,
                    "main_sha": "c" * 40,
                    "visibility": "public",
                    "ci": "active",
                    "ci_action": "already-active",
                },
                "file-tunnel/ftnl-mcp-server.rs": {
                    "id": 4,
                    "main_sha": "d" * 40,
                    "visibility": "private",
                    "ci": "active",
                    "ci_action": "activated",
                },
            },
        }
        rendered = finalizer.markdown(report)
        self.assertIn("`hypesiege/example`", rendered)
        self.assertIn("; public", rendered)
        self.assertIn("## meta-agents-demo/meta-agent-control-plane.rs", rendered)
        self.assertIn("visibility: public", rendered)
        self.assertIn("## file-tunnel/ftnl-mcp-server.rs", rendered)
        self.assertIn("visibility: private", rendered)
        self.assertIn("All 34 approved repositories", rendered)


if __name__ == "__main__":
    unittest.main()
