import json
import re
import unittest
from pathlib import Path
from typing import Any


ROOT = Path(__file__).resolve().parents[2]
MANIFEST_PATH = ROOT / ".github" / "alex-main-agent.json"
LINEAR_ISSUE = re.compile(r"^[A-Z]+-[0-9]+$")


def load_manifest() -> dict[str, Any]:
    with MANIFEST_PATH.open(encoding="utf-8") as manifest_file:
        value = json.load(manifest_file)
    if not isinstance(value, dict):
        raise AssertionError("routing manifest must contain a JSON object")
    return value


class AlexMainAgentRoutingE2E(unittest.TestCase):
    def test_slack_route_reaches_the_expected_linear_project_and_github_repo(self) -> None:
        manifest = load_manifest()

        self.assertEqual(manifest["version"], 1)
        self.assertEqual(
            manifest["slack"],
            {
                "workspace_id": "T01B3C83PMK",
                "app_id": "A0BMBAMM5NJ",
                "channel_id": "C0BLYPGGFH6",
                "channel_name": "cliptown",
            },
        )
        self.assertEqual(manifest["linear"]["team"], "Denman")
        self.assertEqual(manifest["linear"]["project"], "github.com/cliptown")
        self.assertEqual(manifest["github"]["organization"], "cliptown")
        self.assertEqual(manifest["github"]["repository"], "cliptown-monorepo")
        for issue_field in ("routing_issue", "delivery_issue"):
            self.assertRegex(manifest["linear"][issue_field], LINEAR_ISSUE)

    def test_dispatch_guardrails_fail_closed(self) -> None:
        routing = load_manifest()["routing"]
        required_flags = (
            "require_linear_issue",
            "branch_and_pr_include_issue_id",
            "post_pr_ci_review_merge_updates_to_origin_thread",
            "organization_allowlist_only",
            "redact_secrets",
        )

        for flag in required_flags:
            self.assertIs(routing.get(flag), True, f"{flag} must remain enabled")
        self.assertEqual(routing.get("idempotency_source"), "slack_event_id")

    def test_manifest_contains_no_credentials_or_callback_urls(self) -> None:
        forbidden_key_fragments = (
            "api_key",
            "credential",
            "password",
            "private_key",
            "signing_key",
            "token",
            "webhook_url",
        )
        forbidden_value_fragments = (
            "-----begin private key-----",
            "ghp_",
            "github_pat_",
            "sk_live_",
            "xapp-",
            "xoxb-",
            "xoxp-",
        )

        def visit(path: str, value: Any) -> None:
            if isinstance(value, dict):
                for key, child in value.items():
                    normalized_key = key.lower()
                    self.assertFalse(
                        any(fragment in normalized_key for fragment in forbidden_key_fragments),
                        f"secret-bearing key is forbidden at {path}.{key}",
                    )
                    if "secret" in normalized_key:
                        self.assertEqual(
                            (normalized_key, child),
                            ("redact_secrets", True),
                            f"only an enabled redaction policy may name secrets at {path}.{key}",
                        )
                    visit(f"{path}.{key}", child)
            elif isinstance(value, list):
                for index, child in enumerate(value):
                    visit(f"{path}[{index}]", child)
            elif isinstance(value, str):
                normalized = value.lower()
                self.assertFalse(
                    any(fragment in normalized for fragment in forbidden_value_fragments),
                    f"credential-shaped value is forbidden at {path}",
                )
                self.assertFalse(
                    normalized.startswith(("http://", "https://")),
                    f"use stable identifiers instead of callback URLs at {path}",
                )

        visit("$", load_manifest())


if __name__ == "__main__":
    unittest.main()
