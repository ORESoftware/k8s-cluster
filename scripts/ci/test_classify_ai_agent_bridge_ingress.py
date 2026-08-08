#!/usr/bin/env python3
from __future__ import annotations

import importlib.util
import sys
import unittest
from pathlib import Path

MODULE_PATH = Path(__file__).with_name("classify_ai_agent_bridge_ingress.py")
SPEC = importlib.util.spec_from_file_location(
    "bridge_ingress_classifier", MODULE_PATH
)
assert SPEC and SPEC.loader
MODULE = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = MODULE
SPEC.loader.exec_module(MODULE)


def evidence(statuses, *, passed=False, tls=True, addresses=None):
    addresses = ["104.21.48.1", "172.67.155.173"] if addresses is None else addresses
    return {
        "host": "api.fiducia.cloud",
        "passed": passed,
        "tls_verified": tls,
        "dns": {"addresses": addresses},
        "observations": [
            {
                "observed_status": status,
                "remote_ip": addresses[0] if addresses else None,
            }
            for status in statuses
        ],
    }


class ClassifierTests(unittest.TestCase):
    def test_522_is_origin_timeout_not_route_drift(self):
        result = MODULE.classify(evidence([522, 522, 522, 522, 522]))
        self.assertEqual(
            result["classification"],
            "cloudflare_origin_connection_timeout",
        )
        self.assertTrue(result["edge_reachable"])
        self.assertEqual(result["origin_reachability"], "failed")
        self.assertFalse(result["credential_material_required"])

    def test_521_is_connection_refused(self):
        result = MODULE.classify(evidence([521, 521]))
        self.assertEqual(
            result["classification"],
            "cloudflare_origin_connection_refused",
        )

    def test_mixed_cloudflare_errors_are_unstable_origin(self):
        result = MODULE.classify(evidence([522, 524]))
        self.assertEqual(
            result["classification"],
            "mixed_cloudflare_origin_failure",
        )
        self.assertEqual(result["origin_reachability"], "failed_or_unstable")

    def test_application_status_drift_remains_route_contract_failure(self):
        result = MODULE.classify(evidence([401, 401, 500]))
        self.assertEqual(result["classification"], "route_contract_failure")

    def test_success_is_healthy(self):
        result = MODULE.classify(
            evidence([401, 401, 401, 405, 404], passed=True)
        )
        self.assertEqual(result["classification"], "healthy")
        self.assertTrue(result["probe_passed"])

    def test_missing_evidence_is_explicit_and_metadata_only(self):
        result = MODULE.missing_evidence_diagnosis(Path("/tmp/missing.json"))
        self.assertEqual(result["classification"], "probe_evidence_missing")
        self.assertFalse(result["response_bodies_recorded"])


if __name__ == "__main__":
    unittest.main()
