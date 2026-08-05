#!/usr/bin/env python3
from __future__ import annotations

from pathlib import Path
import sys
import unittest

ROOT = Path(__file__).resolve().parents[2]
OPS = ROOT / "scripts" / "ops"
if str(OPS) not in sys.path:
    sys.path.insert(0, str(OPS))

import bootstrap_org_dotgithub_repositories_all as module  # noqa: E402


class AllOrganizationGovernancePublisherTests(unittest.TestCase):
    def test_registry_covers_the_complete_bounded_fleet(self) -> None:
        module.validate_registry()
        self.assertEqual(61, len(module.TARGET_ORGANIZATIONS))
        self.assertEqual(61, len({name.lower() for name in module.TARGET_ORGANIZATIONS}))
        self.assertEqual(
            {name.lower() for name in module.TARGET_ORGANIZATIONS},
            {name.lower() for name in module.LINEAR_PROJECTS},
        )

    def test_r2g_accounts_remain_hard_excluded(self) -> None:
        targets = {name.lower() for name in module.TARGET_ORGANIZATIONS}
        self.assertTrue(module.EXCLUDED_ORGANIZATIONS.isdisjoint(targets))
        self.assertNotIn("r2g", targets)
        self.assertNotIn("r2g-test", targets)

    def test_new_production_organizations_have_canonical_linear_projects(self) -> None:
        expected = {
            "fanwaave",
            "agent-pontifex",
            "fifa-math",
            "gha-indie-worker",
            "apostille-me",
            "embedded-alerts",
            "evento-globolo",
            "hacker-house-medellin",
        }
        self.assertEqual(expected, set(module.ADDITIONAL_LINEAR_PROJECTS))
        for organization in expected:
            with self.subTest(organization=organization):
                self.assertIn(
                    "https://linear.app/denman/project/",
                    module.LINEAR_PROJECTS[organization],
                )

    def test_test_organizations_share_their_production_linear_project(self) -> None:
        self.assertEqual(17, len(module.PAIRED_TEST_PROJECTS))
        for test_organization, production_organization in module.PAIRED_TEST_PROJECTS.items():
            with self.subTest(test_organization=test_organization):
                self.assertTrue(test_organization.endswith("-test"))
                self.assertEqual(
                    module.LINEAR_PROJECTS[production_organization],
                    module.LINEAR_PROJECTS[test_organization],
                )

    def test_import_does_not_expand_the_reviewed_base_publisher(self) -> None:
        self.assertEqual(36, len(module.base.ORGANIZATIONS))
        self.assertEqual(36, len(module.hard.LINEAR_PROJECTS))

    def test_direct_runner_delegates_to_the_dedicated_61_org_launcher(self) -> None:
        runner = (ROOT / "scripts" / "ops" / "run_direct_org_dotgithub_publisher.sh").read_text(
            encoding="utf-8"
        )
        launcher = (
            ROOT / "scripts" / "ops" / "run_protected_org_dotgithub_all_publisher.sh"
        ).read_text(encoding="utf-8")

        self.assertIn("run_protected_org_dotgithub_all_publisher.sh", runner)
        self.assertIn("bootstrap_org_dotgithub_repositories_all.py", runner)
        self.assertNotIn("old_target_count", runner)
        self.assertNotIn("text.replace", runner)
        self.assertIn("bootstrap_org_dotgithub_repositories_all.py", launcher)
        self.assertIn("len(organizations) != 61", launcher)
        self.assertIn("publisher.TARGET_ORGANIZATIONS", launcher)
        self.assertIn("publisher.EXCLUDED_ORGANIZATIONS", launcher)


if __name__ == "__main__":
    unittest.main()
