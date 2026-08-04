#!/usr/bin/env python3
from __future__ import annotations

import importlib.util
import json
import os
import pathlib
import sys
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[2]
PUBLISHER = ROOT / "scripts" / "ops" / "apostille-me" / "publish.py"
MANIFEST = PUBLISHER.with_name("manifest.json")
BUNDLE = pathlib.Path(
    os.environ.get("APME_BUNDLE_PATH", str(PUBLISHER.with_name("apostille-me-combined.bundle")))
).resolve()

spec = importlib.util.spec_from_file_location("apostille_me_publisher", PUBLISHER)
assert spec is not None and spec.loader is not None
module = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = module
spec.loader.exec_module(module)


class ApostilleMePublisherTests(unittest.TestCase):
    def test_manifest_is_exact_and_bounded(self) -> None:
        manifest = module.load_manifest(MANIFEST)
        self.assertEqual(manifest.organization, "apostille-me")
        self.assertEqual(manifest.expected_login, "ORESoftware")
        self.assertEqual(manifest.visibility, "public")
        self.assertEqual(manifest.default_branch, "main")
        self.assertEqual(manifest.feature_branch, "agent/bootstrap-apostille-me")
        self.assertEqual(len(manifest.repositories), 8)
        self.assertEqual(
            {repo.name for repo in manifest.repositories},
            {
                "apme-interfaces",
                "apme-api",
                "apme-web-mash",
                "apme-web-leptos",
                "apme-web-dioxus",
                "apme-cli",
                "apme-sync",
                "apme-infra",
            },
        )

    def test_combined_bundle_matches_sealed_refs(self) -> None:
        manifest = module.load_manifest(MANIFEST)
        module.validate_bundle(manifest, BUNDLE)

    def test_validate_only_materializes_exact_branch_shape(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            output = pathlib.Path(temp) / "publication.json"
            result = module.publish(MANIFEST, BUNDLE, output, execute=False)
            self.assertEqual(result["status"], "validated")
            self.assertEqual(result["repository_count"], 8)
            self.assertEqual(json.loads(output.read_text())["status"], "validated")


if __name__ == "__main__":
    unittest.main()
