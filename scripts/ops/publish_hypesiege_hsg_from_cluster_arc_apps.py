#!/usr/bin/env python3
"""Create the HypeSiege hsg-* fleet from Kubernetes-held GitHub App credentials.

The script is designed to run on the protected cluster administration host. It
reads only Kubernetes Secret entries that contain an App-ID field and a PEM
private-key field. PAT/token-only Secrets are ignored. Every candidate pair is
validated against the live HypeSiege organization installation and must expose
all-repositories repository-administration, contents, and pull-request write
permissions. Secret values are never printed or written to the report.
"""

from __future__ import annotations

import argparse
import base64
import hashlib
import json
import re
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

API_URL = "https://api.github.com"
API_VERSION = "2022-11-28"
KUBECONFIGS = (
    "/etc/kubernetes/admin.conf",
    "/root/.kube/config",
    "/home/ec2-user/.kube/config",
)
TARGETS = {
    "hsg-web-mash": "HypeSiege MASH web server: Maud, Axum, SeaORM, Supabase Auth, htmx, and WebSockets",
    "hsg-web-leptos": "HypeSiege Leptos SSR web server and realtime dashboard",
    "hsg-web-dioxus": "HypeSiege Dioxus SSR web server and realtime dashboard",
    "hsg-api": "HypeSiege Rust API, publishing, approvals, inbox, and WebSockets",
    "hsg-infra": "HypeSiege Kubernetes and least-privilege infrastructure",
    "hsg-cli": "HypeSiege Rust CLI with audited flags-2-env configuration",
    "hsg-interfaces": "HypeSiege SQL, OpenAPI, AsyncAPI, schemas, NATS, and generated contracts",
    "hsg-sync": "HypeSiege offline-first synchronization facade",
}
APP_ID_NAMES = {
    "githubappid",
    "appid",
    "k8ssubmoduleappid",
    "repositoryadminappid",
}
PRIVATE_KEY_NAMES = {
    "githubappprivatekey",
    "appprivatekey",
    "privatekey",
    "k8ssubmoduleappprivatekey",
    "repositoryadminappprivatekey",
}
PEM_PATTERN = re.compile(
    r"^-----BEGIN (?:RSA )?PRIVATE KEY-----\n.+\n-----END (?:RSA )?PRIVATE KEY-----\s*$",
    re.DOTALL,
)


@dataclass
class Candidate:
    app_id: str
    private_key: str
    fingerprint: str
    sources: set[str] = field(default_factory=set)


@dataclass(frozen=True)
class Validated:
    candidate: Candidate
    app_slug: str
    installation_id: int
    permissions: dict[str, str]


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--organization", default="hypesiege")
    parser.add_argument("--report", type=Path, required=True)
    parser.add_argument("--self-test", action="store_true")
    return parser.parse_args()


def normalize(value: str) -> str:
    return re.sub(r"[^a-z0-9]", "", value.casefold())


def decode_secret_value(value: str) -> bytes | None:
    try:
        return base64.b64decode(value, validate=True)
    except (ValueError, TypeError):
        return None


def canonical_pem(value: str) -> str | None:
    variants = [value.strip()]
    if "\\n" in value:
        variants.append(value.replace("\\n", "\n").strip())
    for variant in variants:
        if PEM_PATTERN.fullmatch(variant):
            return variant + "\n"
    return None


