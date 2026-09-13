from __future__ import annotations

import importlib.util
import json
import sys
import tempfile
import unittest
from pathlib import Path

MODULE_PATH = Path(__file__).with_name("validate_cross_repo_workflow_dependencies.py")
SPEC = importlib.util.spec_from_file_location("validator", MODULE_PATH)
assert SPEC and SPEC.loader
validator = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = validator
SPEC.loader.exec_module(validator)


class ValidatorTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        (self.root / ".github/workflows").mkdir(parents=True)

    def tearDown(self) -> None:
        self.temp.cleanup()

    def write(self, path: str, content: str) -> None:
        target = self.root / path
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(content, encoding="utf-8")

    @staticmethod
    def ledger(dependencies=None, exceptions=None):
        return {
            "schema_version": 1,
            "dependencies": dependencies or [],
            "feature_ref_exceptions": exceptions or [],
        }

    def run_report(self, ledger, as_of="2026-08-01"):
        return validator.build_report(
            ledger, self.root, validator.parse_date(as_of, "test")
        )

    def test_named_and_shorthand_checkout_steps_do_not_cross_boundaries(self):
        sha = "a" * 40
        workflow = ".github/workflows/contracts.yml"
        text = f"""env:
  SERVICE_REVISION: {sha}
jobs:
  test:
    steps:
      - name: Check out service
        uses: actions/checkout@deadbeef
        with:
          repository: Acme/service
          ref: ${{{{ env.SERVICE_REVISION }}}}
      - uses: actions/checkout@deadbeef
        with:
          repository: Acme/contracts
      - name: Run tests
        run: true
"""
        self.write(workflow, text)
        blocks = validator.parse_checkout_blocks(text, workflow)
        self.assertEqual(
            [(block.repository, block.ref_value) for block in blocks],
            [
                ("Acme/service", "${{ env.SERVICE_REVISION }}"),
                ("Acme/contracts", None),
            ],
        )
        ledger = self.ledger(
            [
                {
                    "workflow": workflow,
                    "repository": "Acme/service",
                    "ref_policy": "immutable_commit",
                    "expected_ref": sha,
                    "ref_source": "env:SERVICE_REVISION",
                    "owning_issue": "DEN-1321",
                },
                {
                    "workflow": workflow,
                    "repository": "Acme/contracts",
                    "ref_policy": "default_branch",
                    "owning_issue": "DEN-1321",
                },
            ]
        )
        report, status = self.run_report(ledger)
        self.assertEqual(status, 0, report)

    def test_literal_immutable_commit_passes(self):
        sha = "b" * 40
        workflow = ".github/workflows/literal.yml"
        self.write(
            workflow,
            f"jobs:\n  test:\n    steps:\n      - uses: actions/checkout@deadbeef\n        with:\n          repository: Acme/service\n          ref: {sha}\n",
        )
        report, status = self.run_report(
            self.ledger(
                [
                    {
                        "workflow": workflow,
                        "repository": "Acme/service",
                        "ref_policy": "immutable_commit",
                        "expected_ref": sha,
                        "ref_source": "literal",
                        "owning_issue": "DEN-1321",
                    }
                ]
            )
        )
        self.assertEqual(status, 0, report)

    def test_unapproved_feature_ref_fails(self):
        self.write(
            ".github/workflows/stale.yml",
            "jobs:\n  test:\n    steps:\n      - uses: actions/checkout@deadbeef\n        with:\n          repository: Acme/service\n          ref: agent/old-branch\n",
        )
        report, status = self.run_report(self.ledger())
        self.assertEqual(status, 1)
        self.assertEqual(report["findings"][0]["code"], "unapproved-feature-ref")

    def test_current_exception_passes_and_expired_exception_fails(self):
        self.write(
            ".github/workflows/temporary.yml",
            "on:\n  workflow_dispatch:\n    inputs:\n      upstream_ref:\n        default: agent/reviewed\n",
        )
        exception = {
            "workflow": ".github/workflows/temporary.yml",
            "owning_issue": "DEN-1321",
            "expires_on": "2026-08-15",
            "reason": "reviewed migration window",
        }
        report, status = self.run_report(self.ledger(exceptions=[exception]))
        self.assertEqual(status, 0, report)
        report, status = self.run_report(
            self.ledger(exceptions=[exception]), as_of="2026-08-16"
        )
        self.assertEqual(status, 1)
        self.assertIn(
            "exception-expired", {item["code"] for item in report["findings"]}
        )

    def test_stale_exception_fails_cleanup_ratchet(self):
        self.write(".github/workflows/clean.yml", "jobs: {}\n")
        report, status = self.run_report(
            self.ledger(
                exceptions=[
                    {
                        "workflow": ".github/workflows/clean.yml",
                        "owning_issue": "DEN-1321",
                        "expires_on": "2026-08-15",
                        "reason": "should be removed",
                    }
                ]
            )
        )
        self.assertEqual(status, 1)
        self.assertEqual(report["findings"][0]["code"], "exception-stale")

    def test_default_branch_policy_rejects_explicit_ref(self):
        workflow = ".github/workflows/default.yml"
        self.write(
            workflow,
            "jobs:\n  test:\n    steps:\n      - uses: actions/checkout@deadbeef\n        with:\n          repository: Acme/contracts\n          ref: main\n",
        )
        report, status = self.run_report(
            self.ledger(
                [
                    {
                        "workflow": workflow,
                        "repository": "Acme/contracts",
                        "ref_policy": "default_branch",
                        "owning_issue": "DEN-1321",
                    }
                ]
            )
        )
        self.assertEqual(status, 1)
        self.assertEqual(report["findings"][0]["code"], "default-branch-has-ref")

    def test_feature_dependency_must_be_unexpired(self):
        workflow = ".github/workflows/feature.yml"
        self.write(
            workflow,
            "jobs:\n  test:\n    steps:\n      - uses: actions/checkout@deadbeef\n        with:\n          repository: Acme/service\n          ref: agent/reviewed\n",
        )
        row = {
            "workflow": workflow,
            "repository": "Acme/service",
            "ref_policy": "feature_branch",
            "expected_ref": "agent/reviewed",
            "owning_issue": "DEN-1321",
            "owning_pr": 7,
            "expires_on": "2026-07-31",
        }
        exception = {
            "workflow": workflow,
            "owning_issue": "DEN-1321",
            "expires_on": "2026-08-15",
            "reason": "reviewed feature dependency",
        }
        report, status = self.run_report(self.ledger([row], [exception]))
        self.assertEqual(status, 1)
        self.assertIn(
            "feature-ref-expired", {item["code"] for item in report["findings"]}
        )

    def test_json_report_is_stable(self):
        self.write(".github/workflows/clean.yml", "jobs: {}\n")
        report, status = self.run_report(self.ledger())
        self.assertEqual(status, 0)
        encoded = json.dumps(report, sort_keys=True)
        self.assertIn('"errors": 0', encoded)


if __name__ == "__main__":
    unittest.main()
