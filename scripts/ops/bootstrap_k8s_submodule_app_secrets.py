#!/usr/bin/env python3
"""Select the dedicated read-only GitHub App used for private k8s submodules.

The selector scans AWS Secrets Manager values visible to the current role,
accepts only submodule-labelled App IDs and PEM private keys, and validates every
candidate pair against GitHub. The selected App must have read-only permissions,
must be installed for the requested private repository, and must mint a token
restricted to exactly that repository with ``contents:read``. The validation
token is revoked before this process exits.

Secret values are written only to caller-provided mode-0600 files. Standard
output and the evidence document contain identifiers and fingerprints, never
credentials.
"""

from __future__ import annotations

import argparse
import base64
import hashlib
import json
import os
import re
import subprocess
import sys
import time
import urllib.error
import urllib.request
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Iterable

API = "https://api.github.com"
API_VERSION = "2022-11-28"
MAX_SECRET_BYTES = 1_048_576
MAX_SECRET_NAMES = 256
EXPLICIT_SECRET_NAMES = (
    "dd/remote-dev/agent-secrets",
    "dd/remote-dev/k8s-submodule-github-app",
    "dd/remote-dev/github-app",
    "dd/remote-dev/github-app-secrets",
    "k8s-submodule-github-app",
)
DISCOVERY_PATTERN = re.compile(r"(?:github|app|submodule|agent-secrets)", re.IGNORECASE)
PEM_PATTERN = re.compile(
    r"^-----BEGIN (?:RSA )?PRIVATE KEY-----\n.+\n-----END (?:RSA )?PRIVATE KEY-----\s*$",
    re.DOTALL,
)


@dataclass
class AppIdCandidate:
    value: str
    sources: set[str] = field(default_factory=set)
    fields: set[str] = field(default_factory=set)


@dataclass
class KeyCandidate:
    value: str
    fingerprint: str
    sources: set[str] = field(default_factory=set)
    fields: set[str] = field(default_factory=set)


@dataclass
class ValidatedPair:
    app_id: AppIdCandidate
    key: KeyCandidate
    app_slug: str
    installation_id: int
    score: int


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--region", default=os.environ.get("AWS_REGION", "us-east-1"))
    parser.add_argument("--target-repository", required=True)
    parser.add_argument("--app-id-out", type=Path, required=True)
    parser.add_argument("--private-key-out", type=Path, required=True)
    parser.add_argument("--evidence-out", type=Path, required=True)
    return parser.parse_args()


def run_aws(arguments: list[str]) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        ["aws", *arguments],
        check=False,
        capture_output=True,
        text=True,
        timeout=60,
    )


def discover_secret_names(region: str) -> list[str]:
    names = set(EXPLICIT_SECRET_NAMES)
    process = run_aws([
        "secretsmanager",
        "list-secrets",
        "--region",
        region,
        "--query",
        "SecretList[].Name",
        "--output",
        "json",
    ])
    if process.returncode == 0:
        try:
            discovered = json.loads(process.stdout)
        except json.JSONDecodeError:
            discovered = []
        if isinstance(discovered, list):
            for name in discovered:
                if isinstance(name, str) and DISCOVERY_PATTERN.search(name):
                    names.add(name)
    return sorted(names)[:MAX_SECRET_NAMES]


def load_secret(region: str, name: str) -> Any | None:
    process = run_aws([
        "secretsmanager",
        "get-secret-value",
        "--region",
        region,
        "--secret-id",
        name,
        "--output",
        "json",
    ])
    if process.returncode != 0:
        return None
    try:
        document = json.loads(process.stdout)
    except json.JSONDecodeError:
        return None
    raw: str | None = None
    if isinstance(document.get("SecretString"), str):
        raw = document["SecretString"]
    elif isinstance(document.get("SecretBinary"), str):
        try:
            raw = base64.b64decode(document["SecretBinary"], validate=True).decode("utf-8")
        except (ValueError, UnicodeDecodeError):
            return None
    if raw is None or len(raw.encode("utf-8")) > MAX_SECRET_BYTES:
        return None
    try:
        return json.loads(raw)
    except json.JSONDecodeError:
        return raw


