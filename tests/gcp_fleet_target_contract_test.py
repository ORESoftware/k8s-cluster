#!/usr/bin/env python3
from __future__ import annotations

import json
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SCHEMA_PATH = ROOT / "remote/gcp/fleet-rust-service-target.schema.json"


class GcpFleetTargetContractTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.schema = json.loads(SCHEMA_PATH.read_text(encoding="utf-8"))

    def test_uses_draft_2020_12_and_stable_id(self):
        self.assertEqual(
            "https://json-schema.org/draft/2020-12/schema",
            self.schema["$schema"],
        )
        self.assertEqual(
            "https://schemas.oresoftware.com/platform/gcp-fleet-rust-service-target.v1.schema.json",
            self.schema["$id"],
        )

    def test_runtime_and_repository_family_vocabulary_is_bounded(self):
        spec = self.schema["properties"]["spec"]["properties"]
        self.assertEqual(
            {"cloud-run", "gke"},
            set(spec["runtimes"]["items"]["enum"]),
        )
        self.assertEqual(
            {
                "*-web-server.rs",
                "*-admin-web-server.rs",
                "*-api-server.rs",
                "*-admin-api-server.rs",
            },
            set(spec["serviceDiscovery"]["properties"]["repositoryGlobs"]["items"]["enum"]),
        )

    def test_http_and_shutdown_contract_is_explicit(self):
        spec = self.schema["properties"]["spec"]["properties"]
        http = spec["httpContract"]["properties"]
        self.assertEqual("/healthz", http["liveness"]["const"])
        self.assertEqual("/readyz", http["readiness"]["const"])
        self.assertEqual("/version", http["version"]["const"])
        self.assertEqual("/metrics", http["metrics"]["const"])
        shutdown = spec["shutdown"]["properties"]
        self.assertIs(True, shutdown["readinessBeforeDrain"]["const"])
        self.assertGreaterEqual(shutdown["terminationGraceSeconds"]["minimum"], 10)

    def test_secret_projection_contains_references_not_values(self):
        spec = self.schema["properties"]["spec"]["properties"]
        delivery = spec["secretDelivery"]["properties"]
        self.assertEqual("gcp-secret-manager", delivery["source"]["const"])
        self.assertEqual("external-secrets", delivery["gkeProjection"]["const"])
        self.assertEqual("secret-manager", delivery["cloudRunProjection"]["const"])
        serialized = json.dumps(self.schema, sort_keys=True).lower()
        for forbidden_property in (
            '"password"',
            '"privatekey"',
            '"private_key"',
            '"apikey"',
            '"api_key"',
            '"secretvalue"',
            '"secret_value"',
            '"connectionstring"',
            '"connection_string"',
        ):
            self.assertNotIn(forbidden_property, serialized)

    def test_neon_mapping_is_symbolic_and_env_based(self):
        database = self.schema["properties"]["spec"]["properties"]["database"]["properties"]
        self.assertEqual("neon", database["provider"]["const"])
        self.assertEqual("NEON_DB_CONNECTION_URI", database["connectionEnv"]["const"])
        self.assertEqual(1, database["organization"]["minLength"])
        self.assertEqual(1, database["project"]["minLength"])


if __name__ == "__main__":
    unittest.main()
