#!/usr/bin/env python3
"""Static fail-closed contract for the exact Meta Agent GitHub App publisher."""

from __future__ import annotations

import argparse
import hashlib
import json
import re
from dataclasses import dataclass
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SCRIPT_PATH = ROOT / "scripts" / "ops" / "publish_meta_agent_control_plane_with_app.sh"

EXPECTED = {
    "SOURCE_SHA": "55ee15c190b7cfa4e075f6984c7cb551acd4b9d3",
    "BUNDLE_SHA256": "1ddaa03743b864348162149b7d2d2e2dce7eab585cf092ea14547c647fcec031",
    "PUBLISHER_SHA256": "e2fe6eaa622db02a54f83e27a822f64ad4b54971c883f97bbda4ac0a4db5d278",
    "EXPECTED_MAIN": "4d6ec3ad0ec7b688f0e777129eee7e0f0d999df1",
    "EXPECTED_FEATURE": "789d48039da232faed985d4f8de176959f117e08",
    "FEATURE_REF": "agent/den-1057-meta-agent-control-plane",
}

REQUIRED_SNIPPETS = {
    "strict-shell": "set -Eeuo pipefail",
    "private-umask": "umask 077",
    "cleanup-trap": "trap cleanup EXIT",
    "error-trap": "trap report_failure ERR",
    "token-cleanup": "unset GH_TOKEN GITHUB_TOKEN GITHUB_REPOSITORY_ADMIN_TOKEN",
    "https-only": "--proto '=https'",
    "tls-floor": "--tlsv1.2",
    "connect-timeout": "--connect-timeout 10",
    "request-timeout": "--max-time 30",
    "private-key-validation": 'openssl pkey -in "$private_key_file" -noout',
    "organization-installation": '.account.login == $org and .account.type == "Organization"',
    "all-repositories-installation": 'test "$repository_selection" = all',
    "administration-write": 'test "$admin_permission" = write',
    "contents-write": 'test "$contents_permission" = write',
    "metadata-read": 'test "$metadata_permission" = read',
    "masked-installation-token": 'echo "::add-mask::${installation_token}"',
    "exact-source-checkout": 'test "$(git -C "$source_root" rev-parse HEAD)" = "$SOURCE_SHA"',
    "clean-source-tree": 'test -z "$(git -C "$source_root" status --porcelain)"',
    "bundle-digest": '"$BUNDLE_SHA256" "$bundle" | sha256sum --check --strict',
    "publisher-digest": '"$PUBLISHER_SHA256" "$publisher" | sha256sum --check --strict',
    "bundle-verification": 'git bundle verify "$bundle"',
    "bundle-ref-set": 'test "$observed_bundle_heads" = "$expected_bundle_heads"',
    "feature-bundle-ref": '"refs/heads/${FEATURE_REF}" "$EXPECTED_FEATURE"',
    "publisher-compile": 'python3 -m py_compile "$publisher"',
    "public-repository": '.visibility == "public"',
    "not-private": '.private == false',
    "not-archived": '.archived == false',
    "not-disabled": '.disabled == false',
    "default-main": '.default_branch == "main"',
    "exact-live-ref-set": 'test "$live_heads" = "$expected_live_heads"',
    "feature-live-ref": '"$EXPECTED_MAIN" "$FEATURE_REF" "$EXPECTED_FEATURE"',
    "idempotent-pr-query": 'gh api --method GET "repos/${FULL_NAME}/pulls"',
    "review-pr-create": 'gh api --method POST "repos/${FULL_NAME}/pulls"',
    "review-pr-base": '.base.ref == "main"',
    "review-pr-head": '.head.ref == $feature',
    "review-pr-output": "META_AGENT_PULL_REQUEST_URL=%s",
}

STAGE_ORDER = [
    "bootstrap",
    "mint-app-jwt",
    "resolve-org-installation",
    "mint-installation-token",
    "verify-installation-token",
    "checkout-exact-publisher-source",
    "reconstruct-exact-bundle",
    "validate-exact-publisher",
    "create-and-push-repository",
    "verify-live-repository",
    "ensure-review-pull-request",
    "complete",
]


