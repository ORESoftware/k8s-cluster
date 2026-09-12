from __future__ import annotations

import unittest
from pathlib import Path

SCRIPT = Path(__file__).with_name("rotate_k8s_libs_deploy_key.sh")


class TransactionalDeployKeyRotationTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.source = SCRIPT.read_text(encoding="utf-8")

    def position(self, needle: str) -> int:
        position = self.source.find(needle)
        self.assertGreaterEqual(position, 0, f"missing contract marker: {needle}")
        return position

    def test_replacement_is_proven_and_installed_before_prior_keys_are_retired(self) -> None:
        snapshot = self.position('prior_key_ids="$work/prior-deploy-key-ids.txt"')
        create = self.position('gh api --method POST "repos/$source_repository/keys"')
        repository_read = self.position('remote_main="$(git ls-remote')
        secret_set = self.position('gh secret set K8S_LIBS_DEPLOY_KEY')
        secret_marked = self.position('secret_updated=true')
        retire = self.position('retired_key_ids="$work/retired-deploy-key-ids.txt"')
        self.assertLess(snapshot, create)
        self.assertLess(create, repository_read)
        self.assertLess(repository_read, secret_set)
        self.assertLess(secret_set, secret_marked)
        self.assertLess(secret_marked, retire)

    def test_failure_cleanup_cannot_delete_a_preexisting_key(self) -> None:
        cleanup_guard = self.position(
            'if [[ "$status" -ne 0 && -n "$deploy_key_id" && "$secret_updated" != true ]]'
        )
        cleanup_delete = self.position(
            'gh api --method DELETE "repos/$source_repository/keys/$deploy_key_id" >/dev/null'
        )
        snapshot = self.position('prior_key_ids="$work/prior-deploy-key-ids.txt"')
        self.assertLess(cleanup_guard, cleanup_delete)
        self.assertLess(cleanup_delete, snapshot)
        self.assertNotIn('keys/$key_id" >/dev/null 2>&1 || true', self.source[:snapshot])

    def test_retirement_is_snapshot_bounded_and_preserves_the_new_key(self) -> None:
        self.assertIn('done < "$prior_key_ids"', self.source)
        self.assertIn('[[ "$key_id" != "$deploy_key_id" ]]', self.source)
        self.assertIn('length == 1 and .[0].id == $id and .[0].read_only == true', self.source)

    def test_receipt_records_transaction_order_without_credentials(self) -> None:
        self.assertIn('schema_version: 2', self.source)
        self.assertIn('actions_secret_updated_before_retirement: true', self.source)
        self.assertIn('rollback_removes_only_uninstalled_new_key: true', self.source)
        self.assertIn('credential_value_recorded: false', self.source)
        self.assertNotIn('credential_value:', self.source)


if __name__ == "__main__":
    unittest.main()
