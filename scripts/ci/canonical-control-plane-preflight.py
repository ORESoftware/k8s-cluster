#!/usr/bin/env python3
"""Fail-closed Canonical test-org staging and Cloudflare read-only inventory."""
from __future__ import annotations

import argparse
import base64
import datetime as dt
import hashlib
import json
import os
import re
import subprocess
import sys
import tarfile
import tempfile
import time
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path
from typing import Any

GITHUB_API = "https://api.github.com"
CLOUDFLARE_API = "https://api.cloudflare.com/client/v4"
HARNESS_MARKER = "canonical-test-harness-v1"
CHECKOUT_SHA = "3d3c42e5aac5ba805825da76410c181273ba90b1"
SETUP_NODE_SHA = "820762786026740c76f36085b0efc47a31fe5020"
RUST_TOOLCHAIN_SHA = "4be7066ada62dd38de10e7b70166bc74ed198c30"
EXPECTED_TEST_REPOSITORIES = [
    "canonical-api-server.rs",
    "api-server-contract-e2e",
    "monorepo-submodules-e2e",
    "zed-package-graph-e2e",
    "web-server-routing-e2e",
    "cli-install-e2e",
    "clients-rust-consumer",
    "clients-typescript-consumer",
    "clients-go-consumer",
    "clients-python-consumer",
    "mcp-contract-e2e",
    "legacy-mirror-guard-e2e",
]
EXPECTED_ROUTES = [
    "app.canonical.plus/u/*",
    "app.canonical.plus/api/v1/quotes*",
    "app.canonical.plus/ws/quotes*",
    "api.canonical.plus/v1/quotes*",
    "api.canonical.plus/v1/ws*",
]
EXPECTED_DNS_NAMES = ["app.canonical.plus", "api.canonical.plus"]


class PreflightError(RuntimeError):
    pass


def sha256(value: Any) -> str | None:
    if value is None or value == "":
        return None
    return hashlib.sha256(str(value).encode()).hexdigest()


def iso_now() -> str:
    return dt.datetime.now(dt.timezone.utc).replace(microsecond=0).isoformat().replace("+00:00", "Z")


def parse_time(value: str) -> dt.datetime:
    return dt.datetime.fromisoformat(value.replace("Z", "+00:00"))


def safe_error_list(payload: Any) -> str:
    if not isinstance(payload, dict):
        return "no structured error details"
    errors = payload.get("errors")
    if not isinstance(errors, list):
        message = payload.get("message")
        return str(message)[:300] if message else "no structured error details"
    parts: list[str] = []
    for item in errors[:5]:
        if isinstance(item, dict):
            parts.append(f"{item.get('code')}: {str(item.get('message', 'unknown'))[:240]}")
    return "; ".join(parts) or "no structured error details"


class GitHubClient:
    def __init__(self, token: str, source_org: str, test_org: str, allowed_repositories: set[str]):
        self._token = token
        self._source_org = source_org
        self._test_org = test_org
        self._allowed_repositories = allowed_repositories

    def _write_allowed(self, method: str, path: str) -> bool:
        if method == "POST" and path == f"/orgs/{self._test_org}/repos":
            return True
        for repository in self._allowed_repositories:
            prefix = f"/repos/{self._test_org}/{repository}/"
            if not path.startswith(prefix):
                continue
            suffix = path[len(prefix):]
            if method == "PUT" and suffix.startswith("contents/"):
                return True
            if method == "POST" and suffix.startswith("actions/workflows/") and suffix.endswith("/dispatches"):
                return True
        return False

    def _read_allowed(self, path: str) -> bool:
        if path == "/user":
            return True
        if path in {
            f"/orgs/{self._source_org}",
            f"/orgs/{self._test_org}",
            f"/user/memberships/orgs/{self._test_org}",
        }:
            return True
        if path.startswith(f"/repos/{self._source_org}/"):
            return True
        if path.startswith(f"/repos/{self._test_org}/"):
            parts = path.split("/")
            return len(parts) >= 4 and parts[3] in self._allowed_repositories
        return False

    def request(
        self,
        method: str,
        path: str,
        *,
        payload: Any | None = None,
        query: dict[str, str] | None = None,
        expected: tuple[int, ...] = (200,),
        optional_statuses: tuple[int, ...] = (),
        label: str,
    ) -> tuple[int, Any | None]:
        if not path.startswith("/") or ".." in path:
            raise PreflightError(f"unsafe GitHub path for {label}")
        if method == "GET":
            if not self._read_allowed(path):
                raise PreflightError(f"GitHub read escaped the reviewed Canonical scope: {label}")
        elif not self._write_allowed(method, path):
            raise PreflightError(f"GitHub write escaped canonical-cloud-test: {label}")

        url = GITHUB_API + path
        if query:
            url += "?" + urllib.parse.urlencode(query)
        body = None if payload is None else json.dumps(payload).encode()
        headers = {
            "Accept": "application/vnd.github+json",
            "Authorization": f"Bearer {self._token}",
            "X-GitHub-Api-Version": "2022-11-28",
            "User-Agent": "canonical-control-plane-preflight/1",
        }
        if body is not None:
            headers["Content-Type"] = "application/json"

        for attempt in range(5):
            request = urllib.request.Request(url, method=method, data=body, headers=headers)
            try:
                with urllib.request.urlopen(request, timeout=45) as response:
                    raw = response.read()
                    result = json.loads(raw) if raw else None
                    if response.status not in expected:
                        raise PreflightError(f"unexpected GitHub status for {label}")
                    return response.status, result
            except urllib.error.HTTPError as error:
                raw = error.read(32768)
                try:
                    error_payload = json.loads(raw) if raw else {}
                except Exception:
                    error_payload = {}
                if error.code in optional_statuses:
                    return error.code, error_payload
                if error.code in (429, 500, 502, 503, 504) and attempt < 4:
                    time.sleep(min(2 ** (attempt + 1), 20))
                    continue
                raise PreflightError(
                    f"GitHub {label} returned HTTP {error.code}: {safe_error_list(error_payload)}"
                ) from None
            except urllib.error.URLError as error:
                if attempt < 4:
                    time.sleep(min(2 ** (attempt + 1), 20))
                    continue
                raise PreflightError(f"GitHub transport failed for {label}") from error
        raise AssertionError("unreachable")

    def get(self, path: str, *, query: dict[str, str] | None = None, label: str) -> Any:
        return self.request("GET", path, query=query, label=label)[1]

    def get_optional(self, path: str, *, query: dict[str, str] | None = None, label: str) -> Any | None:
        status, payload = self.request(
            "GET", path, query=query, optional_statuses=(404,), label=label
        )
        return None if status == 404 else payload

    def post(self, path: str, payload: Any, *, expected: tuple[int, ...], label: str) -> Any:
        return self.request("POST", path, payload=payload, expected=expected, label=label)[1]

    def put(self, path: str, payload: Any, *, expected: tuple[int, ...], label: str) -> Any:
        return self.request("PUT", path, payload=payload, expected=expected, label=label)[1]


