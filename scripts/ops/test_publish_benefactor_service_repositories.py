#!/usr/bin/env python3
from __future__ import annotations

import importlib.util
from pathlib import Path
import sys
import unittest

MODULE_PATH = Path(__file__).with_name("publish_benefactor_service_repositories.py")
SPEC = importlib.util.spec_from_file_location("benefactor_service_publisher", MODULE_PATH)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError(f"unable to load {MODULE_PATH}")
MODULE = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = MODULE
SPEC.loader.exec_module(MODULE)

SHA_A = "a" * 40
SHA_B = "b" * 40
SHA_C = "c" * 40


class ScriptedApi:
    def __init__(self, steps):
        self.steps = list(steps)
        self.calls = []

    def __call__(self, method, path, body=None):
        self.calls.append((method, path, body))
        if not self.steps:
            raise AssertionError(f"unexpected API call: {method} {path}")
        expected_method, expected_path, status, document = self.steps.pop(0)
        self.assert_equal(method, expected_method)
        self.assert_equal(path, expected_path)
        return status, document

    @staticmethod
    def assert_equal(actual, expected):
        if actual != expected:
            raise AssertionError(f"{actual!r} != {expected!r}")

    def assert_finished(self):
        if self.steps:
            raise AssertionError(f"unconsumed API steps: {self.steps!r}")


def repository(spec, *, private=True, size=1, default_branch="main"):
    return {
        "id": 1001,
        "full_name": spec.full_name,
        "private": private,
        "visibility": "private" if private else "public",
        "default_branch": default_branch,
        "archived": False,
        "disabled": False,
        "has_issues": True,
        "has_wiki": False,
        "size": size,
    }


def ref(sha=SHA_A):
    return {"object": {"sha": sha}}


class PublisherTests(unittest.TestCase):
    def setUp(self):
        self.spec = MODULE.REPOSITORIES[0]
        self.repo_path = f"/repos/{self.spec.full_name}"
        self.ref_path = f"{self.repo_path}/git/ref/heads/main"

    def test_exact_allowlist(self):
        self.assertEqual(
            [spec.full_name for spec in MODULE.REPOSITORIES],
            [
                "benefactor-cc/benefactor-web-server.rs",
                "benefactor-cc/benefactor-api-server.rs",
                "benefactor-cc/benefactor-infra",
            ],
        )
        self.assertTrue(all(MODULE.create_payload(spec)["private"] for spec in MODULE.REPOSITORIES))
        self.assertTrue(all(MODULE.create_payload(spec)["auto_init"] for spec in MODULE.REPOSITORIES))

    def test_existing_private_repository_is_preserved(self):
        existing = repository(self.spec)
        api = ScriptedApi(
            [
                ("GET", self.repo_path, 200, existing),
                ("PATCH", self.repo_path, 200, existing),
                ("GET", self.ref_path, 200, ref()),
                ("GET", self.repo_path, 200, existing),
                ("GET", self.ref_path, 200, ref()),
            ]
        )
        evidence = MODULE.ensure_repository(api, self.spec)
        api.assert_finished()
        self.assertFalse(evidence["created"])
        self.assertEqual(evidence["main_sha"], SHA_A)
        self.assertFalse(any(method == "POST" and path.endswith("/repos") for method, path, _ in api.calls))

    def test_new_repository_uses_private_auto_init(self):
        created = repository(self.spec)
        api = ScriptedApi(
            [
                ("GET", self.repo_path, 404, None),
                ("POST", "/orgs/benefactor-cc/repos", 201, created),
                ("PATCH", self.repo_path, 200, created),
                ("GET", self.ref_path, 200, ref()),
                ("GET", self.repo_path, 200, created),
                ("GET", self.ref_path, 200, ref()),
            ]
        )
        evidence = MODULE.ensure_repository(api, self.spec)
        api.assert_finished()
        self.assertTrue(evidence["created"])
        create_call = next(call for call in api.calls if call[0] == "POST")
        self.assertEqual(create_call[1], "/orgs/benefactor-cc/repos")
        self.assertEqual(create_call[2]["name"], self.spec.name)
        self.assertIs(create_call[2]["private"], True)
        self.assertIs(create_call[2]["auto_init"], True)

    def test_create_race_reconciles_only_exact_private_identity(self):
        reconciled = repository(self.spec)
        api = ScriptedApi(
            [
                ("GET", self.repo_path, 404, None),
                ("POST", "/orgs/benefactor-cc/repos", 422, {"message": "already exists"}),
                ("GET", self.repo_path, 200, reconciled),
                ("PATCH", self.repo_path, 200, reconciled),
                ("GET", self.ref_path, 200, ref()),
                ("GET", self.repo_path, 200, reconciled),
                ("GET", self.ref_path, 200, ref()),
            ]
        )
        evidence = MODULE.ensure_repository(api, self.spec)
        api.assert_finished()
        self.assertFalse(evidence["created"])

    def test_public_repository_is_rejected(self):
        api = ScriptedApi([("GET", self.repo_path, 200, repository(self.spec, private=False))])
        with self.assertRaisesRegex(RuntimeError, "must already be private"):
            MODULE.ensure_repository(api, self.spec)
        api.assert_finished()

    def test_empty_private_repository_gets_one_initial_main_commit(self):
        empty = repository(self.spec, size=0)
        api = ScriptedApi(
            [
                ("GET", self.repo_path, 200, empty),
                ("PATCH", self.repo_path, 200, empty),
                ("GET", self.ref_path, 404, None),
                ("POST", f"{self.repo_path}/git/blobs", 201, {"sha": SHA_A}),
                ("POST", f"{self.repo_path}/git/trees", 201, {"sha": SHA_B}),
                ("POST", f"{self.repo_path}/git/commits", 201, {"sha": SHA_C}),
                ("POST", f"{self.repo_path}/git/refs", 201, {}),
                ("GET", self.ref_path, 200, ref(SHA_C)),
                ("GET", self.repo_path, 200, empty),
                ("GET", self.ref_path, 200, ref(SHA_C)),
            ]
        )
        evidence = MODULE.ensure_repository(api, self.spec)
        api.assert_finished()
        self.assertEqual(evidence["main_sha"], SHA_C)
        ref_call = next(call for call in api.calls if call[1].endswith("/git/refs"))
        self.assertEqual(ref_call[2], {"ref": "refs/heads/main", "sha": SHA_C})

    def test_nonempty_repository_without_main_is_rejected(self):
        nonempty = repository(self.spec, size=5)
        api = ScriptedApi(
            [
                ("GET", self.repo_path, 200, nonempty),
                ("PATCH", self.repo_path, 200, nonempty),
                ("GET", self.ref_path, 404, None),
            ]
        )
        with self.assertRaisesRegex(RuntimeError, "nonempty repository without main"):
            MODULE.ensure_repository(api, self.spec)
        api.assert_finished()


if __name__ == "__main__":
    unittest.main()