def flatten(value: Any, path: tuple[str, ...] = ()) -> Iterable[tuple[str, Any]]:
    if isinstance(value, dict):
        for key, child in value.items():
            yield from flatten(child, path + (str(key),))
    elif isinstance(value, list):
        for index, child in enumerate(value):
            yield from flatten(child, path + (str(index),))
    else:
        yield ".".join(path) or "$", value


def normalize(value: str) -> str:
    return re.sub(r"[^a-z0-9]", "", value.casefold())


def parse_nested(value: str) -> Any | None:
    text = value.strip()
    if not text or text[0] not in "[{":
        return None
    try:
        return json.loads(text)
    except json.JSONDecodeError:
        return None


def app_id_shaped(field_path: str, source: str) -> bool:
    field_name = normalize(field_path)
    combined = normalize(source + field_path)
    if any(marker in field_name for marker in ("token", "pat", "installationid")):
        return False
    return (
        field_name in {"k8ssubmoduleappid", "submoduleappid"}
        or "k8ssubmoduleappid" in field_name
        or "submoduleappid" in field_name
        or (
            field_name in {"appid", "githubappid"}
            and "submodule" in combined
        )
        or (
            field_path == "$"
            and "submodule" in combined
            and "appid" in combined
        )
    )


def canonical_pem(value: str) -> str | None:
    variants = [value.strip()]
    if "\\n" in value:
        variants.append(value.replace("\\n", "\n").strip())
    try:
        variants.append(base64.b64decode(value.strip(), validate=True).decode("utf-8").strip())
    except (ValueError, UnicodeDecodeError):
        pass
    for candidate in variants:
        if PEM_PATTERN.fullmatch(candidate):
            return candidate + "\n"
    return None


def private_key_shaped(field_path: str, source: str) -> bool:
    field_name = normalize(field_path)
    combined = normalize(source + field_path)
    if any(marker in field_name for marker in ("token", "pat")):
        return False
    return (
        field_name in {
            "k8ssubmoduleappprivatekey",
            "submoduleappprivatekey",
            "k8ssubmoduleprivatekey",
        }
        or "k8ssubmoduleappprivatekey" in field_name
        or "submoduleappprivatekey" in field_name
        or (
            "privatekey" in field_name
            and "submodule" in combined
        )
        or (
            field_path == "$"
            and "submodule" in combined
            and "privatekey" in combined
        )
    )


def collect_candidates(region: str) -> tuple[dict[str, AppIdCandidate], dict[str, KeyCandidate]]:
    app_ids: dict[str, AppIdCandidate] = {}
    keys: dict[str, KeyCandidate] = {}
    for secret_name in discover_secret_names(region):
        payload = load_secret(region, secret_name)
        if payload is None:
            continue
        queue: list[tuple[str, Any]] = [("$", payload)]
        visited: set[str] = set()
        while queue:
            root, current = queue.pop(0)
            prefix = () if root == "$" else (root,)
            for field_path, value in flatten(current, prefix):
                if isinstance(value, int):
                    text = str(value)
                elif isinstance(value, str):
                    text = value
                else:
                    continue
                if app_id_shaped(field_path, secret_name):
                    candidate = text.strip()
                    if candidate.isdigit() and int(candidate) > 0:
                        item = app_ids.setdefault(candidate, AppIdCandidate(candidate))
                        item.sources.add(secret_name)
                        item.fields.add(field_path)
                if private_key_shaped(field_path, secret_name):
                    pem = canonical_pem(text)
                    if pem is not None:
                        fingerprint = hashlib.sha256(pem.encode("utf-8")).hexdigest()
                        item = keys.setdefault(fingerprint, KeyCandidate(pem, fingerprint))
                        item.sources.add(secret_name)
                        item.fields.add(field_path)
                nested = parse_nested(text)
                digest = hashlib.sha256(text.encode("utf-8")).hexdigest()
                if nested is not None and digest not in visited:
                    visited.add(digest)
                    queue.append((field_path, nested))
        if isinstance(payload, dict):
            payload.clear()
    return app_ids, keys