class CloudflareClient:
    """GET-only Cloudflare client. No write method exists by design."""

    def __init__(self, token: str, account_id: str):
        self._token = token
        self._account_id = account_id
        self._zone_id: str | None = None

    def bind_zone(self, zone_id: str) -> None:
        if not re.fullmatch(r"[0-9a-f]{32}", zone_id):
            raise PreflightError("canonical.plus returned an invalid zone identifier")
        self._zone_id = zone_id

    def _allowed(self, path: str) -> bool:
        allowed = {
            "/user/tokens/verify",
            f"/accounts/{self._account_id}",
            f"/accounts/{self._account_id}/workers/scripts",
            f"/accounts/{self._account_id}/workers/scripts/canonical-plus-auth-edge/settings",
            "/zones",
        }
        if path in allowed:
            return True
        if self._zone_id and path in {
            f"/zones/{self._zone_id}/workers/routes",
            f"/zones/{self._zone_id}/dns_records",
        }:
            return True
        return False

    def get(
        self,
        path: str,
        *,
        query: dict[str, str] | None = None,
        label: str,
        optional_statuses: tuple[int, ...] = (),
    ) -> tuple[int, Any | None]:
        if not self._allowed(path):
            raise PreflightError(f"Cloudflare read escaped the reviewed Canonical scope: {label}")
        url = CLOUDFLARE_API + path
        if query:
            url += "?" + urllib.parse.urlencode(query)
        headers = {
            "Accept": "application/json",
            "Authorization": f"Bearer {self._token}",
            "User-Agent": "canonical-control-plane-preflight/1",
        }
        for attempt in range(5):
            request = urllib.request.Request(url, method="GET", headers=headers)
            try:
                with urllib.request.urlopen(request, timeout=45) as response:
                    payload = json.loads(response.read())
                if not isinstance(payload, dict) or payload.get("success") is not True:
                    raise PreflightError(f"Cloudflare {label} returned an unsuccessful result")
                return response.status, payload.get("result")
            except urllib.error.HTTPError as error:
                raw = error.read(32768)
                try:
                    payload = json.loads(raw) if raw else {}
                except Exception:
                    payload = {}
                if error.code in optional_statuses:
                    return error.code, None
                if error.code in (429, 500, 502, 503, 504) and attempt < 4:
                    time.sleep(min(2 ** (attempt + 1), 20))
                    continue
                raise PreflightError(
                    f"Cloudflare {label} returned HTTP {error.code}: {safe_error_list(payload)}"
                ) from None
            except urllib.error.URLError as error:
                if attempt < 4:
                    time.sleep(min(2 ** (attempt + 1), 20))
                    continue
                raise PreflightError(f"Cloudflare transport failed for {label}") from error
        raise AssertionError("unreachable")



def load_json(path: Path) -> dict[str, Any]:
    payload = json.loads(path.read_text())
    if not isinstance(payload, dict):
        raise PreflightError(f"{path.name} must contain a JSON object")
    return payload


def validate_contract(contract: dict[str, Any]) -> None:
    if contract.get("schema_version") != 1 or contract.get("fail_closed") is not True:
        raise PreflightError("unexpected Canonical preflight contract version")
    if contract.get("source_org") != "canonical-cloud":
        raise PreflightError("unexpected source organization")
    if contract.get("test_org") != "canonical-cloud-test":
        raise PreflightError("unexpected test organization")
    if contract.get("test_repositories") != EXPECTED_TEST_REPOSITORIES:
        raise PreflightError("canonical-cloud-test repository allowlist diverged")
    if len(set(contract["test_repositories"])) != len(contract["test_repositories"]):
        raise PreflightError("canonical-cloud-test repository allowlist contains duplicates")

    cloudflare = contract.get("cloudflare")
    if not isinstance(cloudflare, dict):
        raise PreflightError("Cloudflare contract is missing")
    if cloudflare.get("zone_name") != "canonical.plus":
        raise PreflightError("unexpected Cloudflare zone")
    if cloudflare.get("worker_script") != "canonical-plus-auth-edge":
        raise PreflightError("unexpected Cloudflare Worker")
    if cloudflare.get("worker_environment") != "default-production":
        raise PreflightError("unexpected Cloudflare Worker environment")
    if cloudflare.get("routes") != EXPECTED_ROUTES:
        raise PreflightError("Canonical Worker route contract diverged")
    if cloudflare.get("dns_names") != EXPECTED_DNS_NAMES:
        raise PreflightError("Canonical DNS-name contract diverged")
    r2 = cloudflare.get("r2")
    if r2 != {"required": False, "exact_bucket": None}:
        raise PreflightError("R2 must remain outside the reviewed quote architecture")

    expected_hash = contract.get("expected_cloudflare_account_sha256")
    if expected_hash != "8007ba16f4d4ff2684639b28a390e8516fcf878e80a09ee32279778cf98934c8":
        raise PreflightError("reviewed Cloudflare account digest diverged")

    source_pins = contract.get("source_pins")
    if not isinstance(source_pins, dict) or set(source_pins) != {
        "canonical-api-server.rs",
        "canonical-web-server.rs",
        "canonical-monorepo",
        "canonical-e2e",
        "canonical-infra",
    }:
        raise PreflightError("unexpected Canonical source pin set")
    for name, item in source_pins.items():
        if not isinstance(item, dict):
            raise PreflightError(f"invalid source pin for {name}")
        if item.get("repository") != f"canonical-cloud/{name}":
            raise PreflightError(f"source pin escaped canonical-cloud: {name}")
        if not re.fullmatch(r"[0-9a-f]{40}", str(item.get("sha", ""))):
            raise PreflightError(f"source pin is not immutable: {name}")

    harnesses = contract.get("harnesses")
    if not isinstance(harnesses, list) or [item.get("repository") for item in harnesses] != [
        "canonical-api-server.rs",
        "web-server-routing-e2e",
        "monorepo-submodules-e2e",
        "zed-package-graph-e2e",
    ]:
        raise PreflightError("unexpected active Canonical test harness set")
    if contract.get("live_writes") != {
        "canonical_cloud_source_repositories": False,
        "cloudflare": False,
        "dns": False,
        "r2": False,
        "kubernetes": False,
        "database": False,
        "secret_store": False,
        "google_model_configuration": False,
        "canonical_cloud_test_repositories": True,
    }:
        raise PreflightError("live-write contract diverged")


