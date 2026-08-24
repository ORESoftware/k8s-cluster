#!/usr/bin/env python3
"""Adversarial tests for the DEN-2786 plaintext migration manifest."""
from __future__ import annotations

import contextlib
import copy
import io
import json
import unittest
from pathlib import Path

from namespace_manifest import (
    DEFAULT_INVENTORY,
    DEFAULT_MANIFEST,
    DEFAULT_REGISTRY,
    DEFAULT_SCHEMA,
    ENTRY_FIELDS,
    build_manifest,
    canonical_json,
    check_manifest,
    main,
    owner_index,
    validate_manifest_semantics,
)

SOURCE_ROOT = Path(__file__).resolve().parents[1]


class CheckedInManifestTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        result = build_manifest(SOURCE_ROOT)
        if not result.valid or result.manifest is None:
            raise AssertionError(result.diagnostics)
        cls.generated = result.manifest
        cls.inventory = json.loads(
            (SOURCE_ROOT / DEFAULT_INVENTORY).read_text(encoding="utf-8")
        )
        cls.registry = json.loads(
            (SOURCE_ROOT / DEFAULT_REGISTRY).read_text(encoding="utf-8")
        )
        cls.owners, diagnostics = owner_index(cls.registry)
        if diagnostics:
            raise AssertionError(diagnostics)

    def diagnostics_for(self, manifest: dict) -> set[str]:
        return {
            item.rule_id
            for item in validate_manifest_semantics(
                manifest,
                self.inventory,
                self.owners,
                inventory_path=DEFAULT_INVENTORY,
            )
        }

    def test_committed_manifest_is_exact_deterministic_output(self) -> None:
        committed = (SOURCE_ROOT / DEFAULT_MANIFEST).read_text(encoding="utf-8")
        self.assertEqual(canonical_json(self.generated), committed)
        report, status = check_manifest(SOURCE_ROOT)
        self.assertEqual(0, status, report)
        self.assertTrue(report["valid"])

    def test_manifest_has_one_row_per_inventory_identity(self) -> None:
        entries = self.generated["spec"]["entries"]
        occurrences = self.inventory["occurrences"]
        self.assertEqual(1276, len(entries))
        self.assertEqual(len(occurrences), len(entries))
        inventory_identities = {
            (
                item["path"],
                item["line"],
                item["column"],
                item["system"],
                item["reference"],
            )
            for item in occurrences
        }
        manifest_identities = {
            (
                item["path"],
                item["line"],
                item["column"],
                item["system"],
                item["current"],
            )
            for item in entries
        }
        self.assertEqual(inventory_identities, manifest_identities)
        self.assertEqual(len(entries), len(manifest_identities))

    def test_generation_is_stable_across_repeated_builds(self) -> None:
        second = build_manifest(SOURCE_ROOT)
        self.assertTrue(second.valid)
        self.assertEqual(canonical_json(self.generated), canonical_json(second.manifest))

    def test_all_rows_have_review_verification_and_rollback_controls(self) -> None:
        for entry in self.generated["spec"]["entries"]:
            with self.subTest(entry=entry["id"]):
                self.assertEqual(ENTRY_FIELDS, set(entry))
                self.assertFalse(entry["destructiveCleanupAllowed"])
                self.assertIn(entry["reviewState"], {"classified", "review-required", "blocked"})
                self.assertTrue(entry["verification"]["procedure"])
                self.assertTrue(entry["rollback"]["procedure"])

    def test_unclassified_rows_are_non_actionable(self) -> None:
        blocked = [
            item
            for item in self.generated["spec"]["entries"]
            if item["owner"] == "unclassified"
        ]
        self.assertEqual(683, len(blocked))
        for entry in blocked:
            self.assertIsNone(entry["target"])
            self.assertEqual("blocked", entry["reviewState"])
            self.assertEqual("manual-review", entry["migrationMode"])
            self.assertEqual([], entry["consumers"])
            self.assertEqual([], entry["consumerGrants"])

    def test_cross_owner_consumers_have_explicit_read_grants(self) -> None:
        for entry in self.generated["spec"]["entries"]:
            expected = sorted(
                consumer
                for consumer in entry["consumers"]
                if consumer != entry["owner"]
            )
            actual = sorted(grant["consumer"] for grant in entry["consumerGrants"])
            self.assertEqual(expected, actual)
            self.assertTrue(
                all(grant["access"] == "read" for grant in entry["consumerGrants"])
            )

    def test_duplicate_identity_is_rejected(self) -> None:
        manifest = copy.deepcopy(self.generated)
        manifest["spec"]["entries"].append(
            copy.deepcopy(manifest["spec"]["entries"][0])
        )
        manifest["metadata"]["entryCount"] += 1
        self.assertIn("manifest.duplicate-identity", self.diagnostics_for(manifest))

    def test_missing_identity_is_rejected(self) -> None:
        manifest = copy.deepcopy(self.generated)
        manifest["spec"]["entries"].pop()
        manifest["metadata"]["entryCount"] -= 1
        self.assertIn("manifest.missing-identity", self.diagnostics_for(manifest))

    def test_unknown_owner_is_rejected(self) -> None:
        manifest = copy.deepcopy(self.generated)
        entry = manifest["spec"]["entries"][0]
        entry["owner"] = "not-a-registered-owner"
        self.assertIn("manifest.unknown-owner", self.diagnostics_for(manifest))

    def test_missing_cross_owner_grant_is_rejected(self) -> None:
        manifest = copy.deepcopy(self.generated)
        entry = next(
            item
            for item in manifest["spec"]["entries"]
            if item["consumerGrants"]
        )
        entry["consumerGrants"] = []
        self.assertIn("manifest.cross-owner-grant", self.diagnostics_for(manifest))

    def test_unknown_consumer_is_rejected(self) -> None:
        manifest = copy.deepcopy(self.generated)
        entry = next(
            item
            for item in manifest["spec"]["entries"]
            if item["owner"] != "unclassified"
        )
        entry["consumers"].append("unknown-test-owner")
        entry["consumerGrants"].append(
            {"access": "read", "consumer": "unknown-test-owner", "state": "required"}
        )
        diagnostics = self.diagnostics_for(manifest)
        self.assertIn("manifest.unknown-consumer", diagnostics)
        self.assertIn("manifest.unknown-grant-consumer", diagnostics)

    def test_distinct_sources_cannot_collapse_to_one_target(self) -> None:
        manifest = copy.deepcopy(self.generated)
        concrete = [
            item for item in manifest["spec"]["entries"] if item["target"] is not None
        ]
        first = concrete[0]
        second = next(
            item
            for item in concrete[1:]
            if (
                item["system"],
                item["current"],
                item["owner"],
            )
            != (
                first["system"],
                first["current"],
                first["owner"],
            )
        )
        second["target"] = first["target"]
        second["targetTemplate"] = first["target"]
        self.assertIn("manifest.target-collision", self.diagnostics_for(manifest))

    def test_product_cannot_target_ores_without_approved_exception(self) -> None:
        manifest = copy.deepcopy(self.generated)
        entry = next(
            item
            for item in manifest["spec"]["entries"]
            if item["owner"] == "fiducia-cloud" and item["target"] is not None
        )
        entry["target"] = "ores/dev/fiducia/runtime"
        entry["targetTemplate"] = entry["target"]
        entry["platformTargetException"] = None
        self.assertIn("manifest.product-to-platform", self.diagnostics_for(manifest))

    def test_unclassified_target_is_rejected(self) -> None:
        manifest = copy.deepcopy(self.generated)
        entry = next(
            item
            for item in manifest["spec"]["entries"]
            if item["owner"] == "unclassified"
        )
        entry["target"] = "ores/dev/accidental/default"
        self.assertIn("manifest.unclassified-target", self.diagnostics_for(manifest))

    def test_destructive_cleanup_is_rejected(self) -> None:
        manifest = copy.deepcopy(self.generated)
        manifest["spec"]["entries"][0]["destructiveCleanupAllowed"] = True
        self.assertIn("manifest.destructive-cleanup", self.diagnostics_for(manifest))

    def test_schema_declares_every_entry_field_required(self) -> None:
        schema = json.loads(
            (SOURCE_ROOT / DEFAULT_SCHEMA).read_text(encoding="utf-8")
        )
        entry_schema = schema["$defs"]["entry"]
        self.assertEqual(ENTRY_FIELDS, set(entry_schema["required"]))
        self.assertFalse(entry_schema["additionalProperties"])

    def test_opaque_bundle_chunks_are_absent(self) -> None:
        self.assertEqual(
            [],
            list((SOURCE_ROOT / "catalog/namespaces").glob(".den-2786-bundle-*.hex")),
        )

    def test_manifest_contains_metadata_not_secret_values(self) -> None:
        prohibited_keys = {
            "secretValue",
            "tokenValue",
            "passwordValue",
            "privateKey",
            "accessKeySecret",
        }
        for entry in self.generated["spec"]["entries"]:
            self.assertTrue(prohibited_keys.isdisjoint(entry))


class CliTests(unittest.TestCase):
    def test_check_command_emits_valid_json(self) -> None:
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            status = main(["check", "--root", str(SOURCE_ROOT), "--format", "json"])
        report = json.loads(output.getvalue())
        self.assertEqual(0, status)
        self.assertTrue(report["valid"])
        self.assertEqual(1276, report["entryCount"])

    def test_render_command_is_canonical(self) -> None:
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            status = main(["render", "--root", str(SOURCE_ROOT)])
        self.assertEqual(0, status)
        self.assertEqual(
            (SOURCE_ROOT / DEFAULT_MANIFEST).read_text(encoding="utf-8"),
            output.getvalue(),
        )


if __name__ == "__main__":
    unittest.main(verbosity=2)