def extract_pairs(data: dict[str, str], source: str) -> list[Candidate]:
    decoded: dict[str, str] = {}
    for name, encoded in data.items():
        raw = decode_secret_value(encoded)
        if raw is None:
            continue
        try:
            decoded[name] = raw.decode("utf-8")
        except UnicodeDecodeError:
            continue

    app_ids: list[str] = []
    keys: list[str] = []
    for name, value in decoded.items():
        normalized = normalize(name)
        if normalized in APP_ID_NAMES and value.strip().isdigit() and int(value.strip()) > 0:
            app_ids.append(value.strip())
        if normalized in PRIVATE_KEY_NAMES:
            pem = canonical_pem(value)
            if pem is not None:
                keys.append(pem)

        stripped = value.strip()
        if stripped.startswith("{"):
            try:
                nested = json.loads(stripped)
            except json.JSONDecodeError:
                nested = None
            if isinstance(nested, dict):
                nested_ids: list[str] = []
                nested_keys: list[str] = []
                for nested_name, nested_value in nested.items():
                    if not isinstance(nested_value, (str, int)):
                        continue
                    text = str(nested_value)
                    normalized_nested = normalize(str(nested_name))
                    if (
                        normalized_nested in APP_ID_NAMES
                        and text.strip().isdigit()
                        and int(text.strip()) > 0
                    ):
                        nested_ids.append(text.strip())
                    if normalized_nested in PRIVATE_KEY_NAMES:
                        pem = canonical_pem(text)
                        if pem is not None:
                            nested_keys.append(pem)
                app_ids.extend(nested_ids)
                keys.extend(nested_keys)

    result: list[Candidate] = []
    for app_id in sorted(set(app_ids), key=int):
        for private_key in keys:
            fingerprint = hashlib.sha256(private_key.encode("utf-8")).hexdigest()
            result.append(
                Candidate(
                    app_id=app_id,
                    private_key=private_key,
                    fingerprint=fingerprint,
                    sources={source},
                )
            )
    return result


def discover_candidates() -> tuple[list[Candidate], dict[str, int]]:
    candidates: dict[tuple[str, str], Candidate] = {}
    stats = {"readable_kubeconfigs": 0, "secrets_inspected": 0, "app_pair_secrets": 0}
    seen_clusters: set[str] = set()

    for kubeconfig in KUBECONFIGS:
        path = Path(kubeconfig)
        if not path.is_file():
            continue
        context = subprocess.run(
            ["kubectl", "--kubeconfig", kubeconfig, "config", "current-context"],
            check=False,
            capture_output=True,
            text=True,
            timeout=20,
        )
        context_name = context.stdout.strip() if context.returncode == 0 else kubeconfig
        if context_name in seen_clusters:
            continue
        process = subprocess.run(
            ["kubectl", "--kubeconfig", kubeconfig, "get", "secrets", "-A", "-o", "json"],
            check=False,
            capture_output=True,
            text=True,
            timeout=90,
        )
        if process.returncode != 0:
            continue
        try:
            document = json.loads(process.stdout)
        except json.JSONDecodeError:
            continue
        items = document.get("items") if isinstance(document, dict) else None
        if not isinstance(items, list):
            continue
        stats["readable_kubeconfigs"] += 1
        seen_clusters.add(context_name)
        for item in items:
            if not isinstance(item, dict):
                continue
            metadata = item.get("metadata")
            data = item.get("data")
            if not isinstance(metadata, dict) or not isinstance(data, dict):
                continue
            stats["secrets_inspected"] += 1
            namespace = metadata.get("namespace")
            name = metadata.get("name")
            if not isinstance(namespace, str) or not isinstance(name, str):
                continue
            pairs = extract_pairs(data, f"{context_name}:{namespace}/{name}")
            if pairs:
                stats["app_pair_secrets"] += 1
            for pair in pairs:
                key = (pair.app_id, pair.fingerprint)
                if key in candidates:
                    candidates[key].sources.update(pair.sources)
                else:
                    candidates[key] = pair
        document.clear()
        process = None

    return sorted(candidates.values(), key=lambda item: (int(item.app_id), item.fingerprint)), stats


def base64url(value: bytes) -> str:
    return base64.urlsafe_b64encode(value).decode("ascii").rstrip("=")


