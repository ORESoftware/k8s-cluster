from __future__ import annotations

import csv
import json
import unittest
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[3]
ACCESS_INVENTORY = REPO_ROOT / "ops/evidence/github-organization-access.json"
PORTFOLIO_REGISTRY = REPO_ROOT / "ops/registries/portfolio-project-links.csv"


class GitHubOrganizationAccessTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.inventory = json.loads(ACCESS_INVENTORY.read_text(encoding="utf-8"))
        cls.organizations = cls.inventory["organizations"]
        with PORTFOLIO_REGISTRY.open(encoding="utf-8", newline="") as handle:
            cls.registry_rows = list(csv.DictReader(handle))

    def test_inventory_covers_the_exact_canonical_registry(self) -> None:
        inventory_keys = [item["portfolio_key"] for item in self.organizations]
        registry_keys = [row["portfolio_key"] for row in self.registry_rows]
        self.assertEqual(inventory_keys, sorted(inventory_keys))
        self.assertEqual(inventory_keys, registry_keys)
        self.assertEqual(len(inventory_keys), 41)
        self.assertEqual(len(set(inventory_keys)), 41)

    def test_native_organization_logins_match_registry_casing(self) -> None:
        registry_logins = {
            row["portfolio_key"]: row["github_org"] for row in self.registry_rows
        }
        for item in self.organizations:
            key = item["portfolio_key"]
            self.assertEqual(item["github_org"], registry_logins[key])
            self.assertEqual(item["github_org"].lower(), key)

    def test_owner_and_installation_ids_are_unique(self) -> None:
        owner_ids = [item["github_owner_id"] for item in self.organizations]
        self.assertTrue(all(isinstance(value, int) and value > 0 for value in owner_ids))
        self.assertEqual(len(owner_ids), len(set(owner_ids)))

        installations = [
            item["github_app_installation_id"]
            for item in self.organizations
            if item["github_app_installation_id"] is not None
        ]
        self.assertEqual(len(installations), 40)
        self.assertTrue(all(isinstance(value, int) and value > 0 for value in installations))
        self.assertEqual(len(installations), len(set(installations)))

    def test_only_dancing_dragons_uses_repository_access(self) -> None:
        exceptions = [
            item
            for item in self.organizations
            if item["access_mode"] != "github_app_installation"
        ]
        self.assertEqual(
            exceptions,
            [
                {
                    "portfolio_key": "dancing-dragons",
                    "github_org": "dancing-dragons",
                    "github_owner_id": 187996352,
                    "github_app_installation_id": None,
                    "access_mode": "repository_access",
                    "verified_repository": "dancing-dragons/dancing-dragons.github.io",
                }
            ],
        )

    def test_declared_summary_matches_inventory(self) -> None:
        self.assertEqual(
            self.inventory["summary"],
            {
                "canonical_organizations": 41,
                "github_app_installations": 40,
                "repository_access_exceptions": 1,
            },
        )
        self.assertEqual(self.inventory["schema_version"], 1)
        self.assertEqual(self.inventory["verified_at"], "2026-08-04")


if __name__ == "__main__":
    unittest.main()
