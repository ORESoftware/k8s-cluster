import importlib.util
import io
import json
import sys
import unittest
import urllib.error
from pathlib import Path
from urllib.parse import urlparse

MODULE_PATH = Path(__file__).resolve().parents[1] / "preflight-msgint-github-app-access.py"
SPEC = importlib.util.spec_from_file_location("msgint_github_preflight", MODULE_PATH)
assert SPEC is not None and SPEC.loader is not None
preflight = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = preflight
SPEC.loader.exec_module(preflight)

REPOSITORY = "messaging-intel/msgint-connectors"
REVISION = "7d905806b2000479bdacb9b206f33b26a707ba5e"
TOKEN = "ghs_test_installation_token_123456789"


class FakeResponse:
    def __init__(self, payload, *, status=200, raw=False):
        self.status = status
        self.body = payload if raw else json.dumps(payload).encode("utf-8")

    def __enter__(self):
        return self

    def __exit__(self, exc_type, exc, traceback):
        return False

    def read(self, limit):
        return self.body[:limit]


class AccessPreflightTests(unittest.TestCase):
    def test_verifies_the_exact_repository_and_commit_without_token_in_urls(self):
        calls = []

        def opener(request, timeout):
            calls.append((request, timeout))
            path = urlparse(request.full_url).path
            if path == f"/repos/{REPOSITORY}":
                return FakeResponse({"full_name": REPOSITORY, "private": True})
            if path == f"/repos/{REPOSITORY}/git/commits/{REVISION}":
                return FakeResponse({"sha": REVISION})
            self.fail(f"unexpected preflight path: {path}")

        preflight.verify_access(REPOSITORY, REVISION, TOKEN, opener=opener)
        self.assertEqual(len(calls), 2)
        for request, timeout in calls:
            self.assertEqual(timeout, 15)
            self.assertEqual(request.headers["Authorization"], f"Bearer {TOKEN}")
            self.assertNotIn(TOKEN, request.full_url)
            self.assertEqual(request.method, "GET")

    def test_rejects_invalid_inputs_before_making_a_request(self):
        calls = []

        def opener(request, timeout):
            calls.append((request, timeout))
            raise AssertionError("opener must not be called")

        cases = [
            ("other/repository", REVISION, TOKEN),
            (REPOSITORY, "main", TOKEN),
            (REPOSITORY, REVISION.upper(), TOKEN),
            (REPOSITORY, REVISION, "short"),
            (REPOSITORY, REVISION, "ghs_token_with whitespace_123456"),
            (REPOSITORY, REVISION, "x" * (preflight.MAX_TOKEN_BYTES + 1)),
        ]
        for repository, revision, token in cases:
            with self.subTest(repository=repository, revision=revision, token_length=len(token)):
                with self.assertRaises(preflight.PreflightError):
                    preflight.verify_access(repository, revision, token, opener=opener)
        self.assertEqual(calls, [])

    def test_http_denial_is_actionable_and_never_echoes_the_token_or_body(self):
        secret_body = b'{"message":"token=' + TOKEN.encode("utf-8") + b'"}'

        def opener(request, timeout):
            raise urllib.error.HTTPError(
                request.full_url,
                404,
                "Not Found",
                {},
                io.BytesIO(secret_body),
            )

        with self.assertRaises(preflight.PreflightError) as raised:
            preflight.verify_access(REPOSITORY, REVISION, TOKEN, opener=opener)
        rendered = str(raised.exception)
        self.assertIn("HTTP 404", rendered)
        self.assertIn("contents read permission", rendered)
        self.assertNotIn(TOKEN, rendered)
        self.assertNotIn("token=", rendered)

    def test_rejects_repository_or_commit_identity_substitution(self):
        def wrong_repository(request, timeout):
            return FakeResponse({"full_name": "attacker/lookalike"})

        with self.assertRaisesRegex(preflight.PreflightError, "repository identity"):
            preflight.verify_access(REPOSITORY, REVISION, TOKEN, opener=wrong_repository)

        def wrong_commit(request, timeout):
            path = urlparse(request.full_url).path
            if path == f"/repos/{REPOSITORY}":
                return FakeResponse({"full_name": REPOSITORY})
            return FakeResponse({"sha": "0" * 40})

        with self.assertRaisesRegex(preflight.PreflightError, "commit identity"):
            preflight.verify_access(REPOSITORY, REVISION, TOKEN, opener=wrong_commit)

    def test_bounds_and_validates_api_responses(self):
        responses = [
            FakeResponse(b"not-json", raw=True),
            FakeResponse([], raw=False),
            FakeResponse(b"x" * (preflight.MAX_RESPONSE_BYTES + 1), raw=True),
            FakeResponse({}, status=201),
        ]
        for response in responses:
            with self.subTest(status=response.status, size=len(response.body)):
                with self.assertRaises(preflight.PreflightError):
                    preflight.verify_access(
                        REPOSITORY,
                        REVISION,
                        TOKEN,
                        opener=lambda request, timeout, response=response: response,
                    )

    def test_redirects_are_not_followed_by_the_default_policy(self):
        handler = preflight.NoRedirectHandler()
        request = handler.redirect_request(None, None, 302, "Found", {}, "https://example.invalid")
        self.assertIsNone(request)


if __name__ == "__main__":
    unittest.main()
