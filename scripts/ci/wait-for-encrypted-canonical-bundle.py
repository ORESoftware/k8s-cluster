#!/usr/bin/env python3
"""Wait for one run-scoped encrypted credential bundle without logging it."""
from __future__ import annotations

import argparse
import base64
import json
import os
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path

EXPECTED_RSA_CIPHERTEXT_BYTES = 512


def decode_contents_api_ciphertext(content: str) -> bytes:
    """Decode GitHub's file envelope and the repository's inner base64 payload."""
    try:
        encoded_ciphertext = base64.b64decode(content, validate=False).strip()
        ciphertext = base64.b64decode(encoded_ciphertext, validate=True)
    except Exception as error:
        raise ValueError("encrypted bundle is not valid nested base64") from error
    if len(ciphertext) != EXPECTED_RSA_CIPHERTEXT_BYTES:
        raise ValueError(
            "encrypted bundle is not a 4096-bit RSA ciphertext: "
            f"received {len(ciphertext)} bytes"
        )
    return ciphertext


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repository", required=True)
    parser.add_argument("--ref", required=True)
    parser.add_argument("--path", required=True)
    parser.add_argument("--output", required=True)
    parser.add_argument("--attempts", type=int, default=540)
    parser.add_argument("--interval-seconds", type=float, default=5.0)
    args = parser.parse_args()

    if args.repository != "ORESoftware/k8s-cluster":
        raise SystemExit("encrypted bundle receiver is restricted to ORESoftware/k8s-cluster")
    if not args.ref.startswith("ci/canonical-test-org-cloudflare-preflight"):
        raise SystemExit("encrypted bundle receiver is restricted to the reviewed Canonical branch")
    expected_prefix = ".github/tmp/canonical-control-plane-"
    if not args.path.startswith(expected_prefix) or not args.path.endswith(".enc.b64"):
        raise SystemExit("unexpected encrypted bundle path")

    token = os.environ.get("GITHUB_TOKEN", "")
    if not token:
        raise SystemExit("GITHUB_TOKEN is required")

    encoded_path = urllib.parse.quote(args.path, safe="/")
    query = urllib.parse.urlencode({"ref": args.ref})
    url = f"https://api.github.com/repos/{args.repository}/contents/{encoded_path}?{query}"
    headers = {
        "Accept": "application/vnd.github+json",
        "Authorization": f"Bearer {token}",
        "X-GitHub-Api-Version": "2022-11-28",
        "User-Agent": "canonical-control-plane-preflight/1",
    }

    for attempt in range(1, args.attempts + 1):
        request = urllib.request.Request(url, method="GET", headers=headers)
        try:
            with urllib.request.urlopen(request, timeout=30) as response:
                payload = json.loads(response.read())
            if not isinstance(payload, dict) or payload.get("type") != "file":
                raise SystemExit("encrypted bundle endpoint did not return a file")
            content = payload.get("content")
            if not isinstance(content, str):
                raise SystemExit("encrypted bundle file has no content")
            try:
                ciphertext = decode_contents_api_ciphertext(content)
            except ValueError as error:
                raise SystemExit(str(error)) from None
            Path(args.output).write_bytes(ciphertext)
            print(f"CANONICAL_ENCRYPTED_BUNDLE_RECEIVED attempt={attempt}")
            return 0
        except urllib.error.HTTPError as error:
            if error.code != 404:
                raise SystemExit(
                    f"GitHub encrypted-bundle lookup returned HTTP {error.code}"
                ) from None
        except urllib.error.URLError as error:
            if attempt == args.attempts:
                raise SystemExit("GitHub encrypted-bundle lookup did not become reachable") from error

        if attempt == 1 or attempt % 30 == 0:
            print(f"CANONICAL_ENCRYPTED_BUNDLE_WAITING attempt={attempt}")
        time.sleep(args.interval_seconds)

    print("encrypted Canonical bundle was not received before the fail-closed deadline", file=sys.stderr)
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