@dataclass(frozen=True)
class ContractError(ValueError):
    code: str
    message: str

    def __str__(self) -> str:
        return f"{self.code}: {self.message}"


def require(condition: bool, code: str, message: str) -> None:
    if not condition:
        raise ContractError(code, message)


def extract_constant(text: str, name: str) -> str:
    match = re.search(rf"^readonly {re.escape(name)}='([^']+)'$", text, re.MULTILINE)
    require(match is not None, "missing-constant", f"missing readonly {name}")
    return match.group(1)


def validate_text(text: str) -> dict[str, object]:
    require("\r" not in text, "invalid-newline", "publisher must use LF newlines")
    require(text.startswith("#!/usr/bin/env bash\n"), "invalid-shebang", "publisher must use env bash")

    for name, expected in EXPECTED.items():
        observed = extract_constant(text, name)
        require(observed == expected, "constant-drift", f"{name} drifted")

    for code, snippet in REQUIRED_SNIPPETS.items():
        require(snippet in text, code, f"required publisher guard is missing: {snippet}")

    raw_curl = re.findall(r"(?<![A-Za-z0-9_])curl\s", text)
    require(len(raw_curl) == 1, "unbounded-curl", "all HTTP calls must use github_curl")
    require(
        "--location" not in text and " -L " not in text,
        "redirect-following",
        "publisher must not follow redirects",
    )
    require("set +e" not in text, "fail-open-shell", "publisher must not disable fail-fast behavior")
    require(
        "GITHUB_REPOSITORY_ADMIN_TOKEN='" not in text,
        "literal-token",
        "literal admin tokens are forbidden",
    )
    require(
        "github_pat_" not in text and "ghp_" not in text,
        "literal-token",
        "PAT-shaped values are forbidden",
    )

    positions: list[int] = []
    for stage in STAGE_ORDER:
        needle = "stage=bootstrap" if stage == "bootstrap" else f"stage={stage}"
        position = text.find(needle)
        require(position >= 0, "missing-stage", f"missing stage {stage}")
        positions.append(position)
    require(positions == sorted(positions), "stage-order", "publisher stages are out of order")

    expected_bundle_refs = {
        "refs/heads/main": EXPECTED["EXPECTED_MAIN"],
        f"refs/heads/{EXPECTED['FEATURE_REF']}": EXPECTED["EXPECTED_FEATURE"],
    }
    require(
        "'refs/heads/main' \"$EXPECTED_MAIN\"" in text,
        "ref-contract",
        "missing exact main bundle ref contract",
    )
    require(
        '"refs/heads/${FEATURE_REF}" "$EXPECTED_FEATURE"' in text,
        "ref-contract",
        "missing exact feature bundle ref contract",
    )

    digest = hashlib.sha256(text.encode("utf-8")).hexdigest()
    return {
        "schema_version": 1,
        "script": str(SCRIPT_PATH.relative_to(ROOT)),
        "script_sha256": digest,
        "source_sha": EXPECTED["SOURCE_SHA"],
        "bundle_sha256": EXPECTED["BUNDLE_SHA256"],
        "publisher_sha256": EXPECTED["PUBLISHER_SHA256"],
        "expected_heads": expected_bundle_refs,
        "sealed_constant_count": len(EXPECTED),
        "required_guard_count": len(REQUIRED_SNIPPETS),
        "stage_count": len(STAGE_ORDER),
        "raw_curl_count": len(raw_curl),
        "creates_review_pull_request": True,
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--report", type=Path)
    arguments = parser.parse_args()
    text = SCRIPT_PATH.read_text(encoding="utf-8")
    report = validate_text(text)
    encoded = json.dumps(report, indent=2, sort_keys=True) + "\n"
    if arguments.report:
        arguments.report.write_text(encoded, encoding="utf-8")
    print(encoded, end="")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