def validate_bundle(bundle: dict[str, Any], expected_account_hash: str) -> dict[str, str]:
    if set(bundle) != {"github", "cloudflare", "r2"}:
        raise PreflightError("credential bundle must contain exactly github, cloudflare, and r2")
    github = bundle.get("github")
    cloudflare = bundle.get("cloudflare")
    r2 = bundle.get("r2")
    if not isinstance(github, dict) or set(github) != {"token"}:
        raise PreflightError("GitHub bundle shape is invalid")
    if not isinstance(cloudflare, dict) or set(cloudflare) != {"account_id", "api_token"}:
        raise PreflightError("Cloudflare bundle shape is invalid")
    if not isinstance(r2, dict) or set(r2) != {"access_key_id", "secret_access_key", "endpoint"}:
        raise PreflightError("R2 bundle shape is invalid")

    values = {
        "github_token": github.get("token"),
        "cloudflare_account_id": cloudflare.get("account_id"),
        "cloudflare_api_token": cloudflare.get("api_token"),
        "r2_access_key_id": r2.get("access_key_id"),
        "r2_secret_access_key": r2.get("secret_access_key"),
        "r2_endpoint": r2.get("endpoint"),
    }
    for key, value in values.items():
        if not isinstance(value, str) or not value or any(character.isspace() for character in value):
            raise PreflightError(f"credential value is invalid: {key}")

    if sha256(values["cloudflare_account_id"]) != expected_account_hash:
        raise PreflightError("Cloudflare account ID does not match the reviewed Canonical account")
    endpoint = urllib.parse.urlparse(values["r2_endpoint"])
    expected_host = f"{values['cloudflare_account_id']}.r2.cloudflarestorage.com"
    if endpoint.scheme != "https" or endpoint.hostname != expected_host or endpoint.path not in ("", "/"):
        raise PreflightError("R2 endpoint does not match the reviewed Cloudflare account")
    return values


def mask_values(values: dict[str, str]) -> None:
    for key in (
        "github_token",
        "cloudflare_api_token",
        "r2_access_key_id",
        "r2_secret_access_key",
    ):
        print(f"::add-mask::{values[key]}")


