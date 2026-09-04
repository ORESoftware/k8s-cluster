#!/usr/bin/env python3
"""Wait for one owner-authored, run-scoped encrypted credential comment."""
from __future__ import annotations

import argparse
import base64
import json
import os
import re
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path

EXPECTED_RSA_CIPHERTEXT_BYTES = 512
EXPECTED_COMMENT_AUTHOR = "ORESoftware"


def decode_comment_ciphertext(encoded: str) -> bytes:
    try:
        ciphertext = base64.b64decode(encoded.strip(), validate=True)
    except Exception as error:
        raise ValueError("encrypted bundle comment is not valid base64") from error
    if len(ciphertext) != EXPECTED_RSA_CIPHERTEXT_BYTES:
        raise ValueError(
            "encrypted bundle is not a 4096-bit RSA ciphertext: "
            f"received {len(ciphertext)} bytes"
        )
    return ciphertext


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repository", required=True)
    parser.add_argument("--pull-request", type=int, required=True)
    parser.add_argument("--marker", required=True)
    parser.add_argument("--output", required=True)
    parser.add_argument("--attempts", type=int, default=540)
    parser.add_argument("--interval-seconds", type=float, default=5.0)
    args = parser.parse_args()

    if args.repository != "ORESoftware/k8s-cluster":
        raise SystemExit("encrypted bundle receiver is restricted to ORESoftware/k8s-cluster")
    if args.pull_request <= 0:
        raise SystemExit("pull request number must be positive")
    if not re.fullmatch(r"canonical-[0-9]+-[0-9]+", args.marker):
        raise SystemExit("unexpected encrypted bundle marker")

    token = os.environ.get("GITHUB_TOKEN", "")
    if not token:
        raise SystemExit("GITHUB_TOKEN is required")

    query = urllib.parse.urlencode(
        {"per_page": "100", "sort": "created", "direction": "desc"}
    )
    url = (
        f"https://api.github.com/repos/{args.repository}/issues/"
        f"{args.pull_request}/comments?{query}"
    )
    headers = {
        "Accept": "application/vnd.github+json",
        "Authorization": f"Bearer {token}",
        "X-GitHub-Api-Version": "2022-11-28",
        "User-Agent": "canonical-control-plane-preflight/1",
    }
    prefix = (
        f"<!-- canonical-control-plane-bundle:{args.marker} -->\n"
        "ciphertext_b64:\n"
    )

    for attempt in range(1, args.attempts + 1):
        request = urllib.request.Request(url, method="GET", headers=headers)
        try:
            with urllib.request.urlopen(request, timeout=30) as response:
                comments = json.loads(response.read())
            if not isinstance(comments, list):
                raise SystemExit("encrypted bundle comment lookup did not return a list")
            matches = []
            for comment in comments:
                if not isinstance(comment, dict):
                    continue
                user = comment.get("user")
                body = comment.get("body")
                if (
                    isinstance(user, dict)
                    and user.get("login") == EXPECTED_COMMENT_AUTHOR
                    and isinstance(body, str)
                    and body.startswith(prefix)
                ):
                    matches.append(comment)
            if len(matches) > 1:
                raise SystemExit("multiple encrypted bundle comments match this run")
            if matches:
                comment = matches[0]
                try:
                    ciphertext = decode_comment_ciphertext(
                        str(comment["body"])[len(prefix):]
                    )
                except ValueError as error:
                    raise SystemExit(str(error)) from None
                Path(args.output).write_bytes(ciphertext)
                print(
                    "CANONICAL_ENCRYPTED_BUNDLE_RECEIVED "
                    f"attempt={attempt} comment_id={comment.get('id')}"
                )
                return 0
        except urllib.error.HTTPError as error:
            if error.code not in (404, 422):
                raise SystemExit(
                    f"GitHub encrypted-bundle comment lookup returned HTTP {error.code}"
                ) from None
        except urllib.error.URLError as error:
            if attempt == args.attempts:
                raise SystemExit(
                    "GitHub encrypted-bundle comment lookup did not become reachable"
                ) from error

        if attempt == 1 or attempt % 30 == 0:
            print(f"CANONICAL_ENCRYPTED_BUNDLE_WAITING attempt={attempt}")
        time.sleep(args.interval_seconds)

    print(
        "encrypted Canonical bundle comment was not received before the fail-closed deadline",
        file=sys.stderr,
    )
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
