#!/usr/bin/env python3
"""Send one signed action_required workflow_run and verify exact-SHA execution.

The script never prints webhook or clone-server auth secrets. Use a public
--webhook-url to prove ingress and a kubectl port-forward --status-url to poll
private run state.
"""

from __future__ import annotations

import argparse
import hashlib
import hmac
import json
import re
import ssl
import sys
import time
import urllib.error
import urllib.request
import uuid
from pathlib import Path
from typing import Any

SHA_RE = re.compile(r"^[0-9a-fA-F]{40}$")
REPOSITORY_RE = re.compile(r"^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$")
WORKFLOW_RE = re.compile(r"^\.github/workflows/[A-Za-z0-9_.-]+\.ya?ml$")
MAX_RESPONSE_BYTES = 1024 * 1024


def read_secret(path: Path, name: str) -> bytes:
    try:
        value = path.read_bytes().rstrip(b"\r\n")
    except OSError as exc:
        raise SystemExit(f"cannot read {name} file {path}: {exc}") from exc
    if len(value) < 32:
        raise SystemExit(f"{name} must contain at least 32 bytes")
    return value


def read_response(response: Any) -> bytes:
    data = response.read(MAX_RESPONSE_BYTES + 1)
    if len(data) > MAX_RESPONSE_BYTES:
        raise RuntimeError("upstream response exceeded 1 MiB")
    return data


