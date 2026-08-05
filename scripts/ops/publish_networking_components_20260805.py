#!/usr/bin/env python3
"""Create the exact missing networking-components repositories, idempotently."""

from __future__ import annotations

import argparse
import json
import os
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from dataclasses import dataclass
from pathlib import Path
from typing import Any

API = "https://api.github.com"
OWNER = "networking-components"
EXPECTED_LOGIN = "ORESoftware"


@dataclass(frozen=True)
class Repository:
    name: str
    description: str
    topics: tuple[str, ...]


REPOSITORIES = (
    Repository(
        "ncc-dhcp-server",
        "DHCPv4 and DHCPv6 server and relay with leases, reservations, prefix delegation, IPAM integration, and DNS updates.",
        ("networking", "dhcp", "dhcpv6", "rust", "networking-components"),
    ),
    Repository(
        "ncc-ipam",
        "Authoritative IPv4 and IPv6 address, subnet, prefix, reservation, and allocation management.",
        ("networking", "ipam", "ipv4", "ipv6", "rust", "networking-components"),
    ),
    Repository(
        "ncc-firewall",
        "Stateful firewall policy, connection tracking, anti-spoofing, rate limits, and portable enforcement adapters.",
        ("networking", "firewall", "nftables", "ebpf", "rust", "networking-components"),
    ),
    Repository(
        "ncc-forward-proxy",
        "Outbound HTTP CONNECT and SOCKS5 forward proxy with explicit egress policy and observability.",
        ("networking", "forward-proxy", "socks5", "http-connect", "rust", "networking-components"),
    ),
    Repository(
        "ncc-ntp",
        "NTPv4 and Network Time Security service with source selection, holdover, and operating-system clock adapters.",
        ("networking", "ntp", "network-time-security", "time-sync", "rust", "networking-components"),
    ),
    Repository(
        "ncc-stun-turn",
        "STUN, TURN, and ICE connectivity infrastructure for WebRTC and other real-time applications.",
        ("networking", "stun", "turn", "ice", "webrtc", "rust", "networking-components"),
    ),
    Repository(
        "ncc-service-discovery",
        "Health-aware service registry with DNS and versioned HTTP and watch projections.",
        ("networking", "service-discovery", "service-registry", "dns", "rust", "networking-components"),
    ),
    Repository(
        "ncc-network-controller",
        "Declarative versioned network intent, staging, reconciliation, rollback, and cross-component configuration distribution.",
        ("networking", "network-controller", "sdn", "reconciliation", "rust", "networking-components"),
    ),
    Repository(
        "ncc-observability",
        "Flow telemetry, bounded packet diagnostics, synthetic probes, OpenTelemetry adapters, and network SLO evidence.",
        ("networking", "observability", "opentelemetry", "telemetry", "rust", "networking-components"),
    ),
    Repository(
        "ncc-pki",
        "Network-service PKI, ACME, certificate issuance, renewal, revocation, and HSM and KMS integration.",
        ("networking", "pki", "acme", "certificates", "rust", "networking-components"),
    ),
)
EXPECTED_NAMES = tuple(repository.name for repository in REPOSITORIES)


class GitHubError(RuntimeError):
    def __init__(self, method: str, path: str, status: int, message: str):
        super().__init__(f"GitHub {method} {path} returned {status}: {message[:500]}")
        self.status = status


def request(
    token: str,
    method: str,
    path: str,
    payload: Any | None = None,
    allow: tuple[int, ...] = (),
) -> tuple[int, Any | None]:
    body = None if payload is None else json.dumps(payload, separators=(",", ":")).encode()
    headers = {
        "Accept": "application/vnd.github+json",
        "Authorization": f"Bearer {token}",
        "User-Agent": "networking-components-bounded-publisher/1",
        "X-GitHub-Api-Version": "2022-11-28",
    }
    if body is not None:
        headers["Content-Type"] = "application/json"
    for attempt in range(6):
        req = urllib.request.Request(API + path, data=body, headers=headers, method=method)
        try:
            with urllib.request.urlopen(req, timeout=60) as response:
                raw = response.read()
                return response.status, json.loads(raw) if raw else None
        except urllib.error.HTTPError as error:
            raw = error.read(8192)
            try:
                message = json.loads(raw).get("message", "unknown error")
            except Exception:
                message = raw.decode(errors="replace")
            if error.code in allow:
                return error.code, None
            if error.code in (429, 500, 502, 503, 504) and attempt < 5:
                time.sleep(min(2 ** (attempt + 1), 20))
                continue
            raise GitHubError(method, path, error.code, str(message)) from error
        except urllib.error.URLError as error:
            if attempt < 5:
                time.sleep(min(2 ** (attempt + 1), 20))
                continue
            raise RuntimeError(f"GitHub transport failed: {error}") from error
    raise AssertionError("unreachable")


def get(token: str, path: str, allow: tuple[int, ...] = ()) -> tuple[int, Any | None]:
    return request(token, "GET", path, allow=allow)


def post(token: str, path: str, payload: Any) -> tuple[int, Any | None]:
    return request(token, "POST", path, payload)


def patch(token: str, path: str, payload: Any) -> tuple[int, Any | None]:
    return request(token, "PATCH", path, payload)


def put(token: str, path: str, payload: Any) -> tuple[int, Any | None]:
    return request(token, "PUT", path, payload)


