#!/usr/bin/env python3
"""Fetch and execute the exact reviewed fallback activator without a temp file."""

from __future__ import annotations

import base64
import hashlib
import json
import re
import subprocess
import sys
import urllib.error
import urllib.parse
import urllib.request


SHA_RE = re.compile(r"^[0-9a-f]{40}$")
DIGEST_RE = re.compile(r"^[0-9a-f]{64}$")
SCRIPT_PATH = "scripts/ops/activate_gha_test_org_fallback.py"
API_URL = "https://api.github.com/repos/ORESoftware/k8s-cluster/contents/" + SCRIPT_PATH


def fail(message: str) -> None:
    raise SystemExit(f"gha-test-fallback bootstrap failed: {message}")


def main() -> int:
    if len(sys.argv) != 4:
        fail("expected trusted SHA, script digest, and callback URL")
    trusted_sha, expected_digest, callback_url = sys.argv[1:]
    if not SHA_RE.fullmatch(trusted_sha):
        fail("trusted SHA is not immutable lowercase hex")
    if not DIGEST_RE.fullmatch(expected_digest):
        fail("script digest is not lowercase SHA-256")

    completed = subprocess.run(
        [
            "kubectl",
            "-n",
            "default",
            "get",
            "secret",
            "dd-agent-secrets",
            "-o",
            "json",
        ],
        check=False,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        encoding="utf-8",
        timeout=60,
    )
    token = ""
    try:
        secret = json.loads(completed.stdout)
        token = base64.b64decode(secret["data"]["GH_PAT"], validate=True).decode("ascii")
    except (KeyError, TypeError, ValueError, UnicodeDecodeError, json.JSONDecodeError):
        token = ""
    if not token:
        secret_result = subprocess.run(
            [
                "aws",
                "secretsmanager",
                "get-secret-value",
                "--region",
                "us-east-1",
                "--secret-id",
                "dd/remote-dev/agent-secrets",
                "--query",
                "SecretString",
                "--output",
                "text",
            ],
            check=False,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            encoding="utf-8",
            timeout=60,
        )
        try:
            token = json.loads(secret_result.stdout).get("GH_PAT", "")
        except (AttributeError, json.JSONDecodeError):
            token = ""
    if len(token) < 20 or any(character.isspace() for character in token):
        fail("protected GH_PAT is unavailable or outside the credential boundary")

    request = urllib.request.Request(
        API_URL + "?ref=" + urllib.parse.quote(trusted_sha),
        headers={
            "Accept": "application/vnd.github+json",
            "Authorization": f"Bearer {token}",
            "X-GitHub-Api-Version": "2022-11-28",
            "User-Agent": "gha-test-fallback-bootstrap/1",
        },
    )
    try:
        with urllib.request.urlopen(request, timeout=30) as response:
            payload = json.load(response)
    except urllib.error.HTTPError as exc:
        fail(f"exact activator fetch returned HTTP {exc.code}")
    except (urllib.error.URLError, TimeoutError, OSError, json.JSONDecodeError):
        fail("exact activator fetch failed")
    try:
        source = base64.b64decode(payload["content"], validate=True)
    except (KeyError, TypeError, ValueError):
        fail("exact activator response had invalid content")
    if hashlib.sha256(source).hexdigest() != expected_digest:
        fail("exact activator digest did not match the workflow checkout")

    token = ""
    source_text = source.decode("utf-8")
    code = compile(source_text, SCRIPT_PATH, "exec")
    sys.argv = [
        SCRIPT_PATH,
        "--callback-url",
        callback_url,
        "--namespace",
        "default",
        "--poll-seconds",
        "3",
        "--timeout-seconds",
        "1800",
        "--reconcile-timeout-seconds",
        "900",
    ]
    namespace = {"__name__": "__main__", "__file__": SCRIPT_PATH}
    exec(code, namespace, namespace)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
