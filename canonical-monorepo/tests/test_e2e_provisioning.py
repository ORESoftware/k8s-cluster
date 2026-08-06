from __future__ import annotations

import copy
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "scripts" / "provision_canonical_e2e.py"
SPEC = importlib.util.spec_from_file_location("provision_canonical_e2e", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


class ProvisioningContractTests(unittest.TestCase):
    def setUp(self) -> None:
        self.manifest = MODULE.load_manifest(
            ROOT / "provisioning" / "canonical-e2e-repositories.json"
        )

    def test_repository_matrix_is_valid(self) -> None:
        self.assertEqual(MODULE.validate_manifest(self.manifest), [])
        self.assertEqual(len(self.manifest["repositories"]), 13)

    def test_duplicate_repository_is_rejected(self) -> None:
        drifted = copy.deepcopy(self.manifest)
        drifted["repositories"].append(copy.deepcopy(drifted["repositories"][0]))
        errors = MODULE.validate_manifest(drifted)
        self.assertTrue(any("duplicate repository" in error for error in errors))

    def test_required_test_repository_cannot_be_removed(self) -> None:
        drifted = copy.deepcopy(self.manifest)
        drifted["repositories"] = [
            entry
            for entry in drifted["repositories"]
            if entry["name"] != "zed-package-graph-e2e"
        ]
        errors = MODULE.validate_manifest(drifted)
        self.assertTrue(any("test repository matrix drift" in error for error in errors))

    def test_initialized_repositories_receive_zed_and_source_contracts(self) -> None:
        orchestrator = next(
            entry
            for entry in self.manifest["repositories"]
            if entry["owner"] == "canonical-cloud"
        )
        files = MODULE.managed_files(orchestrator)
        self.assertIn(".zpkg.toml", files)
        self.assertEqual(files[".zpkg.lock"], "version = 1\n")
        contract = json.loads(files[".canonical-source-contract.json"])
        self.assertEqual(contract["revision_policy"], "immutable-sha-or-digest")
        self.assertFalse(contract["production_source_allowed"])

    def test_staging_mirror_remains_empty(self) -> None:
        mirror = next(
            entry
            for entry in self.manifest["repositories"]
            if entry["name"] == "canonical-api-server.rs"
            and entry["owner"] == "canonical-cloud-test"
        )
        self.assertEqual(MODULE.managed_files(mirror), {})

    def test_zed_identity_matches_repository(self) -> None:
        entry = next(
            entry
            for entry in self.manifest["repositories"]
            if entry["name"] == "clients-rust-consumer"
        )
        rendered = MODULE.render_zpkg(entry)
        self.assertIn('org = "canonical-cloud-test"', rendered)
        self.assertIn('name = "clients-rust-consumer"', rendered)
        self.assertIn('dir = ".vendor/.zed"', rendered)

    def test_apply_rejects_wrong_confirmation_before_network_access(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            manifest_path = Path(directory) / "manifest.json"
            manifest_path.write_text(json.dumps(self.manifest), encoding="utf-8")
            result = MODULE.main(
                [
                    "--manifest",
                    str(manifest_path),
                    "--apply",
                    "--confirm",
                    "wrong",
                ]
            )
        self.assertEqual(result, 1)


if __name__ == "__main__":
    unittest.main()
