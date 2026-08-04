from __future__ import annotations

import ast
import importlib.util
import json
import sys
import types
from pathlib import Path
import unittest

MODULE_PATH = (
    Path(__file__).resolve().parents[2]
    / "scripts"
    / "ops"
    / "publish_org_repository_relationships.py"
)
WORKFLOW_PATH = (
    Path(__file__).resolve().parents[2]
    / ".github"
    / "workflows"
    / "ops-org-dotgithub-relationships-ephemeral-publish.yml"
)
if "bootstrap_org_dotgithub_repositories" not in sys.modules:
    bootstrap_path = MODULE_PATH.with_name("bootstrap_org_dotgithub_repositories.py")
    if not bootstrap_path.exists():
        stub = types.ModuleType("bootstrap_org_dotgithub_repositories")
        stub.ORGANIZATIONS = (
            "channelsiege", "OmniBlitz", "streamkore", "hypeblitz", "3FA-app",
            "messaging-intel", "akrion-sim", "athlet-o", "benefactor-cc",
            "canonical-cloud", "claritas-viz", "cliptown", "daedalus-fab",
            "declarative-migrations", "fiducia-cloud", "anticaptrad", "opto-sync",
            "quaestor-ledger", "sagitta-stack", "shared-auth", "scintilla-run",
            "rust-ssr-demos", "sonus-auris", "usa-acc", "voxletra", "zed-pkg",
            "zed-pkg-test", "memebank", "meta-agents-demo", "networking-components",
            "StreemPilot", "unreal-unity-poc", "file-tunnel", "hypesiege",
            "discrete-event-systems", "drone-mngr",
        )
        stub.EXPECTED_ACTOR = "ORESoftware"
        sys.modules[stub.__name__] = stub

spec = importlib.util.spec_from_file_location("publish_org_repository_relationships", MODULE_PATH)
module = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = module
assert spec.loader is not None
spec.loader.exec_module(module)


