#!/usr/bin/env python3
from __future__ import annotations

import importlib.util
from pathlib import Path
import sys
import unittest

ROOT = Path(__file__).resolve().parents[2]
MODULE_PATH = ROOT / "scripts" / "ops" / "bootstrap_org_dotgithub_repositories.py"
SPEC = importlib.util.spec_from_file_location("org_dotgithub", MODULE_PATH)
assert SPEC and SPEC.loader
module = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = module
SPEC.loader.exec_module(module)


class GovernancePublisherTests(unittest.TestCase):
    def test_allowlist_is_fixed_and_unique(self) -> None:
        self.assertEqual(36, len(module.ORGANIZATIONS))
        self.assertEqual(36, len({name.lower() for name in module.ORGANIZATIONS}))
        self.assertIn("fiducia-cloud", module.ORGANIZATIONS)
        self.assertIn("StreemPilot", module.ORGANIZATIONS)
        self.assertEqual(".github", module.REPOSITORY_NAME)

    def test_managed_paths_are_exact(self) -> None:
        self.assertEqual(
            {
                "README.md",
                "profile/README.md",
                "AGENTS.md",
                ".github/copilot-instructions.md",
                "CONTRIBUTING.md",
                "PULL_REQUEST_TEMPLATE.md",
            },
            set(module.MANAGED_PATHS),
        )

    def test_merge_preserves_unmanaged_content_and_is_idempotent(self) -> None:
        original = "# Custom profile\n\nKeep this organization-specific text.\n"
        first = module.merge_managed_block(original, "# Managed\n\nVersion one")
        self.assertTrue(first.startswith(original.rstrip()))
        self.assertIn("Keep this organization-specific text.", first)
        self.assertIn(module.BEGIN_MARKER, first)
        second = module.merge_managed_block(first, "# Managed\n\nVersion two")
        self.assertEqual(1, second.count(module.BEGIN_MARKER))
        self.assertNotIn("Version one", second)
        self.assertIn("Version two", second)
        self.assertIn("Keep this organization-specific text.", second)

    def test_malformed_markers_fail_closed(self) -> None:
        with self.assertRaises(ValueError):
            module.merge_managed_block(f"prefix\n{module.BEGIN_MARKER}\n", "body")

    def test_every_template_contains_semantic_policy(self) -> None:
        for path in module.MANAGED_PATHS:
            with self.subTest(path=path):
                rendered = module.render_managed_body(path, "example-org")
                self.assertIn("semantic", rendered.lower())
                self.assertIn("full context", rendered.lower())
                self.assertIn("ours", rendered)
                self.assertIn("theirs", rendered)
                self.assertIn("external", rendered.lower())
                self.assertTrue(
                    "3 and up to 10" in rendered or "3–10" in rendered,
                    rendered,
                )

    def test_readme_states_non_inheritance_caveat(self) -> None:
        rendered = module.render_managed_body("README.md", "example-org")
        self.assertIn("not automatically inherited", rendered)
        self.assertIn("fallback defaults", rendered)

    def test_workflow_is_secret_isolated_and_bounded(self) -> None:
        workflow = (ROOT / ".github" / "workflows" / "ops-bootstrap-org-dotgithub-fleet.yml").read_text()
        before_publish, publish = workflow.split("  publish:\n", 1)
        self.assertNotIn("id-token: write", before_publish)
        self.assertNotIn("issues: write", before_publish)
        self.assertIn("id-token: write", publish)
        self.assertIn("issues: write", publish)
        self.assertIn("github.actor == 'ORESoftware'", publish)
        self.assertIn("github.event.issue.number == 615", publish)
        self.assertIn(
            "ops-bootstrap-org-dotgithub:615:ad6e27250729ed647513a302d02fd767",
            publish,
        )

    def test_report_contains_no_token_material(self) -> None:
        result = module.OrganizationResult(
            organization="example-org",
            repository="example-org/.github",
            created_repository=True,
            changed_files=["README.md"],
            verified=True,
        )
        report = module.markdown_report([result], execute=True)
        self.assertNotIn("GH_TOKEN", report)
        self.assertNotIn("Authorization", report)
        self.assertIn("example-org/.github", report)


if __name__ == "__main__":
    unittest.main()
