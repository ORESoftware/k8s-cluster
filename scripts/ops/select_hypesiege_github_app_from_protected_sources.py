#!/usr/bin/env python3
"""Select one HypeSiege repository-admin GitHub App from protected sources.

The selector runs only on the protected cluster administration host. It scans:

* every readable local Kubernetes context and its Secret objects;
* AWS Secrets Manager values visible to the host role; and
* encrypted SSM Parameter Store values visible to the host role.

Only numeric App-ID-shaped fields and PEM private-key-shaped fields are
considered. PAT/token-only fields are ignored. Candidate pairs are validated
against the live ``hypesiege`` organization installation, must cover all
repositories, and must mint a short-lived installation token with repository
administration, contents, pull-request, and metadata permissions. Secret values
are never printed or written to evidence.
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
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Iterable

API_URL = "https://api.github.com"
API_VERSION = "2022-11-28"
MAX_AWS_SECRETS = 512
MAX_SSM_PARAMETERS = 512
MAX_SECRET_BYTES = 1_048_576
FIXED_KUBECONFIGS = (
    "/etc/kubernetes/admin.conf",
    "/etc/rancher/k3s/k3s.yaml",
    "/root/.kube/config",
    "/home/ec2-user/.kube/config",
    "/home/ubuntu/.kube/config",
)
KUBECONFIG_ROOTS = (
    "/etc/kubernetes",
    "/etc/rancher",
    "/root/.kube",
    "/home/ec2-user/.kube",
    "/home/ubuntu/.kube",
)
PEM_PATTERN = re.compile(
    r"^-----BEGIN (?:RSA )?PRIVATE KEY-----\n.+\n-----END (?:RSA )?PRIVATE KEY-----\s*$",
    re.DOTALL,
)


@dataclass
class AppIdCandidate:
    value: str
    sources: set[str] = field(default_factory=set)


@dataclass
class KeyCandidate:
    value: str
    fingerprint: str
    sources: set[str] = field(default_factory=set)


@dataclass(frozen=True)
class ValidatedPair:
    app_id: AppIdCandidate
    private_key: KeyCandidate
    app_slug: str
    installation_id: int
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


def normalize(value: str) -> str:
    return re.sub(r"[^a-z0-9]", "", value.casefold())


def flatten(value: Any, path: tuple[str, ...] = ()) -> Iterable[tuple[str, Any]]:
    if isinstance(value, dict):
        for key, child in value.items():
            yield from flatten(child, path + (str(key),))
    elif isinstance(value, list):
        for index, child in enumerate(value):
            yield from flatten(child, path + (str(index),))
    else:
        yield ".".join(path) or "$", value


def app_id_shaped(field_path: str, source: str) -> bool:
    field = normalize(field_path)
    combined = normalize(source + field_path)
    if any(marker in field for marker in ("pat", "token", "installationid")):
        return False
    return (
        field in {
            "appid",
            "githubappid",
            "k8ssubmoduleappid",
            "repositoryadminappid",
            "arcgithubappid",
        }
        or "githubappid" in field
        or "submoduleappid" in field
        or "repositoryadminappid" in field
        or (
            field_path == "$"
            and any(
                marker in combined
                for marker in (
                    "githubappid",
                    "repositoryadminappid",
                    "submoduleappid",
                    "arcgithubappid",
                )
            )
        )
    )


def private_key_shaped(field_path: str, source: str) -> bool:
    field = normalize(field_path)
    combined = normalize(source + field_path)
    if any(marker in field for marker in ("pat", "token")):
        return False
    return (
        field in {
            "privatekey",
            "appprivatekey",
            "githubappprivatekey",
            "k8ssubmoduleappprivatekey",
            "repositoryadminappprivatekey",
            "arcgithubappprivatekey",
        }
        or "githubappprivatekey" in field
        or "submoduleappprivatekey" in field
        or "repositoryadminappprivatekey" in field
        or (
            "privatekey" in field
            and any(marker in field for marker in ("github", "app", "arc", "submodule"))
        )
        or (
            field_path == "$"
            and "privatekey" in combined
            and any(marker in combined for marker in ("githubapp", "arcgithub", "submoduleapp"))
        )
    )


def canonical_pem(value: str) -> str | None:
    variants = [value.strip()]
    if "\\n" in value:
        variants.append(value.replace("\\n", "\n").strip())
    try:
        decoded = base64.b64decode(value.strip(), validate=True).decode("utf-8").strip()
        variants.append(decoded)
    except (ValueError, UnicodeDecodeError):
        pass
    for candidate in variants:
        if PEM_PATTERN.fullmatch(candidate):
            return candidate + "\n"
    return None


def parse_nested(value: str) -> Any | None:
    stripped = value.strip()
    if not stripped or stripped[0] not in "[{":
        return None
    try:
        return json.loads(stripped)
    except json.JSONDecodeError:
        return None


def collect_material(
    payload: Any,
    source: str,
    app_ids: dict[str, AppIdCandidate],
    keys: dict[str, KeyCandidate],
) -> bool:
    relevant = False
    queue: list[tuple[str, Any]] = [("$", payload)]
    visited_nested: set[str] = set()
    while queue:
        root, current = queue.pop(0)
        for field_path, value in flatten(current, (() if root == "$" else (root,))):
            if isinstance(value, int):
                text = str(value)
            elif isinstance(value, str):
                text = value
            else:
                continue

            if app_id_shaped(field_path, source):
                candidate = text.strip()
                if candidate.isdigit() and int(candidate) > 0:
                    app_ids.setdefault(candidate, AppIdCandidate(candidate)).sources.add(source)
                    relevant = True

            if private_key_shaped(field_path, source):
                pem = canonical_pem(text)
                if pem is not None:
                    fingerprint = hashlib.sha256(pem.encode("utf-8")).hexdigest()
                    keys.setdefault(fingerprint, KeyCandidate(pem, fingerprint)).sources.add(source)
                    relevant = True

            nested = parse_nested(text)
            nested_hash = hashlib.sha256(text.encode("utf-8")).hexdigest()
            if nested is not None and nested_hash not in visited_nested:
                visited_nested.add(nested_hash)
                queue.append((field_path, nested))
    return relevant


def run(
    command: list[str],
    *,
    timeout: int = 60,
    input_bytes: bytes | None = None,
) -> subprocess.CompletedProcess[bytes]:
    return subprocess.run(
        command,
        input=input_bytes,
        check=False,
        capture_output=True,
        timeout=timeout,
    )


def discover_kubeconfigs() -> list[Path]:
    candidates = {Path(value) for value in FIXED_KUBECONFIGS}
    for root_value in KUBECONFIG_ROOTS:
        root = Path(root_value)
        if not root.exists():
            continue
        try:
            for path in root.rglob("*"):
                if path.is_file() and path.stat().st_size <= MAX_SECRET_BYTES:
                    candidates.add(path)
        except (OSError, PermissionError):
            continue
    return sorted(path for path in candidates if path.is_file())


def kubectl_json(kubeconfig: Path, context: str, resource: str) -> Any | None:
    process = run(
        [
            "kubectl",
            "--kubeconfig",
            str(kubeconfig),
            "--context",
            context,
            "get",
            resource,
            "-A",
            "-o",
            "json",
        ],
        timeout=90,
    )
    if process.returncode != 0:
        return None
    try:
        return json.loads(process.stdout)
    except json.JSONDecodeError:
        return None


def discover_kubernetes_material(
    app_ids: dict[str, AppIdCandidate],
    keys: dict[str, KeyCandidate],
) -> tuple[dict[str, int], set[str]]:
    stats = {
        "kubeconfig_files": 0,
        "kubernetes_contexts": 0,
        "kubernetes_secrets": 0,
        "kubernetes_relevant_sources": 0,
        "external_secret_refs": 0,
    }
    external_secret_names: set[str] = set()
    seen_contexts: set[tuple[str, str]] = set()

    for kubeconfig in discover_kubeconfigs():
        contexts_process = run(
            [
                "kubectl",
                "--kubeconfig",
                str(kubeconfig),
                "config",
                "get-contexts",
                "-o",
                "name",
            ],
            timeout=20,
        )
        if contexts_process.returncode != 0:
            continue
        contexts = [
            line.strip()
            for line in contexts_process.stdout.decode("utf-8", "replace").splitlines()
            if line.strip()
        ]
        if not contexts:
            continue
        stats["kubeconfig_files"] += 1
        for context in contexts:
            identity = (str(kubeconfig), context)
            if identity in seen_contexts:
                continue
            seen_contexts.add(identity)
            document = kubectl_json(kubeconfig, context, "secrets")
            if not isinstance(document, dict) or not isinstance(document.get("items"), list):
                continue
            stats["kubernetes_contexts"] += 1
            for item in document["items"]:
                if not isinstance(item, dict):
                    continue
                metadata = item.get("metadata")
                data = item.get("data")
                if not isinstance(metadata, dict) or not isinstance(data, dict):
                    continue
                namespace = metadata.get("namespace")
                name = metadata.get("name")
                if not isinstance(namespace, str) or not isinstance(name, str):
                    continue
                stats["kubernetes_secrets"] += 1
                decoded: dict[str, str] = {}
                for field_name, encoded in data.items():
                    if not isinstance(field_name, str) or not isinstance(encoded, str):
                        continue
                    try:
                        raw = base64.b64decode(encoded, validate=True)
                    except ValueError:
                        continue
                    if len(raw) > MAX_SECRET_BYTES:
                        continue
                    try:
                        decoded[field_name] = raw.decode("utf-8")
                    except UnicodeDecodeError:
                        continue
                source = f"kubernetes:{context}:{namespace}/{name}"
                if collect_material(decoded, source, app_ids, keys):
                    stats["kubernetes_relevant_sources"] += 1
                decoded.clear()
            document.clear()

            external_document = kubectl_json(
                kubeconfig,
                context,
                "externalsecrets.external-secrets.io",
            )
            if isinstance(external_document, dict) and isinstance(
                external_document.get("items"), list
            ):
                for item in external_document["items"]:
                    if not isinstance(item, dict):
                        continue
                    spec = item.get("spec")
                    if not isinstance(spec, dict):
                        continue
                    for entry in spec.get("data", []) if isinstance(spec.get("data"), list) else []:
                        if not isinstance(entry, dict):
                            continue
                        remote = entry.get("remoteRef")
                        if isinstance(remote, dict) and isinstance(remote.get("key"), str):
                            external_secret_names.add(remote["key"])
                    data_from = spec.get("dataFrom")
                    if isinstance(data_from, list):
                        for entry in data_from:
                            if not isinstance(entry, dict):
                                continue
                            extract = entry.get("extract")
                            if isinstance(extract, dict) and isinstance(extract.get("key"), str):
                                external_secret_names.add(extract["key"])
    stats["external_secret_refs"] = len(external_secret_names)
    return stats, external_secret_names


def aws_json(command: list[str], timeout: int = 90) -> Any | None:
    process = run(command, timeout=timeout)
    if process.returncode != 0:
        return None
    try:
        return json.loads(process.stdout)
    except json.JSONDecodeError:
        return None


def discover_aws_secret_material(
    region: str,
    explicit_names: set[str],
    app_ids: dict[str, AppIdCandidate],
    keys: dict[str, KeyCandidate],
) -> dict[str, int]:
    stats = {
        "aws_secret_names": 0,
        "aws_secrets_read": 0,
        "aws_relevant_sources": 0,
    }
    document = aws_json(
        [
            "aws",
            "secretsmanager",
            "list-secrets",
            "--region",
            region,
            "--max-results",
            "100",
            "--output",
            "json",
        ],
        timeout=120,
    )
    names = set(explicit_names)
    if isinstance(document, dict) and isinstance(document.get("SecretList"), list):
        for item in document["SecretList"]:
            if isinstance(item, dict) and isinstance(item.get("Name"), str):
                names.add(item["Name"])
    selected_names = sorted(names)[:MAX_AWS_SECRETS]
    stats["aws_secret_names"] = len(selected_names)

    for name in selected_names:
        response = aws_json(
            [
                "aws",
                "secretsmanager",
                "get-secret-value",
                "--region",
                region,
                "--secret-id",
                name,
                "--output",
                "json",
            ],
            timeout=45,
        )
        if not isinstance(response, dict):
            continue
        raw: str | None = None
        if isinstance(response.get("SecretString"), str):
            raw = response["SecretString"]
        elif isinstance(response.get("SecretBinary"), str):
            try:
                decoded = base64.b64decode(response["SecretBinary"], validate=True)
                raw = decoded.decode("utf-8") if len(decoded) <= MAX_SECRET_BYTES else None
            except (ValueError, UnicodeDecodeError):
                raw = None
        if raw is None or len(raw.encode("utf-8")) > MAX_SECRET_BYTES:
            continue
        stats["aws_secrets_read"] += 1
        payload: Any
        try:
            payload = json.loads(raw)
        except json.JSONDecodeError:
            payload = raw
        if collect_material(payload, f"aws-secrets-manager:{name}", app_ids, keys):
            stats["aws_relevant_sources"] += 1
        raw = ""
        if isinstance(payload, dict):
            payload.clear()
    return stats


def discover_ssm_material(
    region: str,
    app_ids: dict[str, AppIdCandidate],
    keys: dict[str, KeyCandidate],
) -> dict[str, int]:
    stats = {
        "ssm_parameter_names": 0,
        "ssm_parameters_read": 0,
        "ssm_relevant_sources": 0,
    }
    document = aws_json(
        [
            "aws",
            "ssm",
            "describe-parameters",
            "--region",
            region,
            "--max-results",
            "50",
            "--output",
            "json",
        ],
        timeout=120,
    )
    names: list[str] = []
    if isinstance(document, dict) and isinstance(document.get("Parameters"), list):
        names = sorted(
            item["Name"]
            for item in document["Parameters"]
            if isinstance(item, dict) and isinstance(item.get("Name"), str)
        )[:MAX_SSM_PARAMETERS]
    stats["ssm_parameter_names"] = len(names)

    for name in names:
        response = aws_json(
            [
                "aws",
                "ssm",
                "get-parameter",
                "--region",
                region,
                "--name",
                name,
                "--with-decryption",
                "--output",
                "json",
            ],
            timeout=45,
        )
        if not isinstance(response, dict):
            continue
        parameter = response.get("Parameter")
        if not isinstance(parameter, dict) or not isinstance(parameter.get("Value"), str):
            continue
        raw = parameter["Value"]
        if len(raw.encode("utf-8")) > MAX_SECRET_BYTES:
            continue
        stats["ssm_parameters_read"] += 1
        try:
            payload: Any = json.loads(raw)
        except json.JSONDecodeError:
            payload = raw
        if collect_material(payload, f"aws-ssm-parameter:{name}", app_ids, keys):
            stats["ssm_relevant_sources"] += 1
        raw = ""
        if isinstance(payload, dict):
            payload.clear()
    return stats


def base64url(value: bytes) -> str:
    return base64.urlsafe_b64encode(value).decode("ascii").rstrip("=")


def mint_app_jwt(app_id: str, private_key: str, directory: Path) -> str:
    key_path = directory / f"key-{hashlib.sha256(private_key.encode()).hexdigest()}.pem"
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
    process = run(
        ["openssl", "dgst", "-sha256", "-sign", str(key_path)],
        timeout=20,
        input_bytes=unsigned.encode("ascii"),
    )
    if process.returncode != 0 or not process.stdout:
        raise ValueError("candidate private key could not sign an App JWT")
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
            "User-Agent": "hypesiege-protected-app-selector",
            **({"Content-Type": "application/json"} if encoded is not None else {}),
        },
    )
    try:
        with urllib.request.urlopen(request, timeout=30) as response:
            raw = response.read(MAX_SECRET_BYTES)
            return response.status, json.loads(raw) if raw else None
    except urllib.error.HTTPError as error:
        error.read(8192)
        return error.code, None
    except (urllib.error.URLError, TimeoutError, json.JSONDecodeError):
        return None, None


def validate_pair(
    app_id: AppIdCandidate,
    private_key: KeyCandidate,
    organization: str,
    directory: Path,
) -> ValidatedPair | None:
    try:
        app_jwt = mint_app_jwt(app_id.value, private_key.value, directory)
    except ValueError:
        return None
    status, installation = request_json("GET", f"/orgs/{organization}/installation", app_jwt)
    if status != 200 or not isinstance(installation, dict):
        return None
    if installation.get("repository_selection") != "all":
        return None
    installation_id = installation.get("id")
    app_slug = installation.get("app_slug")
    if not isinstance(installation_id, int) or installation_id <= 0 or not isinstance(app_slug, str):
        return None
    status, token_document = request_json(
        "POST", f"/app/installations/{installation_id}/access_tokens", app_jwt, {}
    )
    if status != 201 or not isinstance(token_document, dict):
        return None
    token = token_document.get("token")
    permissions = token_document.get("permissions")
    if not isinstance(token, str) or not isinstance(permissions, dict):
        return None
    observed = {
        name: str(permissions.get(name, "none"))
        for name in ("administration", "contents", "pull_requests", "metadata")
    }
    request_json("DELETE", "/installation/token", token)
    required = {
        "administration": "write",
        "contents": "write",
        "pull_requests": "write",
        "metadata": "read",
    }
    if observed != required:
        return None
    return ValidatedPair(app_id, private_key, app_slug, installation_id, observed)


def write_private(path: Path, value: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(value, encoding="utf-8")
    path.chmod(0o600)


def self_test() -> None:
    app_ids: dict[str, AppIdCandidate] = {}
    keys: dict[str, KeyCandidate] = {}
    fixture = {
        "github_app_id": "12345",
        "github_app_private_key": "-----BEGIN PRIVATE KEY-----\\nabc\\n-----END PRIVATE KEY-----",
        "github_token": "ignored",
    }
    assert collect_material(fixture, "self-test", app_ids, keys)
    assert set(app_ids) == {"12345"}
    assert len(keys) == 1
    assert not app_id_shaped("github_token", "self-test")
    print("protected App selector self-test: ok")


def main() -> int:
    args = parse_args()
    if args.self_test:
        self_test()
        return 0

    app_ids: dict[str, AppIdCandidate] = {}
    keys: dict[str, KeyCandidate] = {}
    kubernetes_stats, external_secret_names = discover_kubernetes_material(app_ids, keys)
    aws_stats = discover_aws_secret_material(
        args.region, external_secret_names, app_ids, keys
    )
    ssm_stats = discover_ssm_material(args.region, app_ids, keys)
    stats = {**kubernetes_stats, **aws_stats, **ssm_stats}

    if not app_ids or not keys:
        raise SystemExit(
            "no GitHub App credential material found "
            f"app_ids={len(app_ids)} private_keys={len(keys)} stats={json.dumps(stats, sort_keys=True)}"
        )

    validated: dict[tuple[str, str], ValidatedPair] = {}
    with tempfile.TemporaryDirectory(prefix="hsg-protected-app-selector-") as temporary:
        directory = Path(temporary)
        for app_id in sorted(app_ids.values(), key=lambda item: int(item.value)):
            for private_key in sorted(keys.values(), key=lambda item: item.fingerprint):
                selected = validate_pair(app_id, private_key, args.organization, directory)
                if selected is not None:
                    validated[(app_id.value, private_key.fingerprint)] = selected

    if len(validated) != 1:
        raise SystemExit(
            "expected exactly one all-repositories repository-admin App "
            f"for {args.organization}; found {len(validated)} from "
            f"{len(app_ids)} App IDs and {len(keys)} private keys"
        )

    selected = next(iter(validated.values()))
    write_private(args.app_id_out, selected.app_id.value + "\n")
    write_private(args.private_key_out, selected.private_key.value)
    evidence = {
        "schema_version": 1,
        "organization": args.organization,
        "app_slug": selected.app_slug,
        "installation_id": selected.installation_id,
        "repository_selection": "all",
        "permissions": selected.permissions,
        "app_id_sources": sorted(selected.app_id.sources),
        "private_key_sources": sorted(selected.private_key.sources),
        "private_key_sha256": selected.private_key.fingerprint,
        "discovery": stats,
        "pat_used": False,
    }
    args.evidence_out.parent.mkdir(parents=True, exist_ok=True)
    args.evidence_out.write_text(
        json.dumps(evidence, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    print(
        f"validated protected GitHub App app={selected.app_slug} "
        f"installation={selected.installation_id}"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
