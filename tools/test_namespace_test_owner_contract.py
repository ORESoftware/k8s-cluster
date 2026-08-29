#!/usr/bin/env python3
"""Adversarial tests for the *-test owner boundary contract."""
from __future__ import annotations

import contextlib
import copy
import io
import json
import unittest
from pathlib import Path

from namespace_test_owner_contract import (
    DEFAULT_REGISTRY,
    DEFAULT_RULES,
    build_report,
    main,
    validate_test_owner_contract,
)

SOURCE_ROOT = Path(__file__).resolve().parents[1]


def read_json(relative: str) -> dict:
    return json.loads((SOURCE_ROOT / relative).read_text(encoding="utf-8"))


class TestOwnerContractTests(unittest.TestCase):
    def setUp(self) -> None:
        self.registry = read_json(DEFAULT_REGISTRY)
        self.rules = read_json(DEFAULT_RULES)

    def validate(self, registry: dict | None = None, rules: dict | None = None):
        return validate_test_owner_contract(
            registry or self.registry,
            rules or self.rules,
            registry_path=DEFAULT_REGISTRY,
            rules_path=DEFAULT_RULES,
        )

    def test_checked_in_contract_has_expected_test_bindings(self) -> None:
        bindings, diagnostics = self.validate()
        self.assertEqual([], [item for item in diagnostics if item.severity == "error"])
        found = {(item.test_owner, item.canonical_owner) for item in bindings}
        self.assertTrue(
            {
                ("canonical-cloud-test", "canonical-cloud"),
                ("discrete-event-systems-test", "discrete-event-systems"),
                ("fiducia-cloud-test", "fiducia-cloud"),
                ("networking-components-test", "networking-components"),
                ("zed-pkg-test", "zed-pkg"),
            }.issubset(found)
        )

    def test_test_kind_requires_test_suffix(self) -> None:
        registry = copy.deepcopy(self.registry)
        owner = next(
            item
            for item in registry["spec"]["owners"]
            if item["namespaceId"] == "fiducia-cloud-test"
        )
        owner["namespaceId"] = "fiducia-cloud-ci"
        owner["githubOwner"] = "fiducia-cloud-ci"
        _, diagnostics = self.validate(registry=registry)
        self.assertIn("test-owner.suffix", {item.rule_id for item in diagnostics})

    def test_test_owner_requires_canonical_parent(self) -> None:
        registry = copy.deepcopy(self.registry)
        registry["spec"]["owners"] = [
            item
            for item in registry["spec"]["owners"]
            if item["namespaceId"] != "fiducia-cloud"
        ]
        _, diagnostics = self.validate(registry=registry)
        self.assertIn(
            "test-owner.canonical-missing",
            {item.rule_id for item in diagnostics},
        )

    def test_test_owner_aliases_are_rejected(self) -> None:
        registry = copy.deepcopy(self.registry)
        owner = next(
            item
            for item in registry["spec"]["owners"]
            if item["namespaceId"] == "fiducia-cloud-test"
        )
        owner["aliases"] = ["fiducia-ci"]
        _, diagnostics = self.validate(registry=registry)
        self.assertIn("test-owner.aliases", {item.rule_id for item in diagnostics})

    def test_test_suffixed_github_owner_requires_test_kind(self) -> None:
        registry = copy.deepcopy(self.registry)
        owner = next(
            item
            for item in registry["spec"]["owners"]
            if item["namespaceId"] == "fiducia-cloud-test"
        )
        owner["kind"] = "product"
        _, diagnostics = self.validate(registry=registry)
        self.assertIn("test-owner.github-kind", {item.rule_id for item in diagnostics})

    def test_test_owner_cannot_target_prod(self) -> None:
        rules = copy.deepcopy(self.rules)
        rules["spec"]["rules"].append(
            {
                "id": "test.invalid-prod",
                "owner": "fiducia-cloud-test",
                "environment": "prod",
                "targetTemplate": "fiducia-cloud-test/prod/canary/runtime",
                "consumers": [],
            }
        )
        _, diagnostics = self.validate(rules=rules)
        self.assertIn(
            "test-owner.prod-target",
            {item.rule_id for item in diagnostics},
        )

    def test_foreign_owner_cannot_write_test_root(self) -> None:
        rules = copy.deepcopy(self.rules)
        rules["spec"]["rules"].append(
            {
                "id": "test.foreign-write",
                "owner": "fiducia-cloud",
                "environment": "dev",
                "targetTemplate": "fiducia-cloud-test/dev/canary/runtime",
                "consumers": [],
            }
        )
        _, diagnostics = self.validate(rules=rules)
        self.assertIn(
            "test-owner.foreign-write",
            {item.rule_id for item in diagnostics},
        )

    def test_explicit_test_consumer_grant_is_allowed(self) -> None:
        rules = copy.deepcopy(self.rules)
        rule = next(
            item
            for item in rules["spec"]["rules"]
            if item["id"] == "remote-dev.fiducia"
        )
        rule["consumers"].append("fiducia-cloud-test")
        _, diagnostics = self.validate(rules=rules)
        self.assertNotIn(
            "test-owner.foreign-write",
            {item.rule_id for item in diagnostics},
        )

    def test_cli_emits_valid_json(self) -> None:
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            status = main(["--root", str(SOURCE_ROOT), "--format", "json"])
        report = json.loads(output.getvalue())
        self.assertEqual(0, status)
        self.assertTrue(report["valid"])

    def test_build_report_is_valid(self) -> None:
        report, status = build_report(SOURCE_ROOT)
        self.assertEqual(0, status)
        self.assertTrue(report["valid"])
        self.assertGreaterEqual(len(report["bindings"]), 5)


if __name__ == "__main__":
    unittest.main(verbosity=2)
