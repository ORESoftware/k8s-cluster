#!/usr/bin/env python3
"""Probe the signed workflow_run fallback and its replay boundary.

This is a synthetic application canary, not evidence that GitHub itself emitted
the delivery or that an organization budget is exhausted. It verifies public
TLS/HMAC admission, invalid-signature rejection, exact repository/SHA/workflow
binding, duplicate-delivery suppression, and terminal fixed-profile execution.
Secrets are read from bounded owner-only files and never printed.
"""

from __future__ import annotations

import argparse
import hashlib
import hmac
import ipaddress
import json
import os
import re
import ssl
import stat
import sys
import time
import urllib.error
import urllib.request
import uuid
from pathlib import Path
from typing import Any
from urllib.parse import urlsplit

SHA_RE = re.compile(r"^[0-9a-f]{40}$")
REPOSITORY_RE = re.compile(r"^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$")
WORKFLOW_RE = re.compile(r"^\.github/workflows/[A-Za-z0-9_.-]+\.ya?ml$")
MAX_RESPONSE_BYTES = 1024 * 1024
MAX_SECRET_BYTES = 4096


class NoRedirect(urllib.request.HTTPRedirectHandler):
    """Treat every redirect as a terminal upstream response."""

    def redirect_request(  # type: ignore[override]
        self,
        req: urllib.request.Request,
        fp: Any,
        code: int,
        msg: str,
        headers: Any,
        newurl: str,
    ) -> None:
        return None


def read_secret(path: Path, name: str) -> bytes:
    flags = os.O_RDONLY | getattr(os, "O_CLOEXEC", 0) | getattr(os, "O_NOFOLLOW", 0)
    try:
        descriptor = os.open(path, flags)
    except OSError as exc:
        raise SystemExit(f"cannot securely open {name} file: {exc}") from exc
    try:
        metadata = os.fstat(descriptor)
        if not stat.S_ISREG(metadata.st_mode):
            raise SystemExit(f"{name} must remain a regular file after open")
        mode = stat.S_IMODE(metadata.st_mode)
        if not mode & stat.S_IRUSR:
            raise SystemExit(f"{name} must be owner-readable")
        if mode & 0o077:
            raise SystemExit(f"{name} must not be group/world accessible")
        with os.fdopen(descriptor, "rb", closefd=False) as handle:
            value = handle.read(MAX_SECRET_BYTES + 3)
    finally:
        os.close(descriptor)

    if len(value) > MAX_SECRET_BYTES + 2:
        raise SystemExit(f"{name} file exceeds the bounded input size")
    if value.endswith(b"\r\n"):
        value = value[:-2]
    elif value.endswith(b"\n"):
        value = value[:-1]
    if not 32 <= len(value) <= MAX_SECRET_BYTES:
        raise SystemExit(f"{name} must contain 32 to {MAX_SECRET_BYTES} bytes")
    if any(byte < 0x21 or byte > 0x7E for byte in value):
        raise SystemExit(f"{name} must be a single visible-ASCII line")
    return value