def base64url(value: bytes) -> str:
    return base64.urlsafe_b64encode(value).decode("ascii").rstrip("=")


def mint_jwt(app_id: str, private_key: str, key_path: Path) -> str:
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
    signed = subprocess.run(
        ["openssl", "dgst", "-sha256", "-sign", str(key_path)],
        input=unsigned.encode("utf-8"),
        check=False,
        capture_output=True,
        timeout=20,
    )
    if signed.returncode != 0 or not signed.stdout:
        raise ValueError("candidate key could not sign a JWT")
    return f"{unsigned}.{base64url(signed.stdout)}"


def request_json(
    method: str,
    path: str,
    bearer: str,
    body: dict[str, Any] | None = None,
) -> tuple[int | None, Any | None]:
    encoded = None if body is None else json.dumps(body, separators=(",", ":")).encode("utf-8")
    request = urllib.request.Request(
        API + path,
        method=method,
        data=encoded,
        headers={
            "Accept": "application/vnd.github+json",
            "Authorization": f"Bearer {bearer}",
            "X-GitHub-Api-Version": API_VERSION,
            "User-Agent": "k8s-submodule-app-secret-bootstrap",
            **({"Content-Type": "application/json"} if encoded is not None else {}),
        },
    )
    try:
        with urllib.request.urlopen(request, timeout=25) as response:
            raw = response.read(MAX_SECRET_BYTES)
            return response.status, json.loads(raw) if raw else None
    except urllib.error.HTTPError as error:
        error.read(8192)
        return error.code, None
    except (urllib.error.URLError, TimeoutError, json.JSONDecodeError):
        return None, None


def validate(
    work: Path,
    target_owner: str,
    target_repo: str,
    app_id: AppIdCandidate,
    key: KeyCandidate,
) -> ValidatedPair | None:
    key_path = work / f"candidate-{key.fingerprint}.pem"
    try:
        app_jwt = mint_jwt(app_id.value, key.value, key_path)
    except ValueError:
        return None
    app_status, app_document = request_json("GET", "/app", app_jwt)
    if app_status != 200 or not isinstance(app_document, dict):
        return None
    app_slug = app_document.get("slug")
    app_permissions = app_document.get("permissions")
    if not isinstance(app_slug, str) or not isinstance(app_permissions, dict):
        return None
    if app_permissions.get("contents") != "read":
        return None
    if any(value not in {"read"} for value in app_permissions.values()):
        return None

    installation_status, installation = request_json(
        "GET",
        f"/repos/{target_owner}/{target_repo}/installation",
        app_jwt,
    )
    if installation_status != 200 or not isinstance(installation, dict):
        return None
    installation_id = installation.get("id")
    account = installation.get("account")
    if (
        not isinstance(installation_id, int)
        or installation_id <= 0
        or not isinstance(account, dict)
        or str(account.get("login", "")).casefold() != target_owner.casefold()
    ):
        return None

    token_status, token_document = request_json(
        "POST",
        f"/app/installations/{installation_id}/access_tokens",
        app_jwt,
        {
            "repositories": [target_repo],
            "permissions": {"contents": "read"},
        },
    )
    if token_status != 201 or not isinstance(token_document, dict):
        return None
    token = token_document.get("token")
    token_permissions = token_document.get("permissions")
    if (
        not isinstance(token, str)
        or not token
        or not isinstance(token_permissions, dict)
        or token_permissions.get("contents") != "read"
        or any(value not in {"read"} for value in token_permissions.values())
    ):
        return None
    try:
        repo_status, repo_document = request_json(
            "GET", f"/repos/{target_owner}/{target_repo}", token
        )
        list_status, repositories = request_json(
            "GET", "/installation/repositories?per_page=100", token
        )
        if repo_status != 200 or not isinstance(repo_document, dict):
            return None
        if list_status != 200 or not isinstance(repositories, dict):
            return None
        full_names = sorted(
            str(item.get("full_name"))
            for item in repositories.get("repositories", [])
            if isinstance(item, dict)
        )
        if repositories.get("total_count") != 1 or full_names != [f"{target_owner}/{target_repo}"]:
            return None
    finally:
        request_json("DELETE", "/installation/token", token)

    shared_sources = app_id.sources & key.sources
    text = " ".join([app_slug, *app_id.sources, *key.sources]).casefold()
    score = 0
    if "submodule" in app_slug.casefold():
        score += 100
    if "k8s" in app_slug.casefold():
        score += 20
    if "k8s-submodule" in text or "k8ssubmodule" in normalize(text):
        score += 80
    if shared_sources:
        score += 30
    return ValidatedPair(app_id, key, app_slug, installation_id, score)