def mint_app_jwt(app_id: str, private_key: str, directory: Path) -> str:
    key_path = directory / f"app-{hashlib.sha256(private_key.encode()).hexdigest()}.pem"
    key_path.write_text(private_key, encoding="utf-8")
    key_path.chmod(0o600)
    now = int(time.time())
    header = base64url(b'{"alg":"RS256","typ":"JWT"}')
    payload = base64url(
        json.dumps(
            {"iat": now - 60, "exp": now + 540, "iss": int(app_id)},
            separators=(",", ":"),
        ).encode("utf-8")
    )
    unsigned = f"{header}.{payload}"
    process = subprocess.run(
        ["openssl", "dgst", "-sha256", "-sign", str(key_path)],
        input=unsigned.encode("ascii"),
        check=False,
        capture_output=True,
        timeout=20,
    )
    if process.returncode != 0 or not process.stdout:
        raise ValueError("private key could not sign")
    return f"{unsigned}.{base64url(process.stdout)}"


def request_json(
    method: str,
    path: str,
    bearer: str,
    body: dict[str, Any] | None = None,
) -> tuple[int | None, Any | None]:
    encoded = None if body is None else json.dumps(body, separators=(",", ":")).encode("utf-8")
    request = urllib.request.Request(
        API_URL + path,
        method=method,
        data=encoded,
        headers={
            "Accept": "application/vnd.github+json",
            "Authorization": f"Bearer {bearer}",
            "X-GitHub-Api-Version": API_VERSION,
            "User-Agent": "hypesiege-cluster-arc-app-publisher",
            **({"Content-Type": "application/json"} if encoded is not None else {}),
        },
    )
    try:
        with urllib.request.urlopen(request, timeout=30) as response:
            raw = response.read(1_048_576)
            return response.status, json.loads(raw) if raw else None
    except urllib.error.HTTPError as error:
        error.read(8192)
        return error.code, None
    except (urllib.error.URLError, TimeoutError, json.JSONDecodeError):
        return None, None


def mint_installation_token(app_jwt: str, installation_id: int) -> tuple[str, dict[str, str]] | None:
    status, document = request_json(
        "POST", f"/app/installations/{installation_id}/access_tokens", app_jwt, {}
    )
    if status != 201 or not isinstance(document, dict):
        return None
    token = document.get("token")
    permissions = document.get("permissions")
    if not isinstance(token, str) or not isinstance(permissions, dict):
        return None
    observed = {
        name: str(permissions.get(name, "none"))
        for name in ("administration", "contents", "pull_requests", "metadata")
    }
    return token, observed


def validate_candidate(candidate: Candidate, organization: str, directory: Path) -> Validated | None:
    try:
        app_jwt = mint_app_jwt(candidate.app_id, candidate.private_key, directory)
    except ValueError:
        return None
    status, installation = request_json("GET", f"/orgs/{organization}/installation", app_jwt)
    if status != 200 or not isinstance(installation, dict):
        return None
    if installation.get("repository_selection") != "all":
        return None
    installation_id = installation.get("id")
    app_slug = installation.get("app_slug")
    if not isinstance(installation_id, int) or not isinstance(app_slug, str):
        return None
    minted = mint_installation_token(app_jwt, installation_id)
    if minted is None:
        return None
    token, permissions = minted
    request_json("DELETE", "/installation/token", token)
    required = {
        "administration": "write",
        "contents": "write",
        "pull_requests": "write",
        "metadata": "read",
    }
    if permissions != required:
        return None
    return Validated(candidate, app_slug, installation_id, permissions)


