#!/usr/bin/env python3
from __future__ import annotations

from pathlib import Path
import re
import unittest


ROOT = Path(__file__).resolve().parents[2]
SCRIPT_PATH = ROOT / "scripts/ops/receive_encrypted_github_credential.sh"
SCRIPT = SCRIPT_PATH.read_text(encoding="utf-8")


class EncryptedCredentialReceiverContractTests(unittest.TestCase):
    def test_uses_rsa_oaep_sha256_and_nonce_specific_paths(self) -> None:
        self.assertIn("rsa_keygen_bits:4096", SCRIPT)
        self.assertIn("rsa_padding_mode:oaep", SCRIPT)
        self.assertIn("rsa_oaep_md:sha256", SCRIPT)
        self.assertIn("rsa_mgf1_md:sha256", SCRIPT)
        self.assertIn("HANDSHAKE_READY", SCRIPT)
        self.assertIn("HANDSHAKE_NONCE", SCRIPT)

    def test_waits_for_both_rest_and_graphql_budget_before_identity_lookup(self) -> None:
        rate_limit = SCRIPT.index('gh api rate_limit')
        ready = SCRIPT.index('RATE_BUDGET_READY')
        identity = SCRIPT.index('gh api user --jq .login')
        self.assertLess(rate_limit, ready)
        self.assertLess(ready, identity)
        self.assertIn("MIN_CORE_REMAINING", SCRIPT)
        self.assertIn("MIN_GRAPHQL_REMAINING", SCRIPT)
        self.assertIn("MAX_RATE_WAIT_SECONDS", SCRIPT)
        self.assertIn("WAIT_RATE_BUDGET", SCRIPT)

    def test_exports_both_required_tokens_without_printing_them(self) -> None:
        self.assertIn("GH_TOKEN=%s", SCRIPT)
        self.assertIn("GITHUB_REPOSITORY_ADMIN_TOKEN=%s", SCRIPT)
        self.assertIn("::add-mask::", SCRIPT)
        self.assertNotIn("echo $USER_TOKEN", SCRIPT)
        self.assertNotIn("printf '%s\\n' \"$USER_TOKEN\"", SCRIPT)
        self.assertIsNone(re.search(r"ghp_[A-Za-z0-9]{20,}", SCRIPT))
        self.assertIsNone(re.search(r"github_pat_[A-Za-z0-9_]{20,}", SCRIPT))

    def test_cleanup_deletes_repository_handoff_and_shreds_local_material(self) -> None:
        self.assertIn('delete_file_if_present "$PUBLIC_KEY_PATH"', SCRIPT)
        self.assertIn('delete_file_if_present "$CIPHERTEXT_PATH"', SCRIPT)
        self.assertIn("shred -u", SCRIPT)
        self.assertIn("trap cleanup_credentials EXIT", SCRIPT)
        self.assertIn("cleanup_credentials\ntrap - EXIT", SCRIPT)

    def test_no_force_or_visibility_mutation_exists(self) -> None:
        self.assertNotIn("git push --force", SCRIPT)
        self.assertNotIn("git push -f", SCRIPT)
        self.assertNotIn("gh repo edit", SCRIPT)
        self.assertNotIn("visibility", SCRIPT)


if __name__ == "__main__":
    unittest.main()
