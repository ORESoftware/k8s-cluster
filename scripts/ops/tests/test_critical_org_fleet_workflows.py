from __future__ import annotations

import re
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
WORKFLOWS = ROOT / ".github" / "workflows"
CANONICAL = WORKFLOWS / "ops-canonical-critical-org-fleet-publication.yml"
BROWSER_WORKFLOW = WORKFLOWS / "critical-org-fleet-browser-canary.yml"
BROWSER_TEST = (
    ROOT
    / "remote"
    / "tests"
    / "browser"
    / "critical-org-fleet-publication.test.mjs"
)
OBSOLETE = (
    WORKFLOWS / "ops-supervise-full-critical-org-fleet-publication.yml",
    WORKFLOWS / "ops-dispatch-full-critical-org-fleet-publication.yml",
    WORKFLOWS / "ops-trigger-full-critical-org-fleet-publication.yml",
)


class CriticalOrgFleetWorkflowContracts(unittest.TestCase):
    def test_only_one_owner_mutation_workflow_remains(self) -> None:
        self.assertTrue(CANONICAL.is_file())
        for path in OBSOLETE:
            self.assertFalse(
                path.exists(),
                f"obsolete competing workflow remains: {path}",
            )
        text = CANONICAL.read_text(encoding="utf-8")
        self.assertIn("group: critical-org-fleet-publication", text)
        self.assertEqual(text.count("gh auth login"), 1)
        self.assertEqual(
            text.count(
                "\n          python3 scripts/ops/"
                "publish_missing_org_repositories_current.py"
            ),
            1,
        )

    def test_carrier_code_is_never_checked_out(self) -> None:
        text = CANONICAL.read_text(encoding="utf-8")
        self.assertIn("id: carrier", text)
        self.assertIn(
            'echo "main_sha=$main_sha" >> "$GITHUB_OUTPUT"',
            text,
        )
        self.assertIn(
            "ref: ${{ steps.carrier.outputs.main_sha }}",
            text,
        )
        self.assertNotIn(
            "ref: ${{ steps.carrier.outputs.head_sha }}",
            text,
        )
        self.assertIn("persist-credentials: false", text)

    def test_report_gate_is_strict_and_not_neutralized(self) -> None:
        text = CANONICAL.read_text(encoding="utf-8")
        self.assertIn(".success == true", text)
        self.assertIn(".organizations.hypesiege.count == 15", text)
        self.assertIn(".organizations.StreemPilot.count == 17", text)
        self.assertIn("(.extracted_repositories | length) == 2", text)
        self.assertNotRegex(
            text,
            re.compile(
                r"(?:test|jq)[^\n]*report_json[^\n]*\|\|\s*true"
            ),
        )
        self.assertIn("if-no-files-found: warn", text)
        self.assertIn("retention-days: 14", text)

    def test_device_code_is_not_used_as_a_status_context(self) -> None:
        text = CANONICAL.read_text(encoding="utf-8")
        self.assertIn("'critical-org-fleet/device-auth'", text)
        self.assertNotIn(
            'code_context="critical-org-fleet/device-auth/$code"',
            text,
        )

    def test_browser_canary_has_hermetic_and_trusted_live_modes(self) -> None:
        workflow = BROWSER_WORKFLOW.read_text(encoding="utf-8")
        test_source = BROWSER_TEST.read_text(encoding="utf-8")
        self.assertIn("workflow_run:", workflow)
        self.assertIn(
            "Canonical critical organization fleet publication",
            workflow,
        )
        self.assertIn(
            "github.event.workflow_run.conclusion == 'success'",
            workflow,
        )
        self.assertIn("ref: main", workflow)
        self.assertNotIn("${{ secrets.", workflow)
        self.assertIn("permissions:\n  contents: read", workflow)
        self.assertIn("CRITICAL_ORG_FLEET_LIVE: '1'", workflow)
        self.assertIn(
            "node --test browser/critical-org-fleet-publication.test.mjs",
            workflow,
        )
        self.assertIn("validateManifest", test_source)
        self.assertIn(
            "octolytics-dimension-repository_nwo",
            test_source,
        )
        self.assertIn("assert.rejects", test_source)
        self.assertIn("repository_count, 32", test_source)
        self.assertIn('visibility, "public"', test_source)


if __name__ == "__main__":
    unittest.main()