def run_command(args: list[str], *, cwd: Path | None = None, env: dict[str, str] | None = None) -> str:
    process = subprocess.run(
        args,
        cwd=cwd,
        env=env,
        check=False,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    if process.returncode != 0:
        detail = process.stderr.strip().splitlines()[-1:] or process.stdout.strip().splitlines()[-1:]
        suffix = detail[0][:300] if detail else "no command diagnostics"
        raise PreflightError(f"command failed ({args[0]}): {suffix}")
    return process.stdout.strip()


def cloudflare_inventory(values: dict[str, str], contract: dict[str, Any]) -> tuple[dict[str, Any], list[str]]:
    blockers: list[str] = []
    desired = contract["cloudflare"]
    account_id = values["cloudflare_account_id"]
    client = CloudflareClient(values["cloudflare_api_token"], account_id)

    _, token = client.get("/user/tokens/verify", label="token verification")
    if not isinstance(token, dict) or token.get("status") != "active":
        raise PreflightError("Cloudflare API token is not active")

    _, account = client.get(f"/accounts/{account_id}", label="account verification")
    if not isinstance(account, dict) or account.get("id") != account_id:
        raise PreflightError("Cloudflare token did not resolve to the reviewed account")

    _, zones = client.get(
        "/zones",
        query={"name": "canonical.plus", "account.id": account_id, "per_page": "50"},
        label="canonical.plus zone lookup",
    )
    if not isinstance(zones, list):
        raise PreflightError("Cloudflare zone lookup did not return a list")
    exact_zones = [
        zone
        for zone in zones
        if isinstance(zone, dict)
        and zone.get("name") == "canonical.plus"
        and isinstance(zone.get("account"), dict)
        and zone["account"].get("id") == account_id
    ]
    if len(exact_zones) != 1:
        raise PreflightError(
            f"expected one canonical.plus zone in the reviewed account; found {len(exact_zones)}"
        )
    zone = exact_zones[0]
    zone_id = zone.get("id")
    if not isinstance(zone_id, str):
        raise PreflightError("canonical.plus zone has no identifier")
    client.bind_zone(zone_id)

    _, scripts = client.get(
        f"/accounts/{account_id}/workers/scripts", label="Worker script inventory"
    )
    if not isinstance(scripts, list):
        raise PreflightError("Cloudflare Worker inventory did not return a list")
    matching_scripts = [
        script
        for script in scripts
        if isinstance(script, dict)
        and (script.get("id") == desired["worker_script"] or script.get("name") == desired["worker_script"])
    ]
    if len(matching_scripts) > 1:
        raise PreflightError("the exact Canonical Worker script is ambiguous")
    worker_exists = len(matching_scripts) == 1
    settings_verified = False
    if worker_exists:
        settings_status, _ = client.get(
            f"/accounts/{account_id}/workers/scripts/{desired['worker_script']}/settings",
            label="Worker production settings",
            optional_statuses=(404,),
        )
        settings_verified = settings_status == 200
        if not settings_verified:
            blockers.append("the exact Worker exists but its top-level production settings were not readable")
    else:
        blockers.append("Worker script canonical-plus-auth-edge is not deployed")

    _, routes = client.get(
        f"/zones/{zone_id}/workers/routes", label="canonical.plus Worker routes"
    )
    if not isinstance(routes, list):
        raise PreflightError("Cloudflare route inventory did not return a list")
    route_results: list[dict[str, Any]] = []
    for pattern in desired["routes"]:
        matches = [route for route in routes if isinstance(route, dict) and route.get("pattern") == pattern]
        if len(matches) > 1:
            raise PreflightError(f"multiple exact Worker routes exist for {pattern}")
        owner = matches[0].get("script") if matches else None
        conflict = bool(matches and owner != desired["worker_script"])
        if not matches:
            blockers.append(f"missing exact Worker route: {pattern}")
        elif conflict:
            blockers.append(f"exact Worker route is owned by another script: {pattern}")
        route_results.append(
            {
                "pattern": pattern,
                "exists": bool(matches),
                "script": owner,
                "conflict": conflict,
                "id_sha256": sha256(matches[0].get("id")) if matches else None,
            }
        )
    canonical_prefixes = ("app.canonical.plus/", "api.canonical.plus/")
    unexpected_patterns = sorted(
        route.get("pattern")
        for route in routes
        if isinstance(route, dict)
        and isinstance(route.get("pattern"), str)
        and route["pattern"].startswith(canonical_prefixes)
        and route["pattern"] not in desired["routes"]
    )
    if unexpected_patterns:
        blockers.append("unexpected canonical.plus Worker routes exist")

    dns_results: list[dict[str, Any]] = []
    for name in desired["dns_names"]:
        _, records = client.get(
            f"/zones/{zone_id}/dns_records",
            query={"name": name, "per_page": "100"},
            label=f"DNS lookup for {name}",
        )
        if not isinstance(records, list):
            raise PreflightError(f"DNS lookup for {name} did not return a list")
        exact = [record for record in records if isinstance(record, dict) and record.get("name") == name]
        if len(exact) > 1:
            raise PreflightError(f"multiple exact DNS records exist for {name}")
        if not exact:
            blockers.append(f"missing exact DNS record: {name}")
            dns_results.append({"name": name, "exists": False, "record": None})
            continue
        record = exact[0]
        content = str(record.get("content", ""))
        dns_results.append(
            {
                "name": name,
                "exists": True,
                "record": {
                    "id_sha256": sha256(record.get("id")),
                    "type": record.get("type"),
                    "proxied": record.get("proxied"),
                    "proxiable": record.get("proxiable"),
                    "ttl": record.get("ttl"),
                    "content_sha256": sha256(content),
                    "content_redacted": bool(content),
                },
            }
        )

    blockers.extend(
        [
            "the exact Kubernetes gateway, load balancer, or tunnel origin is not proven by this inventory",
            "origin health and TLS are not certified by this inventory",
        ]
    )
    script = matching_scripts[0] if worker_exists else {}
    return (
        {
            "token": {
                "status": token.get("status"),
                "id_sha256": sha256(token.get("id")),
                "expires_on": token.get("expires_on"),
            },
            "account": {
                "id_sha256": sha256(account.get("id")),
                "name": account.get("name"),
                "type": account.get("type"),
            },
            "zone": {
                "id_sha256": sha256(zone_id),
                "name": zone.get("name"),
                "status": zone.get("status"),
                "type": zone.get("type"),
                "account_id_sha256": sha256((zone.get("account") or {}).get("id")),
            },
            "worker": {
                "script": desired["worker_script"],
                "environment": desired["worker_environment"],
                "exists": worker_exists,
                "settings_verified": settings_verified,
                "created_on": script.get("created_on"),
                "modified_on": script.get("modified_on"),
            },
            "routes": route_results,
            "unexpected_canonical_route_patterns": unexpected_patterns,
            "dns": dns_results,
            "write_performed": False,
            "ready_for_write": False,
        },
        blockers,
    )


def verify_github_scope(
    client: GitHubClient, contract: dict[str, Any]
) -> tuple[dict[str, Any], list[str]]:
    blockers: list[str] = []
    user = client.get("/user", label="authenticated user")
    if not isinstance(user, dict) or not user.get("login"):
        raise PreflightError("GitHub PAT did not resolve to an authenticated user")
    source_org = client.get("/orgs/canonical-cloud", label="canonical-cloud organization")
    if not isinstance(source_org, dict) or source_org.get("login") != "canonical-cloud":
        raise PreflightError("canonical-cloud source organization did not resolve exactly")
    test_org = client.get("/orgs/canonical-cloud-test", label="canonical-cloud-test organization")
    if not isinstance(test_org, dict) or test_org.get("login") != "canonical-cloud-test":
        raise PreflightError("canonical-cloud-test organization did not resolve exactly")
    membership = client.get(
        "/user/memberships/orgs/canonical-cloud-test", label="canonical-cloud-test membership"
    )
    if not isinstance(membership, dict):
        raise PreflightError("canonical-cloud-test membership did not resolve")
    if membership.get("state") != "active" or membership.get("role") != "admin":
        raise PreflightError("GitHub PAT owner is not an active canonical-cloud-test administrator")

    verified_pins: dict[str, dict[str, Any]] = {}
    for name, item in contract["source_pins"].items():
        repository = item["repository"]
        commit = client.get(
            f"/repos/{repository}/commits/{item['sha']}", label=f"source commit {name}"
        )
        if not isinstance(commit, dict) or commit.get("sha") != item["sha"]:
            raise PreflightError(f"source commit did not resolve exactly: {name}")
        verified_pins[name] = {
            "repository": repository,
            "sha": item["sha"],
            "tree_sha256": sha256(((commit.get("commit") or {}).get("tree") or {}).get("sha")),
        }

    infra = client.get("/repos/canonical-cloud/canonical-infra", label="private canonical-infra access")
    permissions = infra.get("permissions") if isinstance(infra, dict) else None
    if not isinstance(permissions, dict) or permissions.get("pull") is not True:
        raise PreflightError("GitHub PAT cannot read the private canonical-infra source")

    return (
        {
            "actor_login_sha256": sha256(user.get("login")),
            "source_org": source_org.get("login"),
            "test_org": test_org.get("login"),
            "test_org_id_sha256": sha256(test_org.get("id")),
            "membership": {"state": membership.get("state"), "role": membership.get("role")},
            "source_pins": verified_pins,
        },
        blockers,
    )


def validate_test_repository(repository: dict[str, Any], expected_name: str) -> None:
    owner = repository.get("owner") if isinstance(repository, dict) else None
    if not isinstance(owner, dict) or owner.get("login") != "canonical-cloud-test":
        raise PreflightError(f"test repository owner mismatch: {expected_name}")
    if repository.get("name") != expected_name:
        raise PreflightError(f"test repository name mismatch: {expected_name}")
    if repository.get("private") is not True:
        raise PreflightError(f"test repository must remain private: {expected_name}")
    if repository.get("default_branch") != "main":
        raise PreflightError(f"test repository default branch must be main: {expected_name}")


def provision_test_repositories(
    client: GitHubClient, contract: dict[str, Any]
) -> list[dict[str, Any]]:
    results: list[dict[str, Any]] = []
    for name in contract["test_repositories"]:
        existing = client.get_optional(
            f"/repos/canonical-cloud-test/{name}", label=f"test repository {name}"
        )
        created = False
        repository = existing
        if repository is None:
            repository = client.post(
                "/orgs/canonical-cloud-test/repos",
                {
                    "name": name,
                    "description": f"[{HARNESS_MARKER}] isolated Canonical exact-head staging",
                    "private": True,
                    "has_issues": True,
                    "has_projects": False,
                    "has_wiki": False,
                    "has_downloads": False,
                    "auto_init": True,
                },
                expected=(201,),
                label=f"create test repository {name}",
            )
            created = True
        if not isinstance(repository, dict):
            raise PreflightError(f"test repository did not resolve: {name}")
        validate_test_repository(repository, name)
        results.append(
            {
                "name": name,
                "created": created,
                "id_sha256": sha256(repository.get("id")),
                "default_branch": repository.get("default_branch"),
                "private": repository.get("private"),
            }
        )
    return results


def git_basic_header(token: str) -> str:
    value = base64.b64encode(f"x-access-token:{token}".encode()).decode()
    print(f"::add-mask::{value}")
    return value


def stage_private_infra_snapshot(
    values: dict[str, str], contract: dict[str, Any]
) -> dict[str, Any]:
    pin = contract["source_pins"]["canonical-infra"]
    source_sha = pin["sha"]
    runner_temp = Path(os.environ.get("RUNNER_TEMP") or tempfile.gettempdir())
    source_dir = runner_temp / f"canonical-infra-source-{source_sha[:12]}"
    snapshot_dir = runner_temp / f"canonical-infra-snapshot-{source_sha[:12]}"
    archive_path = runner_temp / f"canonical-infra-{source_sha[:12]}.tar"
    source_dir.mkdir(parents=True, exist_ok=False)
    snapshot_dir.mkdir(parents=True, exist_ok=False)

    basic = git_basic_header(values["github_token"])
    header = f"http.https://github.com/.extraheader=AUTHORIZATION: basic {basic}"
    run_command(["git", "init", "--quiet", str(source_dir)])
    run_command(
        ["git", "-C", str(source_dir), "remote", "add", "source", "https://github.com/canonical-cloud/canonical-infra.git"]
    )
    run_command(
        [
            "git",
            "-C",
            str(source_dir),
            "-c",
            header,
            "fetch",
            "--quiet",
            "--depth=1",
            "source",
            source_sha,
        ]
    )
    if run_command(["git", "-C", str(source_dir), "rev-parse", "FETCH_HEAD"]) != source_sha:
        raise PreflightError("private canonical-infra fetch did not resolve the pinned commit")
    source_tree = run_command(["git", "-C", str(source_dir), "rev-parse", "FETCH_HEAD^{tree}"])
    root_paths = [
        item
        for item in run_command(
            ["git", "-C", str(source_dir), "ls-tree", "--name-only", "FETCH_HEAD"]
        ).splitlines()
        if item and item != ".github"
    ]
    if not root_paths or ".github" in root_paths:
        raise PreflightError("private source snapshot path filter failed closed")
    run_command(
        [
            "git",
            "-C",
            str(source_dir),
            "archive",
            "--format=tar",
            "--output",
            str(archive_path),
            "FETCH_HEAD",
            "--",
            *root_paths,
        ]
    )
    with tarfile.open(archive_path, "r") as archive:
        for member in archive.getmembers():
            member_path = Path(member.name)
            if member_path.is_absolute() or ".." in member_path.parts or member_path.parts[:1] == (".github",):
                raise PreflightError("private source archive contains an unsafe path")
        archive.extractall(snapshot_dir, filter="data")

    provenance = {
        "schema_version": 1,
        "source_repository": pin["repository"],
        "source_sha": source_sha,
        "source_tree_sha": source_tree,
        "omitted_paths": [".github"],
        "purpose": "isolated canonical-cloud-test exact-head execution",
    }
    (snapshot_dir / ".canonical-source.json").write_text(
        json.dumps(provenance, indent=2, sort_keys=True) + "\n"
    )

    commit_date = run_command(
        ["git", "-C", str(source_dir), "show", "-s", "--format=%cI", "FETCH_HEAD"]
    )
    run_command(["git", "init", "--quiet", str(snapshot_dir)])
    run_command(["git", "-C", str(snapshot_dir), "config", "user.name", "Canonical Test Harness"])
    run_command(
        ["git", "-C", str(snapshot_dir), "config", "user.email", "canonical-test@users.noreply.github.com"]
    )
    run_command(["git", "-C", str(snapshot_dir), "add", "-A"])
    commit_env = os.environ.copy()
    commit_env.update(
        {
            "GIT_AUTHOR_DATE": commit_date,
            "GIT_COMMITTER_DATE": commit_date,
        }
    )
    run_command(
        ["git", "-C", str(snapshot_dir), "commit", "--quiet", "-m", f"stage canonical-infra {source_sha}"],
        env=commit_env,
    )
    snapshot_sha = run_command(["git", "-C", str(snapshot_dir), "rev-parse", "HEAD"])
    candidate_branch = f"candidate/canonical-infra-{source_sha[:12]}"
    run_command(
        [
            "git",
            "-C",
            str(snapshot_dir),
            "remote",
            "add",
            "test",
            "https://github.com/canonical-cloud-test/web-server-routing-e2e.git",
        ]
    )
    run_command(
        [
            "git",
            "-C",
            str(snapshot_dir),
            "-c",
            header,
            "push",
            "--quiet",
            "test",
            f"HEAD:refs/heads/{candidate_branch}",
        ]
    )
    return {
        "source_repository": pin["repository"],
        "source_sha": source_sha,
        "source_tree_sha256": sha256(source_tree),
        "candidate_repository": "canonical-cloud-test/web-server-routing-e2e",
        "candidate_branch": candidate_branch,
        "candidate_commit_sha": snapshot_sha,
        "workflows_omitted": True,
    }


def api_workflow(contract: dict[str, Any]) -> str:
    pin = contract["source_pins"]["canonical-api-server.rs"]
    return f'''# {HARNESS_MARKER}
name: Canonical API exact-head staging
run-name: Canonical API {pin["sha"]} ${{{{ inputs.nonce }}}}

on:
  workflow_dispatch:
    inputs:
      nonce:
        required: true
        type: string

permissions:
  contents: read

jobs:
  test:
    if: github.repository == 'canonical-cloud-test/canonical-api-server.rs'
    runs-on: ubuntu-24.04
    timeout-minutes: 45
    steps:
      - name: Check out exact Canonical API source
        uses: actions/checkout@{CHECKOUT_SHA}
        with:
          repository: {pin["repository"]}
          ref: {pin["sha"]}
          path: source
          persist-credentials: false
          fetch-depth: 1
      - name: Install stable Rust
        uses: dtolnay/rust-toolchain@{RUST_TOOLCHAIN_SHA}
        with:
          toolchain: stable
          components: rustfmt,clippy
      - name: Run locked API contracts
        working-directory: source
        run: |
          set -euo pipefail
          test "$(git rev-parse HEAD)" = '{pin["sha"]}'
          cargo fmt --all --check
          cargo clippy --all-targets --locked -- -D warnings
          cargo test --all-targets --locked
'''


def infra_workflow(contract: dict[str, Any], snapshot: dict[str, Any]) -> str:
    expected_routes = json.dumps(EXPECTED_ROUTES)
    return f'''# {HARNESS_MARKER}
name: Canonical edge split-host staging
run-name: Canonical edge {snapshot["source_sha"]} ${{{{ inputs.nonce }}}}

on:
  workflow_dispatch:
    inputs:
      nonce:
        required: true
        type: string

permissions:
  contents: read

jobs:
  test:
    if: github.repository == 'canonical-cloud-test/web-server-routing-e2e'
    runs-on: ubuntu-24.04
    timeout-minutes: 20
    steps:
      - name: Check out sanitized exact-head infrastructure snapshot
        uses: actions/checkout@{CHECKOUT_SHA}
        with:
          ref: {snapshot["candidate_branch"]}
          path: source
          persist-credentials: false
          fetch-depth: 1
      - name: Install Node.js 22
        uses: actions/setup-node@{SETUP_NODE_SHA}
        with:
          node-version: '22'
      - name: Run split-host Worker contracts
        working-directory: source
        run: |
          set -euo pipefail
          python3 - <<'PY'
          import json
          from pathlib import Path
          provenance = json.loads(Path('.canonical-source.json').read_text())
          assert provenance['source_sha'] == '{snapshot["source_sha"]}'
          assert provenance['omitted_paths'] == ['.github']
          PY
          npm run check
          python3 - <<'PY'
          import tomllib
          from pathlib import Path
          config = tomllib.loads(Path('wrangler.toml').read_text())
          assert config['name'] == 'canonical-plus-auth-edge'
          assert config['main'] == 'vendor/shared-auth-edge/src/canonical-plus.mjs'
          assert [route['pattern'] for route in config['routes']] == {expected_routes}
          assert config['vars']['CANONICAL_API_HOST'] == 'api.canonical.plus'
          assert config['vars']['PROTECTED_PATH_PREFIXES'] == '/u/,/api/v1/quotes,/ws/quotes,/v1/quotes,/v1/ws'
          assert 'env' not in config
          assert not any(key.lower().startswith('r2') for key in config)
          PY
'''


def monorepo_workflow(contract: dict[str, Any]) -> str:
    pin = contract["source_pins"]["canonical-monorepo"]
    return f'''# {HARNESS_MARKER}
name: Canonical monorepo exact-head staging
run-name: Canonical monorepo {pin["sha"]} ${{{{ inputs.nonce }}}}

on:
  workflow_dispatch:
    inputs:
      nonce:
        required: true
        type: string

permissions:
  contents: read

jobs:
  test:
    if: github.repository == 'canonical-cloud-test/monorepo-submodules-e2e'
    runs-on: ubuntu-24.04
    timeout-minutes: 35
    steps:
      - name: Use HTTPS for public Canonical submodules
        run: git config --global url."https://github.com/".insteadOf "git@github.com:"
      - name: Check out exact Canonical monorepo source
        uses: actions/checkout@{CHECKOUT_SHA}
        with:
          repository: {pin["repository"]}
          ref: {pin["sha"]}
          path: source
          persist-credentials: false
          fetch-depth: 0
          submodules: recursive
      - name: Install Node.js 22
        uses: actions/setup-node@{SETUP_NODE_SHA}
        with:
          node-version: '22'
      - name: Run superproject and gitlink contracts
        working-directory: source
        run: |
          set -euo pipefail
          test "$(git rev-parse HEAD)" = '{pin["sha"]}'
          node --test tests/*.test.mjs
          python3 - <<'PY'
          import configparser
          from pathlib import Path
          parser = configparser.ConfigParser()
          parser.read('.gitmodules')
          paths = sorted(parser[section]['path'] for section in parser.sections())
          assert all(path.startswith('apps/') for path in paths)
          for required in (
              'apps/canonical-api-server.rs',
              'apps/canonical-interfaces',
              'apps/canonical-marketing-site.web',
              'apps/canonical-mcp-server.rs',
              'apps/canonical-web-server.rs',
          ):
              assert required in paths
              assert Path(required).exists()
          PY
'''


def topology_workflow(contract: dict[str, Any]) -> str:
    pin = contract["source_pins"]["canonical-e2e"]
    return f'''# {HARNESS_MARKER}
name: Canonical test-org topology exact-head staging
run-name: Canonical topology {pin["sha"]} ${{{{ inputs.nonce }}}}

on:
  workflow_dispatch:
    inputs:
      nonce:
        required: true
        type: string

permissions:
  contents: read

jobs:
  test:
    if: github.repository == 'canonical-cloud-test/zed-package-graph-e2e'
    runs-on: ubuntu-24.04
    timeout-minutes: 20
    steps:
      - name: Check out exact Canonical E2E topology source
        uses: actions/checkout@{CHECKOUT_SHA}
        with:
          repository: {pin["repository"]}
          ref: {pin["sha"]}
          path: source
          persist-credentials: false
          fetch-depth: 1
      - name: Install Node.js 22
        uses: actions/setup-node@{SETUP_NODE_SHA}
        with:
          node-version: '22'
      - name: Validate the exact twelve-repository topology
        working-directory: source
        run: |
          set -euo pipefail
          test "$(git rev-parse HEAD)" = '{pin["sha"]}'
          npm run check
          npm run topology:validate
          node src/topology-cli.mjs plan --manifest topology.json --format json > "$RUNNER_TEMP/topology-plan.json"
          python3 - <<'PY'
          import json
          import os
          from pathlib import Path
          manifest = json.loads(Path('topology.json').read_text())
          plan = json.loads(Path(os.environ['RUNNER_TEMP'], 'topology-plan.json').read_text())
          assert manifest['test_org'] == 'canonical-cloud-test'
          assert len(manifest['repositories']) == 12
          assert plan['network_requests'] is False
          assert plan['repository_writes'] is False
          PY
'''


def marker_document(repository: str, sources: dict[str, Any]) -> str:
    return (
        f"<!-- {HARNESS_MARKER} -->\n"
        f"# {repository}\n\n"
        "Isolated Canonical exact-head staging repository. Production source repositories are read-only.\n\n"
        "This harness may be updated only by the reviewed ORESoftware/k8s-cluster Canonical control-plane preflight.\n\n"
        "Pinned sources:\n\n"
        + "\n".join(
            f"- `{item['repository']}@{item['sha']}`" for item in sources.values()
        )
        + "\n"
    )


def put_harness_file(
    client: GitHubClient, repository: str, path: str, content: str, message: str
) -> None:
    encoded_path = urllib.parse.quote(path, safe="/")
    endpoint = f"/repos/canonical-cloud-test/{repository}/contents/{encoded_path}"
    existing = client.get_optional(endpoint, query={"ref": "main"}, label=f"existing {repository}/{path}")
    payload: dict[str, Any] = {
        "message": message,
        "content": base64.b64encode(content.encode()).decode(),
        "branch": "main",
    }
    if existing is not None:
        if not isinstance(existing, dict) or existing.get("type") != "file":
            raise PreflightError(f"test harness path is not a file: {repository}/{path}")
        current = base64.b64decode(str(existing.get("content", ""))).decode(errors="replace")
        if HARNESS_MARKER not in current:
            raise PreflightError(f"refusing to overwrite an unmarked test-org file: {repository}/{path}")
        payload["sha"] = existing.get("sha")
    client.put(endpoint, payload, expected=(200, 201), label=f"write {repository}/{path}")


def install_harnesses(
    client: GitHubClient, contract: dict[str, Any], snapshot: dict[str, Any]
) -> list[dict[str, str]]:
    workflows = {
        "canonical-api-server.rs": api_workflow(contract),
        "web-server-routing-e2e": infra_workflow(contract, snapshot),
        "monorepo-submodules-e2e": monorepo_workflow(contract),
        "zed-package-graph-e2e": topology_workflow(contract),
    }
    installed: list[dict[str, str]] = []
    for repository, workflow in workflows.items():
        put_harness_file(
            client,
            repository,
            "CANONICAL_TEST_HARNESS.md",
            marker_document(repository, contract["source_pins"]),
            "ci(canonical): record exact-head test harness",
        )
        put_harness_file(
            client,
            repository,
            ".github/workflows/canonical-staging.yml",
            workflow,
            "ci(canonical): install exact-head staging workflow",
        )
        installed.append({"repository": repository, "workflow": "canonical-staging.yml"})
    return installed


def dispatch_harnesses(
    client: GitHubClient, installed: list[dict[str, str]], nonce: str
) -> list[dict[str, Any]]:
    started = dt.datetime.now(dt.timezone.utc)
    for item in installed:
        repository = item["repository"]
        workflow = urllib.parse.quote(item["workflow"], safe="")
        path = f"/repos/canonical-cloud-test/{repository}/actions/workflows/{workflow}/dispatches"
        for attempt in range(12):
            try:
                client.post(
                    path,
                    {"ref": "main", "inputs": {"nonce": nonce}},
                    expected=(204,),
                    label=f"dispatch {repository}",
                )
                break
            except PreflightError:
                if attempt == 11:
                    raise
                time.sleep(5)

    tracked: dict[str, dict[str, Any]] = {}
    deadline = time.monotonic() + 3300
    while time.monotonic() < deadline:
        for item in installed:
            repository = item["repository"]
            if tracked.get(repository, {}).get("status") == "completed":
                continue
            workflow = urllib.parse.quote(item["workflow"], safe="")
            path = f"/repos/canonical-cloud-test/{repository}/actions/workflows/{workflow}/runs"
            payload = client.get(
                path,
                query={"event": "workflow_dispatch", "branch": "main", "per_page": "20"},
                label=f"workflow runs for {repository}",
            )
            runs = payload.get("workflow_runs") if isinstance(payload, dict) else None
            if not isinstance(runs, list):
                raise PreflightError(f"workflow run list is invalid: {repository}")
            candidates = [
                run
                for run in runs
                if isinstance(run, dict)
                and isinstance(run.get("created_at"), str)
                and parse_time(run["created_at"]) >= started - dt.timedelta(seconds=10)
                and nonce in str(run.get("display_title", ""))
            ]
            if candidates:
                run = sorted(candidates, key=lambda value: value["created_at"], reverse=True)[0]
                tracked[repository] = {
                    "repository": repository,
                    "run_id_sha256": sha256(run.get("id")),
                    "url": run.get("html_url"),
                    "status": run.get("status"),
                    "conclusion": run.get("conclusion"),
                    "head_sha": run.get("head_sha"),
                    "created_at": run.get("created_at"),
                    "updated_at": run.get("updated_at"),
                }
        if len(tracked) == len(installed) and all(
            item.get("status") == "completed" for item in tracked.values()
        ):
            return [tracked[item["repository"]] for item in installed]
        time.sleep(10)
    missing = [item["repository"] for item in installed if item["repository"] not in tracked]
    raise PreflightError(
        "canonical-cloud-test workflows did not complete before the deadline"
        + (f"; no run observed for {', '.join(missing)}" if missing else "")
    )


def markdown_report(evidence: dict[str, Any]) -> str:
    lines = [
        "# Canonical control-plane read-only preflight",
        "",
        f"Generated: `{evidence['generated_at']}`",
        "",
        "## Safety",
        "",
        "- Production Canonical GitHub source writes: `false`",
        "- Cloudflare writes: `false`",
        "- DNS writes: `false`",
        "- R2 access or writes: `false`",
        "- Kubernetes, database, secret-store, or Google-model writes: `false`",
        "- `canonical-cloud-test` repository writes: `true` (exact allowlist only)",
        "",
    ]
    cloudflare = evidence.get("cloudflare") or {}
    if cloudflare:
        lines.extend(
            [
                "## Cloudflare inventory",
                "",
                f"- Token active: `{cloudflare.get('token', {}).get('status') == 'active'}`",
                f"- Reviewed account hash: `{cloudflare.get('account', {}).get('id_sha256')}`",
                f"- Zone: `{cloudflare.get('zone', {}).get('name')}` (`{cloudflare.get('zone', {}).get('status')}`)",
                f"- Worker present: `{cloudflare.get('worker', {}).get('exists')}`",
                f"- Worker top-level settings readable: `{cloudflare.get('worker', {}).get('settings_verified')}`",
                "",
                "### Exact routes",
                "",
            ]
        )
        for route in cloudflare.get("routes", []):
            state = "present" if route.get("exists") and not route.get("conflict") else "missing/conflict"
            lines.append(f"- `{route.get('pattern')}` — {state}")
        lines.extend(["", "### Exact DNS names", ""])
        for item in cloudflare.get("dns", []):
            record = item.get("record") or {}
            state = (
                f"{record.get('type')}, proxied={record.get('proxied')}, origin redacted"
                if item.get("exists")
                else "missing"
            )
            lines.append(f"- `{item.get('name')}` — {state}")
        lines.append("")

    github = evidence.get("github") or {}
    if github:
        lines.extend(
            [
                "## GitHub isolated staging",
                "",
                f"- Test organization: `{github.get('test_org')}`",
                f"- Membership: `{github.get('membership', {}).get('state')}` / `{github.get('membership', {}).get('role')}`",
                f"- Exact repositories verified: `{len(github.get('repositories', []))}`",
                "",
                "### Workflow results",
                "",
            ]
        )
        for run in github.get("workflow_runs", []):
            lines.append(
                f"- `{run.get('repository')}` — `{run.get('conclusion')}` ({run.get('url')})"
            )
        lines.append("")

    lines.extend(["## Blocking gates", ""])
    for blocker in evidence.get("blockers", []):
        lines.append(f"- {blocker}")
    if not evidence.get("blockers"):
        lines.append("- None recorded by this preflight.")
    if evidence.get("errors"):
        lines.extend(["", "## Execution errors", ""])
        for error in evidence["errors"]:
            lines.append(f"- {error}")
    return "\n".join(lines).rstrip() + "\n"


def run(bundle_path: Path, contract_path: Path, evidence_dir: Path, nonce: str) -> int:
    evidence: dict[str, Any] = {
        "schema_version": 1,
        "generated_at": iso_now(),
        "nonce_sha256": sha256(nonce),
        "mode": "read-only-production-inventory-and-isolated-test-org-staging",
        "live_writes": {
            "canonical_cloud_source_repositories": False,
            "cloudflare": False,
            "dns": False,
            "r2": False,
            "kubernetes": False,
            "database": False,
            "secret_store": False,
            "google_model_configuration": False,
            "canonical_cloud_test_repositories": True,
        },
        "r2": {
            "required": False,
            "access_performed": False,
            "write_performed": False,
            "reason": "no exact Canonical quote bucket or Worker binding is reviewed",
        },
        "blockers": [],
        "errors": [],
        "cloudflare": None,
        "github": None,
    }
    evidence_dir.mkdir(parents=True, exist_ok=True)
    status = 0
    try:
        contract = load_json(contract_path)
        validate_contract(contract)
        bundle = load_json(bundle_path)
        values = validate_bundle(bundle, contract["expected_cloudflare_account_sha256"])
        mask_values(values)

        try:
            cloudflare, blockers = cloudflare_inventory(values, contract)
            evidence["cloudflare"] = cloudflare
            evidence["blockers"].extend(blockers)
        except PreflightError as error:
            evidence["errors"].append(str(error))
            status = 1

        allowed = set(contract["test_repositories"])
        github_client = GitHubClient(
            values["github_token"], contract["source_org"], contract["test_org"], allowed
        )
        try:
            github, github_blockers = verify_github_scope(github_client, contract)
            evidence["blockers"].extend(github_blockers)
            repositories = provision_test_repositories(github_client, contract)
            snapshot = stage_private_infra_snapshot(values, contract)
            installed = install_harnesses(github_client, contract, snapshot)
            workflow_runs = dispatch_harnesses(github_client, installed, nonce)
            github.update(
                {
                    "repositories": repositories,
                    "private_infra_snapshot": snapshot,
                    "installed_harnesses": installed,
                    "workflow_runs": workflow_runs,
                }
            )
            evidence["github"] = github
            failed = [run for run in workflow_runs if run.get("conclusion") != "success"]
            if failed:
                evidence["errors"].append(
                    "canonical-cloud-test workflow failures: "
                    + ", ".join(run["repository"] for run in failed)
                )
                status = 1
        except PreflightError as error:
            evidence["errors"].append(str(error))
            status = 1
    except (PreflightError, json.JSONDecodeError) as error:
        evidence["errors"].append(str(error))
        status = 1

    evidence["blockers"] = sorted(set(evidence["blockers"]))
    evidence["errors"] = sorted(set(evidence["errors"]))
    evidence["ready_for_cloudflare_write"] = False
    evidence["completed_at"] = iso_now()
    (evidence_dir / "results.json").write_text(
        json.dumps(evidence, indent=2, sort_keys=True) + "\n"
    )
    (evidence_dir / "README.md").write_text(markdown_report(evidence))
    print(
        "CANONICAL_CONTROL_PLANE_PREFLIGHT "
        f"status={'success' if status == 0 else 'failed'} "
        f"cloudflare_writes=false r2_access=false test_org_writes=true"
    )
    return status


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bundle", required=True)
    parser.add_argument("--contract", required=True)
    parser.add_argument("--evidence-dir", required=True)
    parser.add_argument("--nonce", required=True)
    args = parser.parse_args()
    return run(Path(args.bundle), Path(args.contract), Path(args.evidence_dir), args.nonce)


if __name__ == "__main__":
    raise SystemExit(main())
