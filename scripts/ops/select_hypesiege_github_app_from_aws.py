#!/usr/bin/env python3
"""Select one repository-admin GitHub App credential pair from AWS Secrets Manager.

The selector never accepts GitHub tokens or PATs. It inspects only App-ID-shaped
fields and PEM private-key fields, validates every candidate pair by minting an
App JWT, resolving the requested organization installation, and checking a
short-lived installation token's bounded permissions. Secret values are written
only to caller-owned mode-0600 output files and are never printed.
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
import tempfile
import time
import urllib.error
import urllib.request
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Iterable

API_URL = "https://api.github.com"
API_VERSION = "2022-11-28"
MAX_DISCOVERED_SECRETS = 96
EXPLICIT_SECRET_NAMES = (
    "dd/remote-dev/agent-secrets",
    "dd/remote-dev/github-app",
    "dd/remote-dev/github-app-secrets",
    "dd/remote-dev/k8s-submodule-github-app",
    "dd/remote-dev/repository-admin-github-app",
    "k8s-submodule-github-app",
    "hypesiege/github-app",
    "hypesiege/repository-admin-github-app",
    "canonical-cloud/arc-github",
    "sonus-auris/arc-github",
)
SECRET_NAME_PATTERN = re.compile(
    r"(?:github|git-hub|app|arc|runner|submodule|agent-secrets)", re.IGNORECASE
)
PEM_PATTERN = re.compile(
    r"^-----BEGIN (?:RSA )?PRIVATE KEY-----\n.+\n-----END (?:RSA )?PRIVATE KEY-----\s*$",
    re.DOTALL,
)


@dataclass(frozen=True)
class AppIdCandidate:
    secret_name: str
    field_path: str
    value: str


@dataclass(frozen=True)
class PrivateKeyCandidate:
    secret_name: str
    field_path: str
    value: str
    fingerprint: str


@dataclass(frozen=True)
class ValidatedPair:
    app_id: AppIdCandidate
    private_key: PrivateKeyCandidate
    app_slug: str
    installation_id: int
    repository_selection: str
    permissions: dict[str, str]


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--organization", required=True)
    parser.add_argument("--app-id-out", type=Path, required=True)
    parser.add_argument("--private-key-out", type=Path, required=True)
    parser.add_argument("--evidence-out", type=Path, required=True)
    parser.add_argument("--region", default=os.environ.get("AWS_REGION", "us-east-1"))
    parser.add_argument("--self-test", action="store_true")
    return parser.parse_args()


def run_aws(arguments: list[str], *, allow_failure: bool = False) -> subprocess.CompletedProcess[str]:
    process = subprocess.run(
        ["aws", *arguments],
        check=False,
        capture_output=True,
        text=True,
        timeout=60,
    )
    if process.returncode != 0 and not allow_failure:
        raise RuntimeError("AWS Secrets Manager request failed")
    return process


def discover_secret_names(region: str) -> list[str]:
    names = set(EXPLICIT_SECRET_NAMES)
    process = run_aws(
        [
            "secretsmanager",
            "list-secrets",
            "--region",
            region,
            "--query",
            "SecretList[].Name",
            "--output",
            "json",
        ],
        allow_failure=True,
    )
    if process.returncode == 0:
        try:
            discovered = json.loads(process.stdout)
        except json.JSONDecodeError:
            discovered = []
        if isinstance(discovered, list):
            for value in discovered:
                if isinstance(value, str) and SECRET_NAME_PATTERN.search(value):
                    names.add(value)
    return sorted(names)[:MAX_DISCOVERED_SECRETS]


def load_secret(name: str, region: str) -> Any | None:
    process = run_aws(
        [
            "secretsmanager",
            "get-secret-value",
            "--region",
            region,
            "--secret-id",
            name,
            "--output",
            "json",
        ],
        allow_failure=True,
    )
    if process.returncode != 0:
        return None
    try:
        response = json.loads(process.stdout)
    except json.JSONDecodeError:
        return None
    raw: str | None = None
    if isinstance(response.get("SecretString"), str):
        raw = response["SecretString"]
    elif isinstance(response.get("SecretBinary"), str):
        try:
            raw = base64.b64decode(response["SecretBinary"], validate=True).decode("utf-8")
        except (ValueError, UnicodeDecodeError):
            return None
    if raw is None:
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


def normalized_name(value: str) -> str:
    return re.sub(r"[^a-z0-9]", "", value.casefold())


def is_app_id_field(path: str) -> bool:
    normalized = normalized_name(path)
    return (
        normalized in {"appid", "githubappid", "k8ssubmoduleappid"}
        or "githubappid" in normalized
        or "submoduleappid" in normalized
        or ("appid" in normalized and any(token in normalized for token in ("github", "repository", "arc")))
    )


def is_private_key_field(path: str, secret_name: str) -> bool:
    normalized = normalized_name(path)
    secret_normalized = normalized_name(secret_name)
    return (
        normalized in {"appprivatekey", "githubappprivatekey", "k8ssubmoduleappprivatekey"}
        or "githubappprivatekey" in normalized
        or "submoduleappprivatekey" in normalized
        or (
            "privatekey" in normalized
            and any(token in normalized for token in ("github", "app", "arc"))
        )
        or (path == "$" and any(token in secret_normalized for token in ("githubapp", "arcgithub")))
    )


def collect_candidates(secret_names: list[str], region: str) -> tuple[list[AppIdCandidate], list[PrivateKeyCandidate]]:
    app_ids: dict[str, AppIdCandidate] = {}
    keys: dict[str, PrivateKeyCandidate] = {}
    for secret_name in secret_names:
        payload = load_secret(secret_name, region)
        if payload is None:
            continue
        for field_path, value in flatten(payload):
            if isinstance(value, int):
                text = str(value)
            elif isinstance(value, str):
                text = value
            else:
                continue
            if is_app_id_field(field_path) and text.isdigit() and int(text) > 0:
                app_ids.setdefault(
                    text,
                    AppIdCandidate(secret_name=secret_name, field_path=field_path, value=text),
                )
            if is_private_key_field(field_path, secret_name) and PEM_PATTERN.fullmatch(text.strip()):
                canonical = text.strip() + "\n"
                fingerprint = hashlib.sha256(canonical.encode("utf-8")).hexdigest()
                keys.setdefault(
                    fingerprint,
                    PrivateKeyCandidate(
                        secret_name=secret_name,
                        field_path=field_path,
                        value=canonical,
                        fingerprint=fingerprint,
                    ),
                )
        if isinstance(payload, dict):
            payload.clear()
    return sorted(app_ids.values(), key=lambda item: int(item.value)), sorted(
        keys.values(), key=lambda item: item.fingerprint
    )


def base64url(value: bytes) -> str:
    return base64.urlsafe_b64encode(value).decode("ascii").rstrip("=")


def mint_app_jwt(app_id: str, private_key: str, work_dir: Path) -> str:
    key_path = work_dir / f"candidate-{hashlib.sha256(private_key.encode()).hexdigest()}.pem"
    key_path.write_text(private_key, encoding="utf-8")
    key_path.chmod(0o600)
    now = int(time.time())
    header = base64url(json.dumps({"alg": "RS256", "typ": "JWT"}, separators=(",", ":")).encode())
    payload = base64url(
        json.dumps(
            {"iat": now - 60, "exp": now + 540, "iss": int(app_id)},
            separators=(",", ":"),
        ).encode()
    )
    unsigned = f"{header}.{payload}"
    process = subprocess.run(
        ["openssl", "dgst", "-sha256", "-sign", str(key_path)],
        input=unsigned.encode(),
        check=False,
        capture_output=True,
        timeout=20,
    )
    if process.returncode != 0 or not process.stdout:
        raise ValueError("candidate private key cannot sign an App JWT")
    return f"{unsigned}.{base64url(process.stdout)}"


def request_json(
    method: str,
    path: str,
    bearer: str,
    body: dict[str, Any] | None = None,
) -> tuple[int | None, Any | None]:
    encoded = None if body is None else json.dumps(body, separators=(",", ":")).encode()
    request = urllib.request.Request(
        API_URL + path,
        method=method,
        data=encoded,
        headers={
            "Accept": "application/vnd.github+json",
            "Authorization": f"Bearer {bearer}",
            "X-GitHub-Api-Version": API_VERSION,
            "User-Agent": "hypesiege-github-app-selector",
            **({"Content-Type": "application/json"} if encoded is not None else {}),
        },
    )
    try:
        with urllib.request.urlopen(request, timeout=25) as response:
            raw = response.read(1_048_576)
            return response.status, json.loads(raw) if raw else None
    except urllib.error.HTTPError as error:
        error.read(8192)
        return error.code, None
    except (urllib.error.URLError, TimeoutError, json.JSONDecodeError):
        return None, None


def validate_pair(
    app_id: AppIdCandidate,
    private_key: PrivateKeyCandidate,
    organization: str,
    work_dir: Path,
) -> ValidatedPair | None:
    try:
        app_jwt = mint_app_jwt(app_id.value, private_key.value, work_dir)
    except ValueError:
        return None
    installation_status, installation = request_json(
        "GET", f"/orgs/{organization}/installation", app_jwt
    )
    if installation_status != 200 or not isinstance(installation, dict):
        return None
    if installation.get("repository_selection") != "all":
        return None
    installation_id = installation.get("id")
    app_slug = installation.get("app_slug")
    if not isinstance(installation_id, int) or installation_id <= 0 or not isinstance(app_slug, str):
        return None
    token_status, token_document = request_json(
        "POST", f"/app/installations/{installation_id}/access_tokens", app_jwt, {}
    )
    if token_status != 201 or not isinstance(token_document, dict):
        return None
    token = token_document.get("token")
    permissions = token_document.get("permissions")
    if not isinstance(token, str) or not isinstance(permissions, dict):
        return None
    observed = {
        name: str(permissions.get(name, "none"))
        for name in ("administration", "contents", "pull_requests", "metadata")
    }
    required = {
        "administration": "write",
        "contents": "write",
        "pull_requests": "write",
        "metadata": "read",
    }
    request_json("DELETE", "/installation/token", token)
    if observed != required:
        return None
    return ValidatedPair(
        app_id=app_id,
        private_key=private_key,
        app_slug=app_slug,
        installation_id=installation_id,
        repository_selection="all",
        permissions=observed,
    )


def write_private(path: Path, value: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(value, encoding="utf-8")
    path.chmod(0o600)


def self_test() -> None:
    fixture = {
        "nested": {
            "K8S_SUBMODULE_APP_ID": 12345,
            "K8S_SUBMODULE_APP_PRIVATE_KEY": "-----BEGIN PRIVATE KEY-----\nabc\n-----END PRIVATE KEY-----",
        },
        "GH_PAT": "must-be-ignored",
    }
    flattened = dict(flatten(fixture))
    assert is_app_id_field("nested.K8S_SUBMODULE_APP_ID")
    assert is_private_key_field("nested.K8S_SUBMODULE_APP_PRIVATE_KEY", "fixture")
    assert not is_app_id_field("GH_PAT")
    assert list(flatten("raw")) == [("$", "raw")]
    assert flattened["nested.K8S_SUBMODULE_APP_ID"] == 12345
    print("hypesiege App selector self-test: ok")


def main() -> int:
    args = parse_args()
    if args.self_test:
        self_test()
        return 0
    names = discover_secret_names(args.region)
    app_ids, keys = collect_candidates(names, args.region)
    if not app_ids or not keys:
        raise SystemExit(
            f"no App credential candidates found: app_ids={len(app_ids)} private_keys={len(keys)}"
        )
    validated: dict[tuple[str, str], ValidatedPair] = {}
    with tempfile.TemporaryDirectory(prefix="hsg-app-selector-") as directory:
        work_dir = Path(directory)
        for app_id in app_ids:
            for private_key in keys:
                pair = validate_pair(app_id, private_key, args.organization, work_dir)
                if pair is not None:
                    validated[(app_id.value, private_key.fingerprint)] = pair
    if len(validated) != 1:
        raise SystemExit(f"expected exactly one repository-admin App pair; found {len(validated)}")
    selected = next(iter(validated.values()))
    write_private(args.app_id_out, selected.app_id.value + "\n")
    write_private(args.private_key_out, selected.private_key.value)
    evidence = {
        "schema_version": 1,
        "organization": args.organization,
        "app_slug": selected.app_slug,
        "installation_id": selected.installation_id,
        "repository_selection": selected.repository_selection,
        "permissions": selected.permissions,
        "credential_sources": {
            "app_id_secret": selected.app_id.secret_name,
            "app_id_field": selected.app_id.field_path,
            "private_key_secret": selected.private_key.secret_name,
            "private_key_field": selected.private_key.field_path,
            "private_key_sha256": selected.private_key.fingerprint,
        },
        "pat_used": False,
    }
    args.evidence_out.parent.mkdir(parents=True, exist_ok=True)
    args.evidence_out.write_text(json.dumps(evidence, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    print(
        "validated GitHub App credential pair "
        f"app={selected.app_slug} installation={selected.installation_id} source={selected.app_id.secret_name}"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