def request_json(
    method: str,
    url: str,
    *,
    body: bytes | None = None,
    headers: dict[str, str] | None = None,
    timeout: float = 15.0,
) -> tuple[int, dict[str, Any]]:
    request = urllib.request.Request(url, data=body, method=method)
    request.add_header("Accept", "application/json")
    for key, value in (headers or {}).items():
        request.add_header(key, value)
    try:
        with urllib.request.urlopen(request, timeout=timeout, context=ssl.create_default_context()) as response:
            raw = read_response(response)
            status = response.status
    except urllib.error.HTTPError as exc:
        raw = read_response(exc)
        status = exc.code
    except urllib.error.URLError as exc:
        raise RuntimeError(f"request to {url} failed: {exc.reason}") from exc

    try:
        decoded = json.loads(raw or b"{}")
    except json.JSONDecodeError as exc:
        preview = raw[:256].decode("utf-8", errors="replace")
        raise RuntimeError(f"HTTP {status} from {url} was not JSON: {preview!r}") from exc
    if not isinstance(decoded, dict):
        raise RuntimeError(f"HTTP {status} from {url} returned a non-object JSON body")
    return status, decoded


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--repository", required=True, help="exact OWNER/REPO")
    parser.add_argument("--sha", required=True, help="full immutable 40-hex commit SHA")
    parser.add_argument("--workflow-path", required=True, help="allowlisted .github/workflows/*.yml path")
    parser.add_argument("--workflow-name", default="CI", help="workflow display name")
    parser.add_argument(
        "--webhook-url",
        default="https://hello.95-217-171-250.sslip.io/gha-webhooks/github",
        help="public or port-forwarded webhook URL",
    )
    parser.add_argument(
        "--status-url",
        default="http://127.0.0.1:18125",
        help="clone-server origin used for health/readiness and run polling",
    )
    parser.add_argument("--webhook-secret-file", required=True, type=Path)
    parser.add_argument("--clone-auth-secret-file", required=True, type=Path)
    parser.add_argument("--poll-seconds", type=float, default=2.0)
    parser.add_argument("--timeout-seconds", type=float, default=1800.0)
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    if not REPOSITORY_RE.fullmatch(args.repository):
        raise SystemExit("--repository must be exact OWNER/REPO")
    if not SHA_RE.fullmatch(args.sha):
        raise SystemExit("--sha must be a full 40-hex immutable commit SHA")
    if not WORKFLOW_RE.fullmatch(args.workflow_path) or ".." in args.workflow_path:
        raise SystemExit("--workflow-path must be a bounded .github/workflows/*.yml or *.yaml path")
    if args.poll_seconds <= 0 or args.timeout_seconds <= 0:
        raise SystemExit("poll and timeout values must be positive")

    webhook_secret = read_secret(args.webhook_secret_file, "webhook secret")
    clone_auth = read_secret(args.clone_auth_secret_file, "clone auth").decode("utf-8")
    status_origin = args.status_url.rstrip("/")

    for endpoint in ("healthz", "readyz"):
        status, response = request_json("GET", f"{status_origin}/{endpoint}")
        if status != 200 or response.get("ok") is not True:
            raise RuntimeError(f"{endpoint} did not report ready: HTTP {status} {response}")

    delivery = str(uuid.uuid4())
    payload = {
        "action": "completed",
        "repository": {"full_name": args.repository},
        "workflow_run": {
            "name": args.workflow_name,
            "path": args.workflow_path,
            "head_sha": args.sha.lower(),
            "conclusion": "action_required",
        },
    }
    body = json.dumps(payload, separators=(",", ":"), sort_keys=True).encode("utf-8")
    signature = "sha256=" + hmac.new(webhook_secret, body, hashlib.sha256).hexdigest()
    status, accepted = request_json(
        "POST",
        args.webhook_url,
        body=body,
        headers={
            "Content-Type": "application/json",
            "X-GitHub-Event": "workflow_run",
            "X-GitHub-Delivery": delivery,
            "X-Hub-Signature-256": signature,
            "User-Agent": "gha-clone-budget-canary/1",
        },
    )
    if status != 202 or accepted.get("accepted") is not True:
        raise RuntimeError(f"webhook was not accepted: HTTP {status} {accepted}")
    if accepted.get("repository") != args.repository or accepted.get("revision") != args.sha.lower():
        raise RuntimeError(f"webhook response lost repository/SHA authority: {accepted}")
    run_ids = accepted.get("runIds")
    if not isinstance(run_ids, list) or not run_ids or not all(isinstance(value, str) for value in run_ids):
        raise RuntimeError(f"webhook response did not include runIds: {accepted}")

    deadline = time.monotonic() + args.timeout_seconds
    terminal: list[dict[str, Any]] = []
    pending = set(run_ids)
    while pending and time.monotonic() < deadline:
        for run_id in list(pending):
            run_status, run = request_json(
                "GET",
                f"{status_origin}/v1/runs/{run_id}",
                headers={"X-Server-Auth": clone_auth},
            )
            if run_status != 200:
                raise RuntimeError(f"run {run_id} status failed: HTTP {run_status} {run}")
            if run.get("repository") != args.repository or run.get("revision") != args.sha.lower():
                raise RuntimeError(f"run {run_id} lost exact repository/SHA binding: {run}")
            if run.get("workflowPath") != args.workflow_path:
                raise RuntimeError(f"run {run_id} lost exact workflow-path binding: {run}")
            state = run.get("status")
            if state in {"succeeded", "failed"}:
                terminal.append(run)
                pending.remove(run_id)
            elif state not in {"queued", "running"}:
                raise RuntimeError(f"run {run_id} returned unexpected state {state!r}")
        if pending:
            time.sleep(args.poll_seconds)

    if pending:
        raise RuntimeError(f"runs did not become terminal before timeout: {sorted(pending)}")
    failures = [run for run in terminal if run.get("status") != "succeeded"]
    evidence = {
        "ok": not failures,
        "delivery": delivery,
        "repository": args.repository,
        "revision": args.sha.lower(),
        "workflowPath": args.workflow_path,
        "runIds": run_ids,
        "statuses": {str(run.get("id")): run.get("status") for run in terminal},
    }
    print(json.dumps(evidence, sort_keys=True))
    return 1 if failures else 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except RuntimeError as exc:
        print(f"canary failed: {exc}", file=sys.stderr)
        raise SystemExit(1) from exc
