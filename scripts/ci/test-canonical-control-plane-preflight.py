#!/usr/bin/env python3
from __future__ import annotations

import base64
import importlib.util
import json
import unittest
from pathlib import Path

MODULE_PATH = Path(__file__).with_name("canonical-control-plane-preflight.py")
SPEC = importlib.util.spec_from_file_location("canonical_control_plane_preflight", MODULE_PATH)
assert SPEC and SPEC.loader
TARGET = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(TARGET)

WAIT_MODULE_PATH = Path(__file__).with_name("wait-for-encrypted-canonical-bundle.py")
WAIT_SPEC = importlib.util.spec_from_file_location(
    "wait_for_encrypted_canonical_bundle", WAIT_MODULE_PATH
)
assert WAIT_SPEC and WAIT_SPEC.loader
WAIT_TARGET = importlib.util.module_from_spec(WAIT_SPEC)
WAIT_SPEC.loader.exec_module(WAIT_TARGET)

CONTRACT_PATH = Path("config/ci/canonical-control-plane-preflight.json")


class ContractTests(unittest.TestCase):
    def setUp(self) -> None:
        self.contract = json.loads(CONTRACT_PATH.read_text())

    def test_reviewed_contract_is_exact_and_fail_closed(self) -> None:
        TARGET.validate_contract(self.contract)
        self.assertEqual("canonical-cloud", self.contract["source_org"])
        self.assertEqual("canonical-cloud-test", self.contract["test_org"])
        self.assertEqual(TARGET.EXPECTED_TEST_REPOSITORIES, self.contract["test_repositories"])
        self.assertEqual(TARGET.EXPECTED_ROUTES, self.contract["cloudflare"]["routes"])
        self.assertFalse(self.contract["cloudflare"]["r2"]["required"])
        self.assertIsNone(self.contract["cloudflare"]["r2"]["exact_bucket"])

    def test_contract_rejects_test_org_substitution(self) -> None:
        changed = json.loads(json.dumps(self.contract))
        changed["test_org"] = "canonical-cloud"
        with self.assertRaisesRegex(TARGET.PreflightError, "test organization"):
            TARGET.validate_contract(changed)

    def test_contract_rejects_repository_expansion(self) -> None:
        changed = json.loads(json.dumps(self.contract))
        changed["test_repositories"].append("unreviewed-repository")
        with self.assertRaisesRegex(TARGET.PreflightError, "allowlist"):
            TARGET.validate_contract(changed)

    def test_contract_rejects_r2_bucket_guessing(self) -> None:
        changed = json.loads(json.dumps(self.contract))
        changed["cloudflare"]["r2"] = {"required": True, "exact_bucket": "guessed"}
        with self.assertRaisesRegex(TARGET.PreflightError, "R2"):
            TARGET.validate_contract(changed)


class CredentialTests(unittest.TestCase):
    account_id = "62b833940607839add74bd2379cac303"

    def bundle(self) -> dict[str, object]:
        return {
            "github": {"token": "github-token-value"},
            "cloudflare": {
                "account_id": self.account_id,
                "api_token": "cloudflare-token-value",
            },
            "r2": {
                "access_key_id": "r2-access-key-value",
                "secret_access_key": "r2-secret-key-value",
                "endpoint": f"https://{self.account_id}.r2.cloudflarestorage.com",
            },
        }

    def test_bundle_accepts_only_the_reviewed_account_endpoint(self) -> None:
        values = TARGET.validate_bundle(
            self.bundle(),
            "8007ba16f4d4ff2684639b28a390e8516fcf878e80a09ee32279778cf98934c8",
        )
        self.assertEqual(self.account_id, values["cloudflare_account_id"])

    def test_bundle_rejects_cross_account_r2_endpoint(self) -> None:
        bundle = self.bundle()
        bundle["r2"]["endpoint"] = "https://00000000000000000000000000000000.r2.cloudflarestorage.com"
        with self.assertRaisesRegex(TARGET.PreflightError, "R2 endpoint"):
            TARGET.validate_bundle(
                bundle,
                "8007ba16f4d4ff2684639b28a390e8516fcf878e80a09ee32279778cf98934c8",
            )


