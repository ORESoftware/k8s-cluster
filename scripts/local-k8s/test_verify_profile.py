#!/usr/bin/env python3
from __future__ import annotations

import importlib.util
import json
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
VERIFIER_PATH = ROOT / "scripts/local-k8s/verify_profile.py"
SPEC = importlib.util.spec_from_file_location("verify_profile", VERIFIER_PATH)
assert SPEC is not None and SPEC.loader is not None
VERIFY = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(VERIFY)

VALID_DIR = ROOT / "contracts/local-k8s-platform/instances/RuntimeProfile/valid"
INVALID_DIR = ROOT / "contracts/local-k8s-platform/instances/RuntimeProfile/invalid"


def read_json(path: Path) -> dict:
    return json.loads(path.read_text(encoding="utf-8"))


class RuntimeProfileTests(unittest.TestCase):
    def test_colima_kind_profile_passes(self) -> None:
        profile = VERIFY.validate_profile(read_json(VALID_DIR / "colima-kind.json"))
        self.assertEqual(profile["runtime"], "colima-kind")
        self.assertEqual(profile["topology"], "three-node")

    def test_ci_docker_kind_profile_passes(self) -> None:
        profile = VERIFY.validate_profile(read_json(VALID_DIR / "ci-kind.json"))
        self.assertEqual(profile["runtime"], "docker-kind")
        self.assertEqual(profile["architecture"], "amd64")
        self.assertEqual(profile["topology"], "single-node")

    def test_multipass_k3s_profile_passes(self) -> None:
        profile = VERIFY.validate_profile(read_json(VALID_DIR / "multipass-k3s.json"))
        self.assertEqual(profile["runtime"], "multipass-k3s")
        self.assertEqual(profile["topology"], "single-node")

    def test_docker_only_profile_is_rejected(self) -> None:
        with self.assertRaisesRegex(VERIFY.ProfileError, "unsupported runtime"):
            VERIFY.validate_profile(read_json(INVALID_DIR / "docker-only.json"))

    def test_cloud_writes_are_rejected(self) -> None:
        profile = read_json(VALID_DIR / "colima-kind.json")
        profile["cloudWrites"] = True
        with self.assertRaisesRegex(VERIFY.ProfileError, "cloudWrites"):
            VERIFY.validate_profile(profile)

    def test_cloud_mutation_policy_is_fail_closed(self) -> None:
        profile = read_json(VALID_DIR / "colima-kind.json")
        profile["cloudMutationPolicy"] = "allowed"
        with self.assertRaisesRegex(VERIFY.ProfileError, "cloudMutationPolicy"):
            VERIFY.validate_profile(profile)

    def test_extra_fields_are_rejected(self) -> None:
        profile = read_json(VALID_DIR / "colima-kind.json")
        profile["providerToken"] = "never-allowed"
        with self.assertRaisesRegex(VERIFY.ProfileError, "profile keys differ"):
            VERIFY.validate_profile(profile)

    def test_bad_dns_name_is_rejected(self) -> None:
        profile = read_json(VALID_DIR / "colima-kind.json")
        profile["name"] = "Mac Kind"
        with self.assertRaisesRegex(VERIFY.ProfileError, "DNS label"):
            VERIFY.validate_profile(profile)

    def test_kind_image_must_be_digest_pinned(self) -> None:
        profile = read_json(VALID_DIR / "colima-kind.json")
        profile["runtimeArtifact"] = "kindest/node:v1.32.8"
        with self.assertRaisesRegex(VERIFY.ProfileError, "digest-pinned"):
            VERIFY.validate_profile(profile)

    def test_kind_image_version_must_match_contract(self) -> None:
        profile = read_json(VALID_DIR / "ci-kind.json")
        profile["kubernetesVersion"] = "v1.32.7"
        with self.assertRaisesRegex(VERIFY.ProfileError, "must equal"):
            VERIFY.validate_profile(profile)

    def test_one_k3s_vm_is_one_cluster_boundary(self) -> None:
        profile = read_json(VALID_DIR / "multipass-k3s.json")
        profile["topology"] = "three-node"
        with self.assertRaisesRegex(VERIFY.ProfileError, "one independent VM cluster"):
            VERIFY.validate_profile(profile)

    def test_k3s_artifact_must_match_version(self) -> None:
        profile = read_json(VALID_DIR / "multipass-k3s.json")
        profile["runtimeArtifact"] = "v1.32.7+k3s1"
        with self.assertRaisesRegex(VERIFY.ProfileError, "must equal"):
            VERIFY.validate_profile(profile)

    def test_bootstrap_scripts_have_no_cloud_provider_mutation_clients_or_secret_inputs(self) -> None:
        shell_dir = ROOT / "scripts/local-k8s"
        forbidden = (
            "aws ",
            "gcloud ",
            "az ",
            "hcloud ",
            "GITHUB_TOKEN",
            "GH_TOKEN",
            "LINEAR_API_KEY",
            "AWS_ACCESS_KEY_ID",
            "AWS_SECRET_ACCESS_KEY",
            "CLOUDFLARE_API_TOKEN",
        )
        scripts = sorted(shell_dir.glob("bootstrap-*.sh"))
        self.assertGreaterEqual(len(scripts), 2)
        for path in scripts:
            content = path.read_text(encoding="utf-8")
            for marker in forbidden:
                self.assertNotIn(marker, content, f"{path}: forbidden marker {marker!r}")


if __name__ == "__main__":
    unittest.main()