def create_repositories(validated: Validated, organization: str, directory: Path) -> list[dict[str, Any]]:
    app_jwt = mint_app_jwt(
        validated.candidate.app_id, validated.candidate.private_key, directory
    )
    minted = mint_installation_token(app_jwt, validated.installation_id)
    if minted is None:
        raise RuntimeError("failed to mint publication installation token")
    token, permissions = minted
    if permissions != validated.permissions:
        request_json("DELETE", "/installation/token", token)
        raise RuntimeError("installation token permission drift")

    results: list[dict[str, Any]] = []
    try:
        for name, description in TARGETS.items():
            full_name = f"{organization}/{name}"
            status, metadata = request_json("GET", f"/repos/{full_name}", token)
            created = False
            if status == 404:
                status, metadata = request_json(
                    "POST",
                    f"/orgs/{organization}/repos",
                    token,
                    {
                        "name": name,
                        "description": description,
                        "private": True,
                        "has_issues": True,
                        "has_projects": False,
                        "has_wiki": False,
                        "auto_init": True,
                    },
                )
                created = True
                if status != 201:
                    raise RuntimeError(f"repository create failed for {full_name}: HTTP {status}")
            elif status != 200:
                raise RuntimeError(f"repository lookup failed for {full_name}: HTTP {status}")
            if not isinstance(metadata, dict):
                raise RuntimeError(f"invalid repository metadata for {full_name}")
            if (
                metadata.get("full_name") != full_name
                or metadata.get("private") is not True
                or metadata.get("default_branch") != "main"
            ):
                raise RuntimeError(f"repository verification failed for {full_name}")
            results.append(
                {
                    "full_name": full_name,
                    "created": created,
                    "private": True,
                    "default_branch": "main",
                    "html_url": metadata.get("html_url"),
                }
            )
    finally:
        request_json("DELETE", "/installation/token", token)
    return results


def self_test() -> None:
    encoded = {
        "github_app_id": base64.b64encode(b"12345").decode(),
        "github_app_private_key": base64.b64encode(
            b"-----BEGIN PRIVATE KEY-----\nabc\n-----END PRIVATE KEY-----\n"
        ).decode(),
        "github_token": base64.b64encode(b"ignored").decode(),
    }
    pairs = extract_pairs(encoded, "test:namespace/secret")
    assert len(pairs) == 1
    assert pairs[0].app_id == "12345"
    assert pairs[0].sources == {"test:namespace/secret"}
    print("cluster ARC App publisher self-test: ok")


def main() -> int:
    args = parse_args()
    if args.self_test:
        self_test()
        return 0

    candidates, stats = discover_candidates()
    if not candidates:
        raise SystemExit(
            "no Kubernetes GitHub App credential pairs found "
            f"kubeconfigs={stats['readable_kubeconfigs']} "
            f"secrets={stats['secrets_inspected']}"
        )

    validated: dict[tuple[str, str], Validated] = {}
    with tempfile.TemporaryDirectory(prefix="hsg-cluster-app-") as temporary:
        directory = Path(temporary)
        for candidate in candidates:
            result = validate_candidate(candidate, args.organization, directory)
            if result is not None:
                validated[(candidate.app_id, candidate.fingerprint)] = result
        if len(validated) != 1:
            raise SystemExit(
                "expected exactly one all-repositories repository-admin App "
                f"for {args.organization}; found {len(validated)} from {len(candidates)} candidates"
            )
        selected = next(iter(validated.values()))
        repositories = create_repositories(selected, args.organization, directory)

    report = {
        "schema_version": 1,
        "organization": args.organization,
        "credential": "kubernetes_github_app_installation_token",
        "pat_used": False,
        "app_slug": selected.app_slug,
        "installation_id": selected.installation_id,
        "repository_selection": "all",
        "permissions": selected.permissions,
        "credential_sources": sorted(selected.candidate.sources),
        "private_key_sha256": selected.candidate.fingerprint,
        "discovery": stats,
        "repositories": repositories,
    }
    if len(repositories) != 8:
        raise RuntimeError("publication did not verify all eight repositories")
    args.report.parent.mkdir(parents=True, exist_ok=True)
    args.report.write_text(json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    print(
        "HSG_CLUSTER_APP_PUBLICATION_SUCCESS "
        f"app={selected.app_slug} installation={selected.installation_id} repositories=8"
    )
    for repository in repositories:
        print(
            "HSG_REPOSITORY_READY "
            f"{repository['full_name']} created={str(repository['created']).lower()}"
        )
    return 0


if __name__ == "__main__":
    sys.exit(main())