class CiphertextEnvelopeTests(unittest.TestCase):
    def test_nested_contents_api_base64_decodes_to_exact_rsa_ciphertext(self) -> None:
        ciphertext = bytes((index % 251 for index in range(512)))
        inner = base64.b64encode(ciphertext)
        outer = base64.b64encode(inner).decode()
        self.assertEqual(
            ciphertext,
            WAIT_TARGET.decode_contents_api_ciphertext(outer),
        )

    def test_ciphertext_rejects_wrong_modulus_length(self) -> None:
        inner = base64.b64encode(b"not-a-4096-bit-rsa-ciphertext")
        outer = base64.b64encode(inner).decode()
        with self.assertRaisesRegex(ValueError, "4096-bit RSA ciphertext"):
            WAIT_TARGET.decode_contents_api_ciphertext(outer)


class ClientBoundaryTests(unittest.TestCase):
    def test_cloudflare_client_has_no_write_method(self) -> None:
        for method in ("post", "put", "patch", "delete"):
            self.assertFalse(hasattr(TARGET.CloudflareClient, method), method)

    def test_github_client_rejects_production_repository_writes(self) -> None:
        client = TARGET.GitHubClient(
            "token",
            "canonical-cloud",
            "canonical-cloud-test",
            set(TARGET.EXPECTED_TEST_REPOSITORIES),
        )
        self.assertFalse(
            client._write_allowed(
                "PUT",
                "/repos/canonical-cloud/canonical-infra/contents/wrangler.toml",
            )
        )
        self.assertTrue(
            client._write_allowed(
                "PUT",
                "/repos/canonical-cloud-test/web-server-routing-e2e/contents/.github/workflows/canonical-staging.yml",
            )
        )


class WorkflowTests(unittest.TestCase):
    def setUp(self) -> None:
        self.contract = json.loads(CONTRACT_PATH.read_text())
        self.snapshot = {
            "source_sha": self.contract["source_pins"]["canonical-infra"]["sha"],
            "candidate_branch": "candidate/canonical-infra-535ea8fdd1f7",
        }

    def test_generated_workflows_are_secret_free_and_dispatch_only(self) -> None:
        workflows = [
            TARGET.api_workflow(self.contract),
            TARGET.infra_workflow(self.contract, self.snapshot),
            TARGET.monorepo_workflow(self.contract),
            TARGET.topology_workflow(self.contract),
        ]
        for workflow in workflows:
            self.assertIn(TARGET.HARNESS_MARKER, workflow)
            self.assertIn("workflow_dispatch:", workflow)
            self.assertNotIn("secrets.", workflow)
            self.assertNotIn("permissions:\n  contents: write", workflow)
            self.assertNotIn("wrangler deploy", workflow)
            self.assertNotIn("docker push", workflow)
            self.assertNotIn("kubectl apply", workflow)
            self.assertIn(TARGET.CHECKOUT_SHA, workflow)

    def test_infra_workflow_uses_the_five_route_contract(self) -> None:
        workflow = TARGET.infra_workflow(self.contract, self.snapshot)
        for route in TARGET.EXPECTED_ROUTES:
            self.assertIn(route, workflow)
        self.assertIn("CANONICAL_API_HOST", workflow)
        self.assertIn("r2", workflow.lower())


class ReportTests(unittest.TestCase):
    def test_report_never_claims_write_readiness(self) -> None:
        evidence = {
            "generated_at": "2026-08-08T00:00:00Z",
            "cloudflare": {},
            "github": {},
            "blockers": ["origin health is not proven"],
            "errors": [],
        }
        report = TARGET.markdown_report(evidence)
        self.assertIn("Cloudflare writes: `false`", report)
        self.assertIn("R2 access or writes: `false`", report)
        self.assertIn("origin health is not proven", report)


if __name__ == "__main__":
    unittest.main()
