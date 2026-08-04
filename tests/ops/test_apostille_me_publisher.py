#!/usr/bin/env python3
from __future__ import annotations

import json
import os
import pathlib
import subprocess
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[2]
PUBLISHER = ROOT / "scripts" / "ops" / "apostille-me" / "publish.sh"
MANIFEST = PUBLISHER.with_name("manifest.json")
BUNDLE = pathlib.Path(os.environ["APME_BUNDLE_PATH"]).resolve()


class ApostilleMePublisherTests(unittest.TestCase):
    def test_manifest_is_exact_and_bounded(self) -> None:
        manifest = json.loads(MANIFEST.read_text())
        self.assertEqual(manifest["organization"], "apostille-me")
        self.assertEqual(manifest["expected_login"], "ORESoftware")
        self.assertEqual(manifest["visibility"], "public")
        self.assertEqual(manifest["default_branch"], "main")
        self.assertEqual(manifest["feature_branch"], "agent/bootstrap-apostille-me")
        self.assertEqual(len(manifest["repositories"]), 8)

    def test_validate_only_materializes_exact_branch_shape(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            output = pathlib.Path(temp) / "publication.json"
            subprocess.run(
                [str(PUBLISHER), "--bundle", str(BUNDLE), "--output", str(output)],
                cwd=ROOT,
                check=True,
            )
            result = json.loads(output.read_text())
            self.assertEqual(result["status"], "validated")
            self.assertEqual(result["repository_count"], 8)


if __name__ == "__main__":
    unittest.main()
