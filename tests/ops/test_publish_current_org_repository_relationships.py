from __future__ import annotations

import importlib.util
import json
from pathlib import Path
import re
import sys
import unittest

ROOT = Path(__file__).resolve().parents[2]
OPS = ROOT / "scripts" / "ops"
MODULE_PATH = OPS / "publish_current_org_repository_relationships.py"
WORKFLOW_PATH = (
    ROOT
    / ".github"
    / "workflows"
    / "ops-current-org-dotgithub-relationships-ephemeral-publish.yml"
)
RUNBOOK_PATH = (
    ROOT
    / "docs"
    / "operations"
    / "org-dotgithub-relationship-publication.md"
)

if str(OPS) not in sys.path:
    sys.path.insert(0, str(OPS))

spec = importlib.util.spec_from_file_location(
    "publish_current_org_repository_relationships",
    MODULE_PATH,
)
module = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = module
assert spec.loader is not None
spec.loader.exec_module(module)
publisher = module.publisher


class CurrentRepositoryRelationshipPublisherTests(unittest.TestCase):
    def repository(
        self,
        name: str,
        *,
        private: bool = False,
        description: str | None = None,
        archived: bool = False,
    ) -> dict:
        return {
            "name": name,
            "full_name": f"example/{name}",
            "private": private,
            "visibility": "private" if private else "public",
            "description": description,
            "archived": archived,
            "fork": False,
            "default_branch": "main",
        }

    def test_current_fleet_is_exactly_62_unique_organizations(self) -> None:
        self.assertEqual(62, module.EXPECTED_COUNT)
        self.assertEqual(62, len(module.ORGANIZATIONS))
        self.assertEqual(
            62,
            len({organization.lower() for organization in module.ORGANIZATIONS}),
        )
        self.assertNotIn("ORESoftware", module.ORGANIZATIONS)
        for organization in (
            "3FA-app",
            "canonical-cloud",
            "networking-components",
            "declarative-migrations-test",
            "shared-auth-test",
            "streempilot-test",
        ):
            self.assertIn(organization, module.ORGANIZATIONS)

    def test_wrapper_patches_preflight_and_engine_to_current_fleet(self) -> None:
        self.assertEqual(module.ORGANIZATIONS, module.governance.ORGANIZATIONS)
        self.assertEqual(module.ORGANIZATIONS, publisher.ORGANIZATIONS)
        self.assertEqual(
            module.ORGANIZATIONS,
            tuple(module.current.ORGANIZATIONS),
        )

    def test_classifies_canonical_repository_roles(self) -> None:
        cases = {
            ".github": "organization_governance",
            "example-interfaces": "interfaces",
            "example-clients": "client_sdk",
            "example-api-server.rs": "api_service",
            "example-web-server.rs": "web_bff",
            "example-sync": "sync_service",
            "example-mcp-server.rs": "mcp_server",
            "example-infra": "infrastructure",
            "example-e2e": "end_to_end_tests",
            "example-monorepo": "composition_workspace",
            "desktop.app.rs": "application",
        }
        for name, expected in cases.items():
            with self.subTest(name=name):
                self.assertEqual(expected, publisher.classify_repository(name))

    def test_manifest_withholds_private_repository_names(self) -> None:
        repositories = [
            self.repository(".github"),
            self.repository("example-interfaces"),
            self.repository("example-clients"),
            self.repository("example-api-server.rs"),
            self.repository("private-control-plane", private=True),
        ]
        manifest = publisher.build_manifest("example", repositories)
        serialized = json.dumps(manifest)
        self.assertNotIn("private-control-plane", serialized)
        self.assertEqual(1, manifest["privacy"]["private_repository_count"])
        self.assertFalse(
            manifest["privacy"]["private_repository_names_published"]
        )

    def test_internal_relationships_follow_contract_direction(self) -> None:
        repositories = [
            publisher.public_repository_entry(self.repository(".github")),
            publisher.public_repository_entry(
                self.repository("example-interfaces")
            ),
            publisher.public_repository_entry(self.repository("example-clients")),
            publisher.public_repository_entry(
                self.repository("example-api-server.rs")
            ),
            publisher.public_repository_entry(self.repository("example-ui.dart")),
            publisher.public_repository_entry(
                self.repository("example-mcp-server.rs")
            ),
            publisher.public_repository_entry(self.repository("example-infra")),
            publisher.public_repository_entry(self.repository("example-e2e")),
        ]
        relationships = publisher.build_internal_relationships(
            "example",
            repositories,
        )
        triples = {
            (item["from"], item["kind"], item["to"])
            for item in relationships
        }
        self.assertIn(
            (
                "example/example-clients",
                "generated_from",
                "example/example-interfaces",
            ),
            triples,
        )
        self.assertIn(
            (
                "example/example-api-server.rs",
                "implements_contracts_from",
                "example/example-interfaces",
            ),
            triples,
        )
        self.assertIn(
            (
                "example/example-infra",
                "deploys",
                "example/example-api-server.rs",
            ),
            triples,
        )

    def test_managed_readme_block_is_idempotent_and_non_destructive(self) -> None:
        existing = "# Custom heading\n\nOwner-authored text.\n"
        body = publisher.relationship_readme_block("example")
        first = publisher.merge_managed_block(existing, body)
        second = publisher.merge_managed_block(first, body)
        self.assertEqual(first, second)
        self.assertIn("# Custom heading", first)
        self.assertIn("Owner-authored text.", first)
        self.assertEqual(1, first.count(publisher.BEGIN_MARKER))
        self.assertEqual(1, first.count(publisher.END_MARKER))

    def test_schema_manifest_and_composition_contract_agree(self) -> None:
        schema = publisher.relationship_schema()
        manifest = publisher.build_manifest(
            "example",
            [self.repository(".github")],
        )
        self.assertEqual(
            publisher.SCHEMA_VERSION,
            schema["properties"]["schema_version"]["const"],
        )
        self.assertEqual(publisher.SCHEMA_VERSION, manifest["schema_version"])
        self.assertEqual(
            "git-submodules",
            manifest["composition_policy"]["editable_source_composition"],
        )
        self.assertEqual(
            "zed-pkg",
            manifest["composition_policy"]["package_and_artifact_resolution"],
        )

    def test_workflow_is_bounded_to_current_fleet_and_one_time_challenge(self) -> None:
        workflow = WORKFLOW_PATH.read_text(encoding="utf-8")
        self.assertIn("github.repository == 'ORESoftware/k8s-cluster'", workflow)
        self.assertIn("github.event.issue.number == 615", workflow)
        self.assertIn("github.event.comment.user.login == 'ORESoftware'", workflow)
        self.assertIn("github.actor == 'ORESoftware'", workflow)
        self.assertIn(
            "ops-publish-current-org-dotgithub-relationships:615:20260808-v1",
            workflow,
        )
        self.assertIn("EXPECTED_COUNT: '62'", workflow)
        self.assertIn("publish_current_org_repository_relationships.py", workflow)
        self.assertIn("openssl genpkey", workflow)
        self.assertIn("rsa_oaep_md:sha256", workflow)
        self.assertIn('select(.user.login == "ORESoftware")', workflow)
        self.assertIn("item.get(\"verified\") is not True", workflow)
        self.assertNotIn("GITHUB_ENV", workflow)
        self.assertNotIn("upload-artifact", workflow)
        self.assertIsNone(
            re.search(
                r"(?:gh[pousr]_[A-Za-z0-9_]{20,}|github_pat_[A-Za-z0-9_]{20,})",
                workflow,
            )
        )

    def test_runbook_names_current_62_organization_scope(self) -> None:
        runbook = RUNBOOK_PATH.read_text(encoding="utf-8")
        self.assertGreaterEqual(runbook.count("62"), 3)
        self.assertNotIn("exactly 36", runbook)
        self.assertIn("repository-relationships.json", runbook)
        self.assertIn("RSA-OAEP-SHA256", runbook)

    def test_workflow_avoids_destructive_recovery_commands(self) -> None:
        lowered = WORKFLOW_PATH.read_text(encoding="utf-8").lower()
        forbidden = (
            "git stash",
            "git reset",
            "git clean",
            "git filter-repo",
            "git filter-branch",
            "git push --force",
            "git push --force-with-lease",
            "rm -rf",
            "find -delete",
            "terraform destroy",
            "pulumi destroy",
            "kubectl delete",
            "helm uninstall",
            "--no-verify",
        )
        for command in forbidden:
            with self.subTest(command=command):
                self.assertNotIn(command, lowered)


if __name__ == "__main__":
    unittest.main()