class RepositoryRelationshipPublisherTests(unittest.TestCase):
    def repository(
        self,
        name: str,
        *,
        private: bool = False,
        visibility: str | None = None,
        description: str | None = None,
        archived: bool = False,
    ) -> dict:
        return {
            "name": name,
            "full_name": f"example/{name}",
            "private": private,
            "visibility": visibility or ("private" if private else "public"),
            "description": description,
            "archived": archived,
            "fork": False,
            "default_branch": "main",
        }

    def test_fixed_organization_allowlist_is_complete_and_unique(self) -> None:
        self.assertEqual(36, len(module.ORGANIZATIONS))
        self.assertEqual(36, len({value.lower() for value in module.ORGANIZATIONS}))
        self.assertIn("sonus-auris", module.ORGANIZATIONS)
        self.assertIn("StreemPilot", module.ORGANIZATIONS)
        self.assertIn("3FA-app", module.ORGANIZATIONS)

    def test_allowlist_matches_the_canonical_dotgithub_bootstrap_publisher(self) -> None:
        bootstrap_path = (
            Path(__file__).resolve().parents[2]
            / "scripts"
            / "ops"
            / "bootstrap_org_dotgithub_repositories.py"
        )
        if not bootstrap_path.exists():
            self.skipTest("canonical bootstrap publisher is available in the full repository checkout")
        tree = ast.parse(bootstrap_path.read_text(encoding="utf-8"))
        canonical = None
        for statement in tree.body:
            if not isinstance(statement, ast.AnnAssign):
                continue
            if isinstance(statement.target, ast.Name) and statement.target.id == "ORGANIZATIONS":
                canonical = ast.literal_eval(statement.value)
                break
        self.assertIsNotNone(canonical)
        self.assertEqual(tuple(canonical), module.ORGANIZATIONS)

    def test_classifies_canonical_repository_roles(self) -> None:
        cases = {
            ".github": "organization_governance",
            "sonus-auris-interfaces": "interfaces",
            "sonus-auris-clients": "client_sdk",
            "sonus-auris-api-server.rs": "api_service",
            "sonus-auris-web-server.rs": "web_bff",
            "sonus-auris-sync": "sync_service",
            "sonus-auris-mcp-server.rs": "mcp_server",
            "sonus-auris.infra": "infrastructure",
            "sonus-auris-e2e": "end_to_end_tests",
            "sonus-auris-monorepo": "composition_workspace",
            "cliptown-extension": "browser_extension",
            "cliptown-cli": "cli",
            "cliptown.github.io": "site",
            "desktop.app.rs": "application",
        }
        for name, expected in cases.items():
            with self.subTest(name=name):
                self.assertEqual(expected, module.classify_repository(name))

    def test_manifest_never_publishes_private_repository_names(self) -> None:
        repositories = [
            self.repository(".github"),
            self.repository("example-interfaces"),
            self.repository("example-clients"),
            self.repository("example-api-server.rs"),
            self.repository("private-control-plane", private=True),
        ]
        manifest = module.build_manifest("example", repositories)
        serialized = json.dumps(manifest)
        self.assertNotIn("private-control-plane", serialized)
        self.assertEqual(1, manifest["privacy"]["private_repository_count"])
        self.assertFalse(manifest["privacy"]["private_repository_names_published"])
        self.assertEqual(
            {".github", "example-interfaces", "example-clients", "example-api-server.rs"},
            {item["name"] for item in manifest["repositories"]},
        )

    def test_internal_edges_follow_contract_direction(self) -> None:
        repositories = [
            module.public_repository_entry(self.repository(".github")),
            module.public_repository_entry(self.repository("example-interfaces")),
            module.public_repository_entry(self.repository("example-clients")),
            module.public_repository_entry(self.repository("example-api-server.rs")),
            module.public_repository_entry(self.repository("example-ui.dart")),
            module.public_repository_entry(self.repository("example-mcp-server.rs")),
            module.public_repository_entry(self.repository("example-sync")),
            module.public_repository_entry(self.repository("example-infra")),
            module.public_repository_entry(self.repository("example-e2e")),
            module.public_repository_entry(self.repository("example-monorepo")),
        ]
        relationships = module.build_internal_relationships("example", repositories)
        triples = {(item["from"], item["kind"], item["to"]) for item in relationships}
        self.assertIn(("example/example-clients", "generated_from", "example/example-interfaces"), triples)
        self.assertIn(("example/example-api-server.rs", "implements_contracts_from", "example/example-interfaces"), triples)
        self.assertIn(("example/example-ui.dart", "calls", "example/example-api-server.rs"), triples)
        self.assertIn(("example/example-mcp-server.rs", "uses_sdk", "example/example-clients"), triples)
        self.assertIn(("example/example-sync", "synchronizes_with", "example/example-api-server.rs"), triples)
        self.assertIn(("example/example-infra", "deploys", "example/example-api-server.rs"), triples)
        self.assertIn(("example/example-e2e", "tests", "example/example-ui.dart"), triples)
        self.assertIn(("example/example-monorepo", "composes", "example/example-clients"), triples)

    def test_platform_edges_are_conditional_and_explicit(self) -> None:
        repositories = [
            module.public_repository_entry(self.repository("sonus-auris-api-server.rs")),
            module.public_repository_entry(self.repository("sonus-auris-sync")),
            module.public_repository_entry(self.repository("sonus-auris-mcp-server.rs")),
        ]
        relationships = module.build_external_relationships("sonus-auris", repositories)
        targets = {(item["kind"], item["to"]) for item in relationships}
        self.assertIn(("deployed_via", "platform://ORESoftware/k8s-cluster"), targets)
        self.assertIn(("packaged_via", "platform://zed-pkg"), targets)
        self.assertIn(("reconciles_via", "platform://opto-sync"), targets)
        self.assertIn(("uses_transport_library", "platform://ORESoftware/mcp-rust-libs"), targets)
        self.assertIn(("authenticates_via", "capability://shared-auth/human-identity"), targets)
        self.assertIn(("coordinates_via", "capability://fiducia-cloud/distributed-coordination"), targets)
        self.assertIn(("uses_capability", "organization://3FA-app"), targets)

    def test_managed_block_preserves_unmanaged_content_and_is_idempotent(self) -> None:
        existing = "# Custom heading\n\nOwner-authored text.\n"
        body = module.relationship_readme_block("example")
        first = module.merge_managed_block(existing, body)
        second = module.merge_managed_block(first, body)
        self.assertEqual(first, second)
        self.assertIn("# Custom heading", first)
        self.assertIn("Owner-authored text.", first)
        self.assertEqual(1, first.count(module.BEGIN_MARKER))
        self.assertEqual(1, first.count(module.END_MARKER))

    def test_malformed_markers_are_rejected(self) -> None:
        with self.assertRaises(ValueError):
            module.merge_managed_block(f"prefix\n{module.BEGIN_MARKER}\nunterminated\n", "replacement")

    def test_schema_and_manifest_agree_on_version(self) -> None:
        schema = module.relationship_schema()
        self.assertEqual(module.SCHEMA_VERSION, schema["properties"]["schema_version"]["const"])
        self.assertIsNotNone(
            __import__("re").fullmatch(
                schema["properties"]["registry_repository"]["pattern"],
                "example/.github",
            )
        )
        manifest = module.build_manifest("empty", [self.repository(".github")])
        self.assertEqual(module.SCHEMA_VERSION, manifest["schema_version"])
        self.assertEqual("git-submodules", manifest["composition_policy"]["editable_source_composition"])
        self.assertEqual("zed-pkg", manifest["composition_policy"]["package_and_artifact_resolution"])

    def test_markdown_explains_privacy_and_composition_policy(self) -> None:
        manifest = module.build_manifest("example", [self.repository(".github"), self.repository("example-api-server.rs")])
        markdown = module.render_markdown(manifest)
        self.assertIn("Private repository names", markdown)
        self.assertIn("Git submodules", markdown)
        self.assertIn("Zed packages", markdown)
        self.assertIn("immutable image digests", markdown)

    def test_workflow_has_exact_owner_issue_trigger_and_ephemeral_challenge(self) -> None:
        workflow = WORKFLOW_PATH.read_text(encoding="utf-8")
        self.assertIn("github.repository == 'ORESoftware/k8s-cluster'", workflow)
        self.assertIn("github.event.issue.number == 615", workflow)
        self.assertIn("github.event.comment.user.login == 'ORESoftware'", workflow)
        self.assertIn("github.actor == 'ORESoftware'", workflow)
        self.assertIn("ops-bootstrap-org-dotgithub-relationships-ephemeral:615:20260804-v1", workflow)
        self.assertIn("openssl genpkey", workflow)
        self.assertIn("rsa_oaep_md:sha256", workflow)
        self.assertIn('select(.user.login == "ORESoftware")', workflow)
        self.assertNotIn("GITHUB_ENV", workflow)
        self.assertNotIn("upload-artifact", workflow)

    def test_workflow_allowlist_matches_publisher_and_runs_both_publications(self) -> None:
        workflow = WORKFLOW_PATH.read_text(encoding="utf-8")
        match = __import__("re").search(
            r"OWNER_ORGANIZATIONS: \|-\n(?P<body>(?:        .+\n)+?)\n    steps:",
            workflow,
        )
        self.assertIsNotNone(match)
        observed = tuple(line.strip() for line in match.group("body").splitlines() if line.strip())
        self.assertEqual(module.ORGANIZATIONS, observed)
        self.assertIn("bootstrap_org_dotgithub_repositories_hardened.py", workflow)
        self.assertIn("publish_org_repository_relationships.py", workflow)
        self.assertIn("private repo name(s) withheld", workflow)
        self.assertGreaterEqual(workflow.count('if len(organizations) != 36:'), 2)
        self.assertGreaterEqual(workflow.count('item.get("verified") is not True'), 2)

    def test_workflow_avoids_blacklisted_destructive_commands(self) -> None:
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
