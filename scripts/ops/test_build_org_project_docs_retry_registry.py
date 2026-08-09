#!/usr/bin/env python3
from __future__ import annotations

import importlib.util
import json
import sys
import tempfile
import unittest
from pathlib import Path

MODULE_PATH = Path(__file__).with_name("build_org_project_docs_retry_registry.py")
SPEC = importlib.util.spec_from_file_location("org_project_docs_retry_registry", MODULE_PATH)
assert SPEC and SPEC.loader
MODULE = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = MODULE
SPEC.loader.exec_module(MODULE)


class RetryRegistryTests(unittest.TestCase):
    def setUp(self) -> None:
        self.rows = [
            MODULE.RegistryRow(
                "Alpha-org", "https://linear.app/denman/project/alpha"
            ),
            MODULE.RegistryRow(
                "beta-org", "https://linear.app/denman/project/beta"
            ),
            MODULE.RegistryRow("Gamma", "https://linear.app/denman/project/gamma"),
            MODULE.RegistryRow(
                "delta-test", "https://linear.app/denman/project/delta"
            ),
        ]

    @staticmethod
    def audit(**overrides):
        payload = {
            "schema_version": 1,
            "expected_records": 4,
            "is_valid": False,
            "invalid": [],
            "missing_requested_orgs": [],
            "unexpected_requested_orgs": [],
        }
        payload.update(overrides)
        return payload

    def test_invalid_and_missing_union_preserves_registry_order_and_casing(self):
        retry_rows, summary = MODULE.build_retry_plan(
            self.rows,
            self.audit(
                invalid=[
                    {"requested_org": "BETA-ORG"},
                    {"requested_org": "Gamma"},
                ],
                missing_requested_orgs=["alpha-org", "delta-test"],
            ),
        )
        self.assertEqual(
            [row.organization for row in retry_rows],
            ["Alpha-org", "beta-org", "Gamma", "delta-test"],
        )
        self.assertEqual(summary["retry_records"], 4)
        self.assertEqual(summary["reason_counts"], {"invalid": 2, "missing": 2})

    def test_overlap_is_deduplicated_but_keeps_both_reasons(self):
        retry_rows, summary = MODULE.build_retry_plan(
            self.rows,
            self.audit(
                invalid=[{"requested_org": "beta-org"}],
                missing_requested_orgs=["BETA-ORG"],
            ),
        )
        self.assertEqual([row.organization for row in retry_rows], ["beta-org"])
        self.assertEqual(summary["reason_counts"], {"invalid": 1, "missing": 1})

    def test_valid_audit_writes_an_empty_retry_registry(self):
        retry_rows, summary = MODULE.build_retry_plan(
            self.rows,
            self.audit(is_valid=True),
        )
        self.assertEqual(retry_rows, [])
        self.assertEqual(summary["retry_records"], 0)

    def test_unknown_or_unexpected_organizations_fail_closed(self):
        with self.assertRaises(MODULE.RetryPlanError):
            MODULE.build_retry_plan(
                self.rows,
                self.audit(missing_requested_orgs=["not-managed"]),
            )
        with self.assertRaises(MODULE.RetryPlanError):
            MODULE.build_retry_plan(
                self.rows,
                self.audit(unexpected_requested_orgs=["beta-org"]),
            )

    def test_invalid_audit_cannot_silently_produce_no_retries(self):
        with self.assertRaises(MODULE.RetryPlanError):
            MODULE.build_retry_plan(self.rows, self.audit())

    def test_registry_and_summary_are_atomic_and_machine_readable(self):
        retry_rows, summary = MODULE.build_retry_plan(
            self.rows,
            self.audit(missing_requested_orgs=["Gamma"]),
        )
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            registry_path = directory / "retry.tsv"
            summary_path = directory / "summary.json"
            MODULE.write_registry(registry_path, retry_rows)
            MODULE.write_json(summary_path, summary)
            self.assertEqual(
                registry_path.read_text(encoding="utf-8"),
                "organization\tlinear_url\n"
                "Gamma\thttps://linear.app/denman/project/gamma\n",
            )
            self.assertEqual(
                json.loads(summary_path.read_text(encoding="utf-8"))[
                    "retry_records"
                ],
                1,
            )


if __name__ == "__main__":
    unittest.main()