def validate_origin(raw: str, *, webhook: bool) -> str:
    parsed = urlsplit(raw)
    if not parsed.hostname or parsed.username is not None or parsed.password is not None:
        raise SystemExit("URLs must have a hostname and must not contain credentials")
    if parsed.query or parsed.fragment:
        raise SystemExit("URLs must not contain a query string or fragment")
    if webhook:
        if parsed.scheme != "https" or parsed.path != "/gha-webhooks/github":
            raise SystemExit("--webhook-url must be exact HTTPS /gha-webhooks/github")
        return raw

    if parsed.path not in {"", "/"}:
        raise SystemExit("--status-url must be an origin without a path")
    if parsed.scheme == "https":
        return raw.rstrip("/")
    if parsed.scheme != "http":
        raise SystemExit("--status-url must use HTTPS or loopback HTTP")
    try:
        address = ipaddress.ip_address(parsed.hostname)
    except ValueError:
        if parsed.hostname != "localhost":
            raise SystemExit("plain HTTP status polling is limited to loopback")
    else:
        if not address.is_loopback:
            raise SystemExit("plain HTTP status polling is limited to loopback")
    return raw.rstrip("/")


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
    opener = urllib.request.build_opener(
        NoRedirect(),
        urllib.request.HTTPSHandler(context=ssl.create_default_context()),
    )
    try:
        with opener.open(request, timeout=timeout) as response:
            raw = read_response(response)
            status = response.status
    except urllib.error.HTTPError as exc:
        raw = read_response(exc)
        status = exc.code
    except urllib.error.URLError as exc:
        raise RuntimeError(f"request to {url} failed: {exc.reason}") from exc

    if 300 <= status < 400:
        raise RuntimeError(f"request to {url} returned a redirect, which is forbidden")
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
    parser.add_argument("--sha", required=True, help="full lowercase immutable 40-hex commit SHA")
    parser.add_argument("--workflow-path", required=True, help="allowlisted .github/workflows/*.yml path")
    parser.add_argument("--workflow-name", default="CI", help="workflow display name")
    parser.add_argument(
        "--webhook-url",
        required=True,
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


def post_delivery(
    webhook_url: str,
    body: bytes,
    *,
    delivery: str,
    signature: str,
) -> tuple[int, dict[str, Any]]:
    return request_json(
        "POST",
        webhook_url,
        body=body,
        headers={
            "Content-Type": "application/json",
            "X-GitHub-Event": "workflow_run",
            "X-GitHub-Delivery": delivery,
            "X-Hub-Signature-256": signature,
            "User-Agent": "gha-clone-budget-canary/2",
        },
    )


def main() -> int:
    args = parse_args()
    if not REPOSITORY_RE.fullmatch(args.repository):
        raise SystemExit("--repository must be exact OWNER/REPO")
    if not SHA_RE.fullmatch(args.sha):
        raise SystemExit("--sha must be a full lowercase 40-hex immutable commit SHA")
    if not WORKFLOW_RE.fullmatch(args.workflow_path) or ".." in args.workflow_path:
        raise SystemExit("--workflow-path must be a bounded .github/workflows/*.yml or *.yaml path")
    if args.poll_seconds <= 0 or args.timeout_seconds <= 0:
        raise SystemExit("poll and timeout values must be positive")

    webhook_url = validate_origin(args.webhook_url, webhook=True)
    status_origin = validate_origin(args.status_url, webhook=False)
    webhook_secret = read_secret(args.webhook_secret_file, "webhook secret")
    clone_auth = read_secret(args.clone_auth_secret_file, "clone auth").decode("ascii")

    for endpoint in ("healthz", "readyz"):
        status, response = request_json("GET", f"{status_origin}/{endpoint}")
        if status != 200 or response.get("ok") is not True:
            raise RuntimeError(f"{endpoint} did not report ready: HTTP {status} {response}")

    payload = {
        "action": "completed",
        "repository": {"full_name": args.repository},
        "workflow_run": {
            "name": args.workflow_name,
            "path": args.workflow_path,
            "head_sha": args.sha,
            "conclusion": "action_required",
        },
    }
    body = json.dumps(payload, separators=(",", ":"), sort_keys=True).encode("utf-8")

    invalid_status, _ = post_delivery(
        webhook_url,
        body,
        delivery=str(uuid.uuid4()),
        signature="sha256=" + ("0" * 64),
    )
    if invalid_status != 401:
        raise RuntimeError(f"invalid signature was not rejected with HTTP 401: {invalid_status}")

    delivery = str(uuid.uuid4())
    signature = "sha256=" + hmac.new(webhook_secret, body, hashlib.sha256).hexdigest()
    status, accepted = post_delivery(
        webhook_url,
        body,
        delivery=delivery,
        signature=signature,
    )
    if status != 202 or accepted.get("accepted") is not True:
        raise RuntimeError(f"webhook was not accepted: HTTP {status} {accepted}")
    if accepted.get("event") != "workflow_run" or accepted.get("delivery") != delivery:
        raise RuntimeError(f"webhook response lost event/delivery authority: {accepted}")
    if accepted.get("repository") != args.repository or accepted.get("revision") != args.sha:
        raise RuntimeError(f"webhook response lost repository/SHA authority: {accepted}")
    run_ids = accepted.get("runIds")
    if not isinstance(run_ids, list) or len(run_ids) != 1 or not isinstance(run_ids[0], str):
        raise RuntimeError(f"exact one-workflow canary did not return exactly one run id: {accepted}")

    replay_status, replay = post_delivery(
        webhook_url,
        body,
        delivery=delivery,
        signature=signature,
    )
    if replay_status != 202 or replay.get("accepted") is not False:
        raise RuntimeError(f"duplicate delivery was not suppressed: HTTP {replay_status} {replay}")
    if replay.get("delivery") != delivery or replay.get("repository") != args.repository:
        raise RuntimeError(f"duplicate response lost delivery/repository authority: {replay}")
    if "duplicate" not in str(replay.get("reason", "")).lower() or "runIds" in replay:
        raise RuntimeError(f"duplicate response did not prove zero redispatch: {replay}")

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
            if run.get("repository") != args.repository or run.get("revision") != args.sha:
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
        "synthetic": True,
        "invalidSignatureRejected": True,
        "duplicateDeliverySuppressed": True,
        "delivery": delivery,
        "repository": args.repository,
        "revision": args.sha,
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
