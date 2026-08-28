#!/usr/bin/env python3
from __future__ import annotations

import importlib.util
from pathlib import Path
import tempfile
import unittest
import sys

ROOT = Path(__file__).resolve().parents[2]
MODULE_PATH = ROOT / "scripts" / "ops" / "invite_org_member_all.py"
spec = importlib.util.spec_from_file_location("invite_org_member_all", MODULE_PATH)
assert spec and spec.loader
module = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = module
spec.loader.exec_module(module)


class FakeApi:
    def __init__(self, responses):
        self.responses = list(responses)
        self.calls = []

    def request(self, method, path, *, payload=None, allowed_statuses=()):
        self.calls.append((method, path, payload, set(allowed_statuses)))
        response = self.responses.pop(0)
        if isinstance(response, Exception):
            raise response
        return response


def response(status, data):
    return module.ApiResponse(status, data, {})


class InviteOrgMemberTests(unittest.TestCase):
    def test_validate_username(self):
        self.assertEqual(module.validate_username("the1mills"), "the1mills")
        for invalid in ("-bad", "bad-", "bad--name", "bad name", "", "x" * 40):
            with self.subTest(invalid=invalid), self.assertRaises(Exception):
                module.validate_username(invalid)

    def test_discovers_only_active_admin_memberships(self):
        api = FakeApi([
            response(200, [
                {"state": "active", "role": "admin", "organization": {"login": "Owned-A"}},
                {"state": "active", "role": "member", "organization": {"login": "Member-B"}},
                {"state": "pending", "role": "admin", "organization": {"login": "Pending-C"}},
                {"state": "active", "role": "admin", "organization": {"login": "owned-a"}},
            ])
        ])
        self.assertEqual(module.discover_owner_organizations(api), ["Owned-A"])

    def test_active_membership_is_untouched(self):
        api = FakeApi([response(200, {"state": "active", "role": "member"})])
        result = module.reconcile_one(api, "example", "the1mills", execute=True)
        self.assertEqual(result.result, "already_member")
        self.assertEqual([call[0] for call in api.calls], ["GET"])

    def test_pending_invitation_is_preserved(self):
        api = FakeApi([response(200, {"state": "pending", "role": "member"})])
        result = module.reconcile_one(api, "example", "the1mills", execute=True)
        self.assertEqual(result.result, "already_invited")
        self.assertIn("preserved", result.detail)
        self.assertEqual([call[0] for call in api.calls], ["GET"])

    def test_missing_membership_is_invited_as_member(self):
        api = FakeApi([
            response(404, {"message": "Not Found"}),
            response(200, {"state": "pending", "role": "member"}),
        ])
        result = module.reconcile_one(api, "example", "the1mills", execute=True)
        self.assertEqual(result.result, "invited")
        self.assertEqual(api.calls[1][2], {"role": "member"})

    def test_dry_run_never_writes(self):
        api = FakeApi([response(404, {"message": "Not Found"})])
        result = module.reconcile_one(api, "example", "the1mills", execute=False)
        self.assertEqual(result.result, "would_invite")
        self.assertEqual([call[0] for call in api.calls], ["GET"])

    def test_per_org_failure_does_not_expose_token(self):
        failure = module.ApiFailure("GET", "/orgs/example/memberships/the1mills", 403, "bad ghp_abcdefghijklmnopqrstuvwxyz123456")
        api = FakeApi([failure])
        result = module.reconcile_one(api, "example", "the1mills", execute=True)
        self.assertEqual(result.result, "failed")
        self.assertNotIn("abcdefghijklmnopqrstuvwxyz", result.detail)

    def test_build_report_requires_expected_authenticated_account(self):
        api = FakeApi([response(200, {"login": "someone-else"})])
        with self.assertRaisesRegex(RuntimeError, "expected"):
            module.build_report(api, "the1mills", execute=True, expected_authenticated_login="ORESoftware")

    def test_markdown_and_json_reports_are_complete(self):
        report = module.Report(
            generated_at="2026-08-23T00:00:00+00:00",
            mode="execute",
            authenticated_login="ORESoftware",
            target_username="the1mills",
            owner_organizations=1,
            counts={"invited": 1},
            organizations=[module.OrganizationResult("example", "invited", "pending", "member")],
        )
        with tempfile.TemporaryDirectory() as directory:
            json_path = Path(directory) / "report.json"
            markdown_path = Path(directory) / "report.md"
            markdown = module.write_report(report, json_path, markdown_path)
            self.assertIn("org-member-invitation-report-complete", markdown)
            self.assertTrue(json_path.is_file())
            self.assertTrue(markdown_path.is_file())


if __name__ == "__main__":
    unittest.main()
