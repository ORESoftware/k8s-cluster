from __future__ import annotations

import copy
import json
import sys
import unittest
from pathlib import Path

SCRIPT_DIR = Path(__file__).resolve().parent
REPOSITORY_ROOT = SCRIPT_DIR.parent
sys.path.insert(0, str(SCRIPT_DIR))

from validate_auth_realms import validate_contract  # noqa: E402


class AuthRealmContractTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.contract = json.loads(
            (REPOSITORY_ROOT / "config/auth-realms.contract.json").read_text(
                encoding="utf-8"
            )
        )
        cls.schema = (REPOSITORY_ROOT / "db/schema.sql").read_text(encoding="utf-8")
        cls.realm_source = (REPOSITORY_ROOT / "src/realm.rs").read_text(
            encoding="utf-8"
        )

    def validate(self, contract=None, schema=None, realm_source=None):
        return validate_contract(
            self.contract if contract is None else contract,
            self.schema if schema is None else schema,
            self.realm_source if realm_source is None else realm_source,
        )

    def test_repository_contract_is_valid(self) -> None:
        self.assertEqual(self.validate(), [])

    def test_shared_database_and_signing_boundaries_are_rejected(self) -> None:
        contract = copy.deepcopy(self.contract)
        for field in (
            "databaseEndpointHost",
            "databaseResourceRef",
            "databaseSecretRef",
            "signingKeyRef",
            "sessionCookieName",
            "supabaseProjectRef",
        ):
            contract["profiles"][1][field] = contract["profiles"][0][field]
        errors = self.validate(contract=contract)
        for field in (
            "databaseEndpointHost",
            "databaseResourceRef",
            "databaseSecretRef",
            "signingKeyRef",
            "sessionCookieName",
            "supabaseProjectRef",
        ):
            self.assertIn(f"{field} must be distinct across realms", errors)

    def test_application_database_fallback_is_rejected(self) -> None:
        contract = copy.deepcopy(self.contract)
        contract["applicationDatabaseFallbackAllowed"] = True
        self.assertIn(
            "applicationDatabaseFallbackAllowed must be false",
            self.validate(contract=contract),
        )

    def test_loopback_is_rejected_by_the_production_contract(self) -> None:
        contract = copy.deepcopy(self.contract)
        contract["loopbackAllowedInProduction"] = True
        self.assertIn(
            "loopbackAllowedInProduction must be false",
            self.validate(contract=contract),
        )

    def test_application_enrollment_tables_are_required(self) -> None:
        schema = self.schema.replace(
            "create table if not exists shared_auth.application_accounts",
            "create table if not exists shared_auth.removed_application_accounts",
        )
        self.assertIn("schema is missing application_accounts", self.validate(schema=schema))

    def test_runtime_source_must_reject_application_db_fallback(self) -> None:
        source = self.realm_source.replace(
            "AUTH_APPLICATION_DATABASE_URL is forbidden in shared-auth",
            "fallback accidentally allowed",
        )
        self.assertTrue(
            any(
                "AUTH_APPLICATION_DATABASE_URL is forbidden" in error
                for error in self.validate(realm_source=source)
            )
        )


if __name__ == "__main__":
    unittest.main()
