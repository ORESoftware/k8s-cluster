from __future__ import annotations

import importlib.util
import os
import pathlib
import tempfile
import unittest
from unittest import mock

SCRIPT = (
    pathlib.Path(__file__).resolve().parents[2]
    / "scripts"
    / "ops-create-meta-agent-repo-selfhosted.py"
)
SPEC = importlib.util.spec_from_file_location("canonical_meta_publisher", SCRIPT)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError("publisher module could not be loaded")
PUBLISHER = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(PUBLISHER)


class PublisherSecurityTests(unittest.TestCase):
    def test_url_allowlist_accepts_only_credential_free_github_https(self) -> None:
        PUBLISHER._validate_url("https://api.github.com/user")
        PUBLISHER._validate_url("https://github.com/login/device/code")

        rejected = [
            "http://api.github.com/user",
            "https://evil.example/user",
            "https://github.com.evil.example/user",
            "https://user:password@github.com/login/device/code",
            "https://github.com/login/device/code#fragment",
        ]
        for url in rejected:
            with self.subTest(url=url):
                with self.assertRaisesRegex(RuntimeError, "allowed credential-free"):
                    PUBLISHER._validate_url(url)

    def test_redirect_handler_never_constructs_a_followup_request(self) -> None:
        handler = PUBLISHER.NoRedirect()
        redirected = handler.redirect_request(
            None,
            None,
            302,
            "Found",
            {},
            "https://evil.example/redirected",
        )
        self.assertIsNone(redirected)

    def test_git_environment_is_an_allowlist_not_an_actions_environment_copy(self) -> None:
        inherited = {
            "PATH": "/usr/bin:/bin",
            "LANG": "C.UTF-8",
            "COMMENT_TOKEN": "comment-secret",
            "ACTIONS_RUNTIME_TOKEN": "runtime-secret",
            "GITHUB_TOKEN": "workflow-secret",
            "HTTP_PROXY": "http://proxy.invalid",
            "HTTPS_PROXY": "http://proxy.invalid",
        }
        with mock.patch.dict(os.environ, inherited, clear=True):
            with tempfile.TemporaryDirectory() as directory:
                root = pathlib.Path(directory)
                environment = PUBLISHER.git_environment(
                    "owner-secret",
                    root / "home",
                    root / "askpass.sh",
                )

        self.assertEqual(environment["GITHUB_REPOSITORY_ADMIN_TOKEN"], "owner-secret")
        self.assertEqual(environment["GIT_TERMINAL_PROMPT"], "0")
        self.assertEqual(environment["GIT_CONFIG_NOSYSTEM"], "1")
        self.assertEqual(environment["GIT_CONFIG_GLOBAL"], os.devnull)
        for secret_or_proxy in (
            "COMMENT_TOKEN",
            "ACTIONS_RUNTIME_TOKEN",
            "GITHUB_TOKEN",
            "HTTP_PROXY",
            "HTTPS_PROXY",
        ):
            self.assertNotIn(secret_or_proxy, environment)

    def test_command_diagnostics_keep_only_the_bounded_tail(self) -> None:
        value = "prefix:" + "x" * (PUBLISHER.MAX_COMMAND_DIAGNOSTIC_CHARS + 100)
        diagnostic = PUBLISHER._diagnostic(value)
        self.assertEqual(len(diagnostic), PUBLISHER.MAX_COMMAND_DIAGNOSTIC_CHARS)
        self.assertNotIn("prefix:", diagnostic)
        self.assertTrue(diagnostic.endswith("x" * 100))

    def test_form_and_json_payloads_are_mutually_exclusive_before_network_io(self) -> None:
        with self.assertRaisesRegex(RuntimeError, "both form and JSON"):
            PUBLISHER.request_json(
                "POST",
                "https://api.github.com/user",
                form={"a": "b"},
                payload={"c": "d"},
            )

    def test_missing_bundle_assets_fail_before_repository_mutation(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            with self.assertRaisesRegex(RuntimeError, "bundle parts are missing"):
                PUBLISHER._read_bundle_parts(pathlib.Path(directory))


if __name__ == "__main__":
    unittest.main()
