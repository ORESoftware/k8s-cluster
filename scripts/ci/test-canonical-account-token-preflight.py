#!/usr/bin/env python3
from __future__ import annotations

import importlib.util
import json
import unittest
from pathlib import Path
from unittest import mock

MODULE_PATH = Path(__file__).with_name("canonical-account-token-preflight.py")
SPEC = importlib.util.spec_from_file_location("canonical_account_token_preflight", MODULE_PATH)
assert SPEC and SPEC.loader
TARGET = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(TARGET)
CONTRACT = json.loads(
    Path("config/ci/canonical-control-plane-preflight.json").read_text()
)
ACCOUNT_ID = "62b833940607839add74bd2379cac303"
ZONE_ID = "a" * 32


class FakeAccountClient:
    instances: list["FakeAccountClient"] = []

    def __init__(self, token: str, account_id: str):
        self.token = token
        self.account_id = account_id
        self.zone_id: str | None = None
        self.calls: list[tuple[str, dict[str, str] | None]] = []
        self.__class__.instances.append(self)

    @property
    def account_token_verify_path(self) -> str:
        return f"/accounts/{self.account_id}/tokens/verify"

    def bind_zone(self, zone_id: str) -> None:
        self.zone_id = zone_id

    def get(
        self,
        path: str,
        *,
        query: dict[str, str] | None = None,
        label: str,
        optional_statuses: tuple[int, ...] = (),
    ):
        self.calls.append((path, query))
        if path == self.account_token_verify_path:
            return 200, {"id": "token-id", "status": "active"}
        if path == f"/accounts/{ACCOUNT_ID}":
            return 200, {"id": ACCOUNT_ID, "name": "Canonical"}
        if path == "/zones":
            return 200, [
                {
                    "id": ZONE_ID,
                    "name": "canonical.plus",
                    "status": "active",
                    "type": "full",
                    "account": {"id": ACCOUNT_ID},
                }
            ]
        if path == f"/accounts/{ACCOUNT_ID}/workers/scripts":
            return 200, [
                {
                    "id": "canonical-plus-auth-edge",
                    "created_on": "2026-08-01T00:00:00Z",
                    "modified_on": "2026-08-07T00:00:00Z",
                }
            ]
        if path.endswith("/workers/scripts/canonical-plus-auth-edge/settings"):
            return 200, {"compatibility_date": "2026-08-05"}
        if path == f"/zones/{ZONE_ID}/workers/routes":
            self.assert_optional(403, optional_statuses)
            return 403, None
        if path == f"/zones/{ZONE_ID}/dns_records":
            name = (query or {}).get("name")
            if name == "app.canonical.plus":
                return 200, [
                    {
                        "id": "dns-app",
                        "name": name,
                        "type": "CNAME",
                        "content": "private-origin.example.invalid",
                        "proxied": False,
                        "proxiable": True,
                        "ttl": 1,
                    }
                ]
            if name == "api.canonical.plus":
                return 200, []
        raise AssertionError(f"unexpected GET {path} {query} ({label})")

    @staticmethod
    def assert_optional(status: int, optional_statuses: tuple[int, ...]) -> None:
        if status not in optional_statuses:
            raise AssertionError(f"{status} was not declared optional")


class PartialEvidenceTests(unittest.TestCase):
    def setUp(self) -> None:
        FakeAccountClient.instances.clear()

    def test_route_permission_denial_preserves_account_zone_worker_and_dns(self) -> None:
        values = {
            "cloudflare_account_id": ACCOUNT_ID,
            "cloudflare_api_token": "cfat_test-value",
        }
        with mock.patch.object(
            TARGET,
            "AccountTokenCloudflareClient",
            FakeAccountClient,
        ):
            evidence, blockers = TARGET.cloudflare_inventory(values, CONTRACT)

        self.assertEqual("active", evidence["token"]["status"])
        self.assertEqual("account-owned", evidence["token"]["family"])
        self.assertEqual("canonical.plus", evidence["zone"]["name"])
        self.assertEqual("canonical-plus-auth-edge", evidence["worker"]["script"])
        self.assertTrue(evidence["worker"]["script_inventory_readable"])
        self.assertTrue(evidence["worker"]["exists"])
        self.assertFalse(evidence["routes_readable"])
        self.assertTrue(all(route["exists"] is None for route in evidence["routes"]))

        app, api = evidence["dns"]
        self.assertTrue(app["readable"])
        self.assertTrue(app["exists"])
        self.assertNotIn("content", app["record"])
        self.assertTrue(app["record"]["content_redacted"])
        self.assertTrue(api["readable"])
        self.assertFalse(api["exists"])

        self.assertIn(
            "account token lacks read access to canonical.plus Worker routes",
            blockers,
        )
        self.assertIn("missing exact DNS record: api.canonical.plus", blockers)
        self.assertIn("origin health and TLS are not certified by this inventory", blockers)

        calls = FakeAccountClient.instances[0].calls
        self.assertEqual(
            f"/accounts/{ACCOUNT_ID}/tokens/verify",
            calls[0][0],
        )
        self.assertIn(
            (f"/zones/{ZONE_ID}/dns_records", {"name": "app.canonical.plus", "per_page": "100"}),
            calls,
        )


if __name__ == "__main__":
    unittest.main()
