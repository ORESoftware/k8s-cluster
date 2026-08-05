#!/usr/bin/env python3
"""Select a read-only GitHub App for private k8s submodule checkout.

This program runs only on the protected administration host. It reuses the
repository's protected-source scanner to inspect readable Kubernetes Secret
objects, ExternalSecret references, AWS Secrets Manager, and encrypted SSM
Parameter Store values. Candidate values are never printed.

A credential pair is accepted only when GitHub proves all of the following:

* the GitHub App's configured repository permissions are read-only;
* the App is installed for the requested private repository;
* the minted installation token is restricted to exactly that repository;
* the token grants ``contents:read`` and no write permission; and
* the validation token is revoked before the process exits.

The selected App ID and private key are written only to caller-provided
mode-0600 files. Evidence contains identifiers, source names, counts, and a
private-key fingerprint, never credential values.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import sys
import tempfile
from dataclasses import dataclass
from pathlib import Path
from typing import Any

import select_hypesiege_github_app_from_protected_sources as protected

MAX_PAIR_ATTEMPTS = 1024


@dataclass(frozen=True)
class ValidatedPair:
    app_id: protected.AppIdCandidate
    private_key: protected.KeyCandidate
    app_slug: str
    installation_id: int
    app_permissions: dict[str, str]
    token_permissions: dict[str, str]
    score: int


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--region", default=os.environ.get("AWS_REGION", "us-east-1"))
    parser.add_argument("--target-repository")
    parser.add_argument("--app-id-out", type=Path)
    parser.add_argument("--private-key-out", type=Path)
    parser.add_argument("--evidence-out", type=Path)
    parser.add_argument("--self-test", action="store_true")
    return parser.parse_args()


def normalize(value: str) -> str:
    return re.sub(r"[^a-z0-9]", "", value.casefold())


def normalize_permissions(value: Any) -> dict[str, str] | None:
    if not isinstance(value, dict):
        return None
    normalized: dict[str, str] = {}
    for name, permission in value.items():
        if not isinstance(name, str) or not isinstance(permission, str):
            return None
        normalized[name] = permission
    return normalized


def permission_set_is_read_only(permissions: dict[str, str]) -> bool:
    return (
        permissions.get("contents") == "read"
        and all(permission == "read" for permission in permissions.values())
    )


def validate_pair(
    app_id: protected.AppIdCandidate,
    private_key: protected.KeyCandidate,
    target_owner: str,
    target_repo: str,
    directory: Path,
) -> ValidatedPair | None:
    try:
        app_jwt = protected.mint_app_jwt(
            app_id.value,
            private_key.value,
            directory,
        )
    except ValueError:
        return None

    status, app_document = protected.request_json("GET", "/app", app_jwt)
    if status != 200 or not isinstance(app_document, dict):
        return None
    app_slug = app_document.get("slug")
    app_permissions = normalize_permissions(app_document.get("permissions"))
    if (
        not isinstance(app_slug, str)
        or app_permissions is None
        or not permission_set_is_read_only(app_permissions)
    ):
        return None

    status, installation = protected.request_json(
        "GET",
        f"/repos/{target_owner}/{target_repo}/installation",
        app_jwt,
    )
    if status != 200 or not isinstance(installation, dict):
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

    status, token_document = protected.request_json(
        "POST",
        f"/app/installations/{installation_id}/access_tokens",
        app_jwt,
        {
            "repositories": [target_repo],
            "permissions": {"contents": "read"},
        },
    )
    if status != 201 or not isinstance(token_document, dict):
        return None
    token = token_document.get("token")
    token_permissions = normalize_permissions(token_document.get("permissions"))
    if (
        not isinstance(token, str)
        or not token
        or token_permissions is None
        or not permission_set_is_read_only(token_permissions)
    ):
        return None

    try:
        status, repo_document = protected.request_json(
            "GET",
            f"/repos/{target_owner}/{target_repo}",
            token,
        )
        if status != 200 or not isinstance(repo_document, dict):
            return None

        status, repositories = protected.request_json(
            "GET",
            "/installation/repositories?per_page=100",
            token,
        )
        if status != 200 or not isinstance(repositories, dict):
            return None
        full_names = sorted(
            str(item.get("full_name"))
            for item in repositories.get("repositories", [])
            if isinstance(item, dict)
        )
        if repositories.get("total_count") != 1:
            return None
        if full_names != [f"{target_owner}/{target_repo}"]:
            return None
    finally:
        protected.request_json("DELETE", "/installation/token", token)

    sources = app_id.sources | private_key.sources
    shared_sources = app_id.sources & private_key.sources
    source_text = " ".join(sorted(sources))
    score = 0
    if shared_sources:
        score += 40
    if "submodule" in app_slug.casefold():
        score += 120
    if "k8s" in app_slug.casefold():
        score += 20
    if "submodule" in source_text.casefold():
        score += 100
    if "k8ssubmodule" in normalize(source_text):
        score += 20

    return ValidatedPair(
        app_id=app_id,
        private_key=private_key,
        app_slug=app_slug,
        installation_id=installation_id,
        app_permissions=app_permissions,
        token_permissions=token_permissions,
        score=score,
    )


def write_private(path: Path, value: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(value, encoding="utf-8")
    path.chmod(0o600)


def candidate_pair_priority(
    app_id: protected.AppIdCandidate,
    private_key: protected.KeyCandidate,
) -> tuple[int, int, int, str, str]:
    sources = app_id.sources | private_key.sources
    source_text = " ".join(sorted(sources)).casefold()
    return (
        0 if "submodule" in source_text else 1,
        0 if app_id.sources & private_key.sources else 1,
        int(app_id.value),
        app_id.value,
        private_key.fingerprint,
    )


def self_test() -> None:
    assert permission_set_is_read_only({"contents": "read", "metadata": "read"})
    assert not permission_set_is_read_only({"contents": "write", "metadata": "read"})
    assert not permission_set_is_read_only({"metadata": "read"})
    assert normalize("k8s-submodule GitHub App") == "k8ssubmodulegithubapp"
    print("k8s submodule App validator self-test: ok")


def main() -> int:
    args = parse_args()
    if args.self_test:
        self_test()
        return 0

    required = {
        "--target-repository": args.target_repository,
        "--app-id-out": args.app_id_out,
        "--private-key-out": args.private_key_out,
        "--evidence-out": args.evidence_out,
    }
    missing = [name for name, value in required.items() if value is None]
    if missing:
        raise SystemExit(f"missing required arguments: {', '.join(missing)}")

    assert isinstance(args.target_repository, str)
    assert isinstance(args.app_id_out, Path)
    assert isinstance(args.private_key_out, Path)
    assert isinstance(args.evidence_out, Path)

    if "/" not in args.target_repository:
        raise SystemExit("--target-repository must use owner/name form")
    target_owner, target_repo = args.target_repository.split("/", 1)
    if not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9-]{0,38}", target_owner):
        raise SystemExit("invalid target repository owner")
    if not re.fullmatch(r"[A-Za-z0-9_.-]{1,100}", target_repo):
        raise SystemExit("invalid target repository name")

    app_ids: dict[str, protected.AppIdCandidate] = {}
    private_keys: dict[str, protected.KeyCandidate] = {}
    kubernetes_stats, external_secret_names = protected.discover_kubernetes_material(
        app_ids,
        private_keys,
    )
    aws_stats = protected.discover_aws_secret_material(
        args.region,
        set(external_secret_names) | {
            "dd/remote-dev/agent-secrets",
            "dd/remote-dev/k8s-submodule-github-app",
            "dd/remote-dev/github-app",
            "dd/remote-dev/github-app-secrets",
            "k8s-submodule-github-app",
        },
        app_ids,
        private_keys,
    )
    ssm_stats = protected.discover_ssm_material(
        args.region,
        app_ids,
        private_keys,
    )
    discovery = {**kubernetes_stats, **aws_stats, **ssm_stats}
    candidate_sources = sorted(
        set().union(
            *(candidate.sources for candidate in app_ids.values()),
            *(candidate.sources for candidate in private_keys.values()),
        )
    ) if app_ids or private_keys else []

    if not app_ids or not private_keys:
        raise SystemExit(
            "no GitHub App credential material found "
            f"app_ids={len(app_ids)} private_keys={len(private_keys)} "
            f"candidate_sources={json.dumps(candidate_sources)} "
            f"discovery={json.dumps(discovery, sort_keys=True)}"
        )

    candidate_pairs = sorted(
        (
            (app_id, private_key)
            for app_id in app_ids.values()
            for private_key in private_keys.values()
        ),
        key=lambda pair: candidate_pair_priority(pair[0], pair[1]),
    )
    if len(candidate_pairs) > MAX_PAIR_ATTEMPTS:
        raise SystemExit(
            f"refusing {len(candidate_pairs)} candidate pairs; "
            f"maximum is {MAX_PAIR_ATTEMPTS}"
        )

    args.evidence_out.parent.mkdir(parents=True, exist_ok=True)
    validated: list[ValidatedPair] = []
    with tempfile.TemporaryDirectory(
        prefix="k8s-submodule-app-validator-",
        dir=str(args.evidence_out.parent),
    ) as temporary:
        directory = Path(temporary)
        for app_id, private_key in candidate_pairs:
            pair = validate_pair(
                app_id,
                private_key,
                target_owner,
                target_repo,
                directory,
            )
            if pair is not None:
                validated.append(pair)

    if not validated:
        raise SystemExit(
            "no globally read-only, single-repository GitHub App pair validated "
            f"from app_ids={len(app_ids)} private_keys={len(private_keys)} "
            f"candidate_sources={json.dumps(candidate_sources)}"
        )

    validated.sort(
        key=lambda item: (
            -item.score,
            int(item.app_id.value),
            item.private_key.fingerprint,
        )
    )
    selected = validated[0]
    if (
        len(validated) > 1
        and validated[1].score == selected.score
        and (
            validated[1].app_id.value != selected.app_id.value
            or validated[1].private_key.fingerprint
            != selected.private_key.fingerprint
        )
    ):
        raise SystemExit(
            "multiple equally preferred read-only Apps validated; "
            "refusing ambiguous selection"
        )

    write_private(args.app_id_out, selected.app_id.value + "\n")
    write_private(args.private_key_out, selected.private_key.value)
    evidence = {
        "schema_version": 1,
        "target_repository": f"{target_owner}/{target_repo}",
        "app_slug": selected.app_slug,
        "installation_id": selected.installation_id,
        "app_permissions": selected.app_permissions,
        "token_permissions": selected.token_permissions,
        "permissions": selected.token_permissions,
        "repository_restriction_verified": True,
        "candidate_counts": {
            "app_ids": len(app_ids),
            "private_keys": len(private_keys),
            "validated_pairs": len(validated),
        },
        "credential_sources": {
            "app_id": sorted(selected.app_id.sources),
            "private_key": sorted(selected.private_key.sources),
            "private_key_sha256": selected.private_key.fingerprint,
        },
        "discovery": discovery,
        "pat_used_for_submodule_access": False,
    }
    write_private(
        args.evidence_out,
        json.dumps(evidence, indent=2, sort_keys=True) + "\n",
    )
    print(
        "validated a read-only repository-restricted GitHub App "
        f"app={selected.app_slug} installation={selected.installation_id}"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