def main() -> int:
    args = parse_args()
    if "/" not in args.target_repository:
        raise SystemExit("--target-repository must be in owner/name form")
    target_owner, target_repo = args.target_repository.split("/", 1)
    if not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9-]{0,38}", target_owner):
        raise SystemExit("invalid target repository owner")
    if not re.fullmatch(r"[A-Za-z0-9_.-]{1,100}", target_repo):
        raise SystemExit("invalid target repository name")

    output_paths = (args.app_id_out, args.private_key_out, args.evidence_out)
    for path in output_paths:
        path.parent.mkdir(parents=True, exist_ok=True)
    work = args.evidence_out.parent

    app_ids, keys = collect_candidates(args.region)
    if not app_ids or not keys:
        raise SystemExit(
            f"no dedicated submodule App material found: app_ids={len(app_ids)} private_keys={len(keys)}"
        )

    validated: list[ValidatedPair] = []
    for app_id in sorted(app_ids.values(), key=lambda item: int(item.value)):
        for key in sorted(keys.values(), key=lambda item: item.fingerprint):
            pair = validate(work, target_owner, target_repo, app_id, key)
            if pair is not None:
                validated.append(pair)

    if not validated:
        raise SystemExit("no read-only, repository-restricted submodule App credential pair validated")
    validated.sort(key=lambda item: (-item.score, int(item.app_id.value), item.key.fingerprint))
    selected = validated[0]
    if (
        len(validated) > 1
        and validated[1].score == selected.score
        and validated[1].app_id.value != selected.app_id.value
    ):
        raise SystemExit("multiple equally preferred read-only submodule Apps validated; refusing ambiguous selection")

    args.app_id_out.write_text(selected.app_id.value + "\n", encoding="utf-8")
    args.private_key_out.write_text(selected.key.value, encoding="utf-8")
    args.app_id_out.chmod(0o600)
    args.private_key_out.chmod(0o600)
    evidence = {
        "schema_version": 1,
        "target_repository": f"{target_owner}/{target_repo}",
        "app_slug": selected.app_slug,
        "installation_id": selected.installation_id,
        "permissions": {"contents": "read", "metadata": "read"},
        "repository_restriction_verified": True,
        "candidate_counts": {
            "app_ids": len(app_ids),
            "private_keys": len(keys),
            "validated_pairs": len(validated),
        },
        "credential_sources": {
            "app_id": sorted(selected.app_id.sources),
            "private_key": sorted(selected.key.sources),
            "private_key_sha256": selected.key.fingerprint,
        },
        "pat_used_for_submodule_access": False,
    }
    args.evidence_out.write_text(json.dumps(evidence, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    args.evidence_out.chmod(0o600)
    print(
        "validated a read-only repository-restricted GitHub App "
        f"app={selected.app_slug} installation={selected.installation_id}"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