def load_request(path: Path) -> dict[str, Any]:
    data = json.loads(path.read_text(encoding="utf-8"))
    if data.get("schema_version") != 1 or data.get("execute") is not True:
        raise RuntimeError("request is not an executable schema-v1 publication request")
    if tuple(data.get("repositories", ())) != EXPECTED_NAMES:
        raise RuntimeError("request repository allowlist does not exactly match the publisher")
    if data.get("repository_count") != len(REPOSITORIES):
        raise RuntimeError("request repository count is invalid")
    return data


def wait_for_main(token: str, name: str) -> str:
    path = f"/repos/{OWNER}/{name}/git/ref/heads/main"
    for _ in range(40):
        status, data = get(token, path, allow=(404, 409))
        if status == 200 and isinstance(data, dict):
            obj = data.get("object")
            sha = obj.get("sha") if isinstance(obj, dict) else None
            if isinstance(sha, str) and len(sha) == 40:
                return sha
        time.sleep(1)
    raise RuntimeError(f"{OWNER}/{name} did not expose an initialized main branch")


def ensure_repository(token: str, repository: Repository) -> str:
    full_name = f"{OWNER}/{repository.name}"
    encoded_name = urllib.parse.quote(repository.name, safe="")
    status, existing = get(token, f"/repos/{OWNER}/{encoded_name}", allow=(404,))
    if status == 404:
        _, created = post(
            token,
            f"/orgs/{OWNER}/repos",
            {
                "name": repository.name,
                "description": repository.description,
                "private": True,
                "auto_init": True,
                "has_issues": True,
                "has_projects": True,
                "has_wiki": False,
            },
        )
        if not isinstance(created, dict) or created.get("full_name") != full_name:
            raise RuntimeError(f"unexpected repository creation response for {full_name}")
        disposition = "CREATED"
    else:
        if not isinstance(existing, dict) or existing.get("full_name") != full_name:
            raise RuntimeError(f"unexpected existing repository response for {full_name}")
        if existing.get("owner", {}).get("login") != OWNER:
            raise RuntimeError(f"repository owner mismatch for {full_name}")
        if existing.get("private") is not True:
            raise RuntimeError(f"refusing to reconcile non-private repository {full_name}")
        disposition = "PRESERVED"

    main_sha = wait_for_main(token, repository.name)
    ref_path = f"/repos/{OWNER}/{encoded_name}/git/ref/heads/dev"
    dev_status, dev_ref = get(token, ref_path, allow=(404, 409))
    if dev_status in (404, 409):
        post(
            token,
            f"/repos/{OWNER}/{encoded_name}/git/refs",
            {"ref": "refs/heads/dev", "sha": main_sha},
        )
    elif not isinstance(dev_ref, dict):
        raise RuntimeError(f"malformed dev ref for {full_name}")

    patch(
        token,
        f"/repos/{OWNER}/{encoded_name}",
        {
            "description": repository.description,
            "private": True,
            "default_branch": "dev",
            "has_issues": True,
            "has_projects": True,
            "has_wiki": False,
            "allow_merge_commit": True,
            "allow_squash_merge": True,
            "allow_rebase_merge": True,
            "allow_auto_merge": True,
            "delete_branch_on_merge": True,
        },
    )
    put(
        token,
        f"/repos/{OWNER}/{encoded_name}/topics",
        {"names": list(repository.topics)},
    )

    _, verified = get(token, f"/repos/{OWNER}/{encoded_name}")
    if not isinstance(verified, dict):
        raise RuntimeError(f"unable to verify {full_name}")
    if verified.get("private") is not True or verified.get("default_branch") != "dev":
        raise RuntimeError(f"repository settings verification failed for {full_name}")
    for branch in ("main", "dev"):
        branch_status, _ = get(
            token,
            f"/repos/{OWNER}/{encoded_name}/git/ref/heads/{branch}",
            allow=(404,),
        )
        if branch_status != 200:
            raise RuntimeError(f"missing {branch} branch for {full_name}")
    print(f"{disposition} {full_name}", flush=True)
    return disposition


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--request", type=Path, required=True)
    args = parser.parse_args()

    token = os.environ.get("GITHUB_REPOSITORY_ADMIN_TOKEN") or os.environ.get("GH_TOKEN")
    if not token or any(character.isspace() for character in token):
        raise RuntimeError("a non-whitespace protected GitHub token is required")
    request_data = load_request(args.request)

    _, profile = get(token, "/user")
    if not isinstance(profile, dict) or profile.get("login") != EXPECTED_LOGIN:
        raise RuntimeError(f"unexpected publisher identity: {profile.get('login') if isinstance(profile, dict) else None!r}")
    _, membership = get(token, f"/user/memberships/orgs/{OWNER}")
    observed = (
        membership.get("role") if isinstance(membership, dict) else None,
        membership.get("state") if isinstance(membership, dict) else None,
    )
    if observed != ("admin", "active"):
        raise RuntimeError(f"{OWNER} owner membership is {observed!r}")

    created = 0
    preserved = 0
    for repository in REPOSITORIES:
        disposition = ensure_repository(token, repository)
        if disposition == "CREATED":
            created += 1
        else:
            preserved += 1

    print(
        "PUBLICATION_COMPLETE "
        f"request_id={request_data['request_id']} created={created} preserved={preserved} total={len(REPOSITORIES)}",
        flush=True,
    )
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except Exception as error:
        print(f"PUBLICATION_FAILED {type(error).__name__}: {error}", file=sys.stderr, flush=True)
        raise
