#!/usr/bin/env python3
from __future__ import annotations

from pathlib import Path
import sys
import unittest

ROOT = Path(__file__).resolve().parents[2]
OPS = ROOT / "scripts" / "ops"
if str(OPS) not in sys.path:
    sys.path.insert(0, str(OPS))

import bootstrap_org_dotgithub_repositories_hardened as module  # noqa: E402


class HardenedGovernancePublisherTests(unittest.TestCase):
    def test_linear_registry_exactly_matches_fixed_allowlist(self) -> None:
        module.validate_registry()
        self.assertEqual(36, len(module.LINEAR_PROJECTS))
        self.assertEqual(
            {name.lower() for name in module.base.ORGANIZATIONS},
            {name.lower() for name in module.LINEAR_PROJECTS},
        )
        self.assertIn("https://linear.app/denman/project/", module.linear_project_url("StreemPilot"))

    def test_managed_paths_are_sane_public_markdown_defaults(self) -> None:
        self.assertEqual(
            {
                "README.md",
                "profile/README.md",
                "AGENTS.md",
                ".github/copilot-instructions.md",
                "CONTRIBUTING.md",
                "SECURITY.md",
                "SUPPORT.md",
                "CODE_OF_CONDUCT.md",
                "PULL_REQUEST_TEMPLATE.md",
            },
            set(module.MANAGED_PATHS),
        )

    def test_every_template_links_the_canonical_linear_project(self) -> None:
        linear = module.linear_project_url("fiducia-cloud")
        for path in module.MANAGED_PATHS:
            with self.subTest(path=path):
                rendered = module.render_managed_body(path, "fiducia-cloud")
                self.assertIn(linear, rendered)
                self.assertIn("semantic", rendered.lower())
                self.assertIn("full context", rendered.lower())

    def test_agents_preserves_exact_directive_and_blacklists_destructive_ops(self) -> None:
        rendered = module.render_managed_body("AGENTS.md", "sonus-auris")
        required = (
            module.ORIGINAL_SEMANTIC_DIRECTIVE,
            "at least 3 and up to 10 prior commits",
            "git stash",
            "git reset",
            "git clean",
            "git filter-repo",
            "git filter-branch",
            "git push --force",
            "rm -rf",
            "terraform destroy",
            "kubectl delete",
            "DROP",
            "TRUNCATE",
            "--no-verify",
            "prohibited by default",
        )
        for phrase in required:
            with self.subTest(phrase=phrase):
                self.assertIn(phrase, rendered)

    def test_copilot_and_pr_template_repeat_the_safety_boundary(self) -> None:
        copilot = module.render_managed_body(".github/copilot-instructions.md", "shared-auth")
        pull_request = module.render_managed_body("PULL_REQUEST_TEMPLATE.md", "shared-auth")
        for phrase in ("git stash", "git reset", "git clean", "git filter-repo"):
            self.assertIn(phrase, copilot)
            self.assertIn(phrase, pull_request)
        self.assertIn("3–10 relevant prior commits", copilot)
        self.assertIn("3–10 relevant prior commits", pull_request)

    def test_base_managed_block_preserves_unmanaged_content(self) -> None:
        original = "# Local organization content\n\nKeep this text.\n"
        managed = module.render_managed_body("README.md", "channelsiege")
        first = module.base.merge_managed_block(original, managed)
        second = module.base.merge_managed_block(first, managed)
        self.assertIn("Keep this text.", second)
        self.assertEqual(1, second.count(module.base.BEGIN_MARKER))
        self.assertEqual(1, second.count(module.base.END_MARKER))
        self.assertEqual(first, second)

    def test_import_does_not_patch_base_until_execution(self) -> None:
        original_paths = module.base.MANAGED_PATHS
        original_render = getattr(module.base, "render_managed_body", None)
        original_verify = getattr(module.base, "verify_organization", None)
        original_report = getattr(module.base, "markdown_report", None)
        try:
            module.install_hardening()
            self.assertEqual(module.MANAGED_PATHS, module.base.MANAGED_PATHS)
            self.assertIs(module.render_managed_body, module.base.render_managed_body)
            self.assertIs(module.verify_organization, module.base.verify_organization)
            self.assertIs(module.markdown_report, module.base.markdown_report)
        finally:
            module.base.MANAGED_PATHS = original_paths
            if original_render is not None:
                module.base.render_managed_body = original_render
            elif hasattr(module.base, "render_managed_body"):
                delattr(module.base, "render_managed_body")
            if original_verify is not None:
                module.base.verify_organization = original_verify
            elif hasattr(module.base, "verify_organization"):
                delattr(module.base, "verify_organization")
            if original_report is not None:
                module.base.markdown_report = original_report
            elif hasattr(module.base, "markdown_report"):
                delattr(module.base, "markdown_report")

    def test_report_contains_linear_and_denylist_without_token_material(self) -> None:
        result = module.base.OrganizationResult(
            organization="channelsiege",
            repository="channelsiege/.github",
            created_repository=True,
            changed_files=["AGENTS.md"],
            verified=True,
        )
        report = module.markdown_report([result], execute=True)
        self.assertIn(module.linear_project_url("channelsiege"), report)
        self.assertIn("git stash", report)
        self.assertIn("git filter-repo", report)
        self.assertNotIn("GH_TOKEN", report)
        self.assertNotIn("Authorization", report)

    def test_workflow_runs_hardened_publisher_and_all_focused_tests(self) -> None:
        workflow = (
            ROOT / ".github" / "workflows" / "ops-bootstrap-org-dotgithub-fleet.yml"
        ).read_text(encoding="utf-8")
        self.assertIn("bootstrap_org_dotgithub_repositories_hardened.py", workflow)
        self.assertIn("unittest discover", workflow)
        self.assertIn("github.actor == 'ORESoftware'", workflow)
        self.assertIn("github.event.issue.number == 615", workflow)
        self.assertIn(
            "ops-bootstrap-org-dotgithub:615:ad6e27250729ed647513a302d02fd767",
            workflow,
        )


if __name__ == "__main__":
    unittest.main()
