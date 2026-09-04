from __future__ import annotations

import importlib.util
import json
import sys
import threading
import unittest
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

MODULE_PATH = Path(__file__).with_name("check_cross_repo_exception_pr_state.py")
SPEC = importlib.util.spec_from_file_location("pr_state", MODULE_PATH)
assert SPEC and SPEC.loader
pr_state = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = pr_state
SPEC.loader.exec_module(pr_state)


class PullHandler(BaseHTTPRequestHandler):
    response = {"state": "open", "merged_at": None, "html_url": "https://example.test/pr/1"}
    authorization = None

    def do_GET(self):
        type(self).authorization = self.headers.get("Authorization")
        body = json.dumps(type(self).response).encode()
        self.send_response(200)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, format, *args):
        return


class PrStateTests(unittest.TestCase):
    def setUp(self):
        PullHandler.response = {
            "state": "open",
            "merged_at": None,
            "html_url": "https://example.test/pr/1",
        }
        PullHandler.authorization = None
        self.server = ThreadingHTTPServer(("127.0.0.1", 0), PullHandler)
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()
        self.base = f"http://127.0.0.1:{self.server.server_port}"

    def tearDown(self):
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(timeout=5)

    @staticmethod
    def ledger(with_pr=True):
        row = {
            "workflow": ".github/workflows/test.yml",
            "owning_issue": "DEN-1321",
            "expires_on": "2026-08-05",
            "reason": "test",
        }
        if with_pr:
            row.update({"repository": "ORESoftware/k8s-cluster", "owning_pr": 495})
        return {"feature_ref_exceptions": [row]}

    def test_open_pull_request_passes_and_uses_bearer_auth(self):
        findings = pr_state.check_exception_prs(self.ledger(), self.base, "token-value", 2)
        self.assertEqual(findings, [])
        self.assertEqual(PullHandler.authorization, "Bearer token-value")

    def test_merged_pull_request_fails_cleanup_ratchet(self):
        PullHandler.response = {
            "state": "closed",
            "merged_at": "2026-08-01T12:00:00Z",
            "html_url": "https://example.test/pr/495",
        }
        findings = pr_state.check_exception_prs(self.ledger(), self.base, "token", 2)
        self.assertEqual(len(findings), 1)
        self.assertIn("outlived", findings[0])
        self.assertIn("#495", findings[0])

    def test_closed_unmerged_pull_request_also_fails(self):
        PullHandler.response = {
            "state": "closed",
            "merged_at": None,
            "html_url": "https://example.test/pr/495",
        }
        findings = pr_state.check_exception_prs(self.ledger(), self.base, "token", 2)
        self.assertEqual(len(findings), 1)

    def test_exception_without_pr_metadata_is_ignored(self):
        findings = pr_state.check_exception_prs(
            self.ledger(with_pr=False), self.base, "token", 2
        )
        self.assertEqual(findings, [])
        self.assertIsNone(PullHandler.authorization)

    def test_partial_pr_metadata_is_rejected(self):
        ledger = self.ledger(with_pr=False)
        ledger["feature_ref_exceptions"][0]["owning_pr"] = 495
        with self.assertRaises(pr_state.CheckError):
            pr_state.check_exception_prs(ledger, self.base, "token", 2)


if __name__ == "__main__":
    unittest.main()
