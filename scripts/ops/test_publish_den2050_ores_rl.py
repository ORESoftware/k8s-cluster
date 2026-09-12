from __future__ import annotations

import importlib.util
import json
import tempfile
import unittest
import sys
from pathlib import Path

MODULE_PATH = Path(__file__).with_name("publish_den2050_ores_rl.py")
SPEC = importlib.util.spec_from_file_location("publish_den2050_ores_rl", MODULE_PATH)
assert SPEC and SPEC.loader
module = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = module
SPEC.loader.exec_module(module)


class OresRateLimitPublisherTests(unittest.TestCase):
    def test_exact_immutable_identity(self) -> None:
        self.assertEqual(module.REPOSITORY, "ores-rate-limit/ores-rl-lib-core")
        self.assertEqual(module.COMMIT, "cfc81aef5d1de60ff6c46798745a6b3f970bc39d")
        self.assertEqual(module.COORDINATE, "ores-rate-limit/ores-rl-lib-core")
        self.assertEqual(module.VERSION, "0.1.0")
        self.assertEqual(module.TAG, "v0.1.0")

    def test_lightweight_tag_is_accepted_only_at_exact_commit(self) -> None:
        evidence = module.parse_remote_tag(
            f"{module.COMMIT}\trefs/tags/{module.TAG}\n"
        )
        self.assertEqual(evidence.kind, "lightweight")
        self.assertEqual(evidence.peeled_commit, module.COMMIT)

    def test_annotated_tag_is_peeled_before_comparison(self) -> None:
        tag_object = "1" * 40
        evidence = module.parse_remote_tag(
            f"{tag_object}\trefs/tags/{module.TAG}\n"
            f"{module.COMMIT}\trefs/tags/{module.TAG}^{{}}\n"
        )
        self.assertEqual(evidence.kind, "annotated")
        self.assertEqual(evidence.direct_object, tag_object)
        self.assertEqual(evidence.peeled_commit, module.COMMIT)

    def test_divergent_annotated_tag_is_rejected(self) -> None:
        with self.assertRaisesRegex(ValueError, "peels to the wrong commit"):
            module.parse_remote_tag(
                f"{'1' * 40}\trefs/tags/{module.TAG}\n"
                f"{'2' * 40}\trefs/tags/{module.TAG}^{{}}\n"
            )

    def test_missing_tag_is_rejected(self) -> None:
        with self.assertRaisesRegex(ValueError, "required immutable tag is missing"):
            module.parse_remote_tag("")

    def test_manifest_requires_exact_repository_identity(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / ".zpkg.toml"
            path.write_text(
                '''[package]\norg = "ores-rate-limit"\nname = "ores-rl-lib-core"\nversion = "0.1.0"\n\n[package.repository]\nvcs = "git"\nurl = "https://github.com/ores-rate-limit/ores-rl-lib-core"\n\n[publish]\ntag_format = "v{version}"\n''',
                encoding="utf-8",
            )
            module.validate_manifest(path)
            path.write_text(path.read_text().replace("0.1.0", "0.1.1"), encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "unexpected package identity"):
                module.validate_manifest(path)

    def test_registry_metadata_binds_artifact_to_exact_source(self) -> None:
        metadata = {
            "org": module.ORG,
            "name": module.NAME,
            "version": module.VERSION,
            "vcs_tag": module.TAG,
            "vcs_commit": module.COMMIT,
            "sha256": "a" * 64,
            "download_url": "https://registry.zpkg.net/artifacts/example.tar.gz",
            "yanked": False,
        }
        normalized = module.verify_metadata(metadata)
        self.assertEqual(normalized["sha256"], "a" * 64)
        metadata["vcs_commit"] = "b" * 40
        with self.assertRaisesRegex(ValueError, "registry commit diverged"):
            module.verify_metadata(metadata)

    def test_lock_requires_coordinate_version_tag_commit_and_digest(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            lock = Path(temporary) / ".zpkg.lock"
            lock.write_text(
                f'''coordinate = "{module.COORDINATE}"\nversion = "{module.VERSION}"\nvcs_tag = "{module.TAG}"\nvcs_commit = "{module.COMMIT}"\nsha256 = "{'c' * 64}"\n''',
                encoding="utf-8",
            )
            evidence = module.validate_lock(lock)
            self.assertEqual(evidence["artifact_sha256"], "c" * 64)
            lock.write_text(lock.read_text().replace(module.COMMIT, ""), encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "missing immutable package evidence"):
                module.validate_lock(lock)

    def test_evidence_is_json_serializable_without_credentials(self) -> None:
        evidence = {
            "source": {"repository": module.REPOSITORY, "commit": module.COMMIT},
            "package": {"sha256": "d" * 64},
        }
        encoded = json.dumps(evidence)
        self.assertNotIn("zpkg_", encoded)


if __name__ == "__main__":
    unittest.main()
