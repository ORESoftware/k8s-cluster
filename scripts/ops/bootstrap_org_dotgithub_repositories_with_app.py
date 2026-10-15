#!/usr/bin/env python3
"""Publish the approved organization `.github` fleet with a GitHub App.

The publisher deliberately avoids personal access tokens. It accepts one GitHub
App ID/private-key pair from the trusted workflow, proves that the App has an
`all repositories` installation with repository-administration capability in
every approved organization, and only then begins the idempotent reconcile.
Each organization uses its own short-lived installation token.
"""

from __future__ import annotations

import argparse
from dataclasses import dataclass
from datetime import datetime, timezone
import json
import os
from pathlib import Path
import re
import sys
import tempfile
from typing import Any, Callable

SCRIPT_DIR = Path(__file__).resolve().parent
if str(SCRIPT_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIR))

import bootstrap_org_dotgithub_repositories as base  # noqa: E402
import bootstrap_org_dotgithub_repositories_all as fleet  # noqa: E402
from select_hypesiege_github_app_from_aws import (  # noqa: E402
    mint_app_jwt,
    request_json,
)

APP_ID_ENV = "K8S_SUBMODULE_APP_ID"
APP_PRIVATE_KEY_ENV = "K8S_SUBMODULE_APP_PRIVATE_KEY"
PEM_PATTERN = re.compile(
    r"^-----BEGIN (?:RSA )?PRIVATE KEY-----\n.+\n-----END (?:RSA )?PRIVATE KEY-----\s*$",
    re.DOTALL,
)
REQUIRED_PERMISSIONS: dict[str, str] = {
    "administration": "write",
    "contents": "write",
    "metadata": "read",
}


@dataclass
class PreparedInstallation:
    organization: str
    app_slug: str
    token: str
    api: base.GitHubApi
    existing_repository: dict[str, Any] | None


class AppPublisherError(RuntimeError):
    """A bounded App capability or publication invariant failed."""


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--execute", action="store_true", help="perform GitHub mutations")
    parser.add_argument("--json-report", type=Path)
    parser.add_argument("--markdown-report", type=Path)
    return parser.parse_args()


def read_app_credentials() -> tuple[str, str]:
    """Read then remove the App credential from the inherited environment."""
    app_id = os.environ.pop(APP_ID_ENV, "").strip()
    private_key = os.environ.pop(APP_PRIVATE_KEY_ENV, "")

    # Explicitly remove token-shaped ambient variables so the publisher cannot
    # accidentally fall back to a user credential through a future dependency.
    for variable in (
        "GH_TOKEN",
        "GITHUB_TOKEN",
        "GITHUB_REPOSITORY_ADMIN_TOKEN",
        "GH_ENTERPRISE_TOKEN",
    ):
        os.environ.pop(variable, None)

    if not app_id.isdigit() or int(app_id) <= 0:
        raise AppPublisherError(f"{APP_ID_ENV} must be a positive integer")
    canonical_key = private_key.replace("\r\n", "\n").strip() + "\n"
    if not PEM_PATTERN.fullmatch(canonical_key):
        raise AppPublisherError(f"{APP_PRIVATE_KEY_ENV} must be a supported PEM private key")
    return app_id, canonical_key


def create_app_jwt(app_id: str, private_key: str) -> str:
    with tempfile.TemporaryDirectory(prefix="org-dotgithub-app-jwt-") as directory:
        return mint_app_jwt(app_id, private_key, Path(directory))


def app_request(
    request_fn: Callable[..., tuple[int | None, Any | None]],
    method: str,
    path: str,
    bearer: str,
    body: dict[str, Any] | None = None,
) -> tuple[int | None, Any | None]:
    return request_fn(method, path, bearer, body)


def revoke_installation_token(
    token: str,
    *,
    request_fn: Callable[..., tuple[int | None, Any | None]] = request_json,
) -> None:
    if not token:
        return
    status, _ = app_request(request_fn, "DELETE", "/installation/token", token)
    if status not in {204, 401, 403}:
        print(
            f"WARNING installation-token revocation returned HTTP {status!r}",
            file=sys.stderr,
        )


def validated_installation_token(
    organization: str,
    app_jwt: str,
    *,
    request_fn: Callable[..., tuple[int | None, Any | None]] = request_json,
) -> tuple[str, str]:
    status, installation = app_request(
        request_fn,
        "GET",
        f"/orgs/{organization}/installation",
        app_jwt,
    )
    if status != 200 or not isinstance(installation, dict):
        raise AppPublisherError(
            f"{organization}: GitHub App installation lookup returned HTTP {status!r}"
        )

    account = installation.get("account")
    account_login = account.get("login") if isinstance(account, dict) else None
    if not isinstance(account_login, str) or account_login.lower() != organization.lower():
        raise AppPublisherError(
            f"{organization}: installation account mismatch {account_login!r}"
        )
    if installation.get("repository_selection") != "all":
        raise AppPublisherError(
            f"{organization}: GitHub App installation must select all repositories"
        )

    installation_id = installation.get("id")
    app_slug = installation.get("app_slug")
    if not isinstance(installation_id, int) or installation_id <= 0:
        raise AppPublisherError(f"{organization}: invalid installation identifier")
    if not isinstance(app_slug, str) or not app_slug:
        raise AppPublisherError(f"{organization}: missing GitHub App slug")

    status, document = app_request(
        request_fn,
        "POST",
        f"/app/installations/{installation_id}/access_tokens",
        app_jwt,
        {"permissions": REQUIRED_PERMISSIONS},
    )
    if status != 201 or not isinstance(document, dict):
        raise AppPublisherError(
            f"{organization}: installation-token mint returned HTTP {status!r}"
        )

    token = document.get("token")
    permissions = document.get("permissions")
    if not isinstance(token, str) or not token or any(character.isspace() for character in token):
        raise AppPublisherError(f"{organization}: installation-token response was unusable")
    if not isinstance(permissions, dict):
        revoke_installation_token(token, request_fn=request_fn)
        raise AppPublisherError(f"{organization}: installation-token permissions were absent")

    observed = {name: permissions.get(name) for name in REQUIRED_PERMISSIONS}
    if observed != REQUIRED_PERMISSIONS:
        revoke_installation_token(token, request_fn=request_fn)
        raise AppPublisherError(
            f"{organization}: insufficient App permissions {observed!r}"
        )
    return app_slug, token


def preflight_installations(
    app_jwt: str,
    *,
    request_fn: Callable[..., tuple[int | None, Any | None]] = request_json,
    api_factory: Callable[[str], base.GitHubApi] = base.GitHubApi,
    repository_getter: Callable[
        [base.GitHubApi, str], dict[str, Any] | None
    ] = base.get_repository,
    repository_validator: Callable[[dict[str, Any], str], None] = base.validate_repository,
) -> tuple[list[PreparedInstallation], str]:
    """Prove all 61 App installations before any repository mutation occurs."""
    prepared: list[PreparedInstallation] = []
    app_slug: str | None = None
    try:
        for organization in fleet.TARGET_ORGANIZATIONS:
            current_token = ""
            try:
                observed_slug, current_token = validated_installation_token(
                    organization,
                    app_jwt,
                    request_fn=request_fn,
                )
                if app_slug is None:
                    app_slug = observed_slug
                elif observed_slug != app_slug:
                    raise AppPublisherError(
                        f"{organization}: App slug {observed_slug!r} differs from {app_slug!r}"
                    )

                api = api_factory(current_token)
                repository = repository_getter(api, organization)
                if repository is not None:
                    repository_validator(repository, organization)
                prepared.append(
                    PreparedInstallation(
                        organization=organization,
                        app_slug=observed_slug,
                        token=current_token,
                        api=api,
                        existing_repository=repository,
                    )
                )
                current_token = ""
                print(
                    f"APP-PREFLIGHT {organization} "
                    f"repository={'present' if repository else 'missing'}"
                )
            finally:
                if current_token:
                    revoke_installation_token(current_token, request_fn=request_fn)
    except Exception:
        for item in prepared:
            revoke_installation_token(item.token, request_fn=request_fn)
            item.token = ""
        raise

    if app_slug is None or len(prepared) != 61:
        for item in prepared:
            revoke_installation_token(item.token, request_fn=request_fn)
            item.token = ""
        raise AppPublisherError("App preflight did not cover the exact 61-organization fleet")

    observed = {item.organization.lower() for item in prepared}
    expected = {name.lower() for name in fleet.TARGET_ORGANIZATIONS}
    if observed != expected or expected & fleet.EXCLUDED_ORGANIZATIONS:
        for item in prepared:
            revoke_installation_token(item.token, request_fn=request_fn)
            item.token = ""
        raise AppPublisherError("App preflight organization set failed exact-set validation")

    print(f"APP-PREFLIGHT-COMPLETE organizations={len(prepared)} app={app_slug}")
    return prepared, app_slug


def reconcile_prepared_installations(
    prepared: list[PreparedInstallation],
    *,
    execute: bool,
    reconcile: Callable[..., base.OrganizationResult] = base.reconcile_organization,
) -> list[base.OrganizationResult]:
    results: list[base.OrganizationResult] = []
    for item in prepared:
        results.append(
            reconcile(
                item.api,
                item.organization,
                item.existing_repository,
                execute=execute,
            )
        )
    return results


def verify_result_set(results: list[base.OrganizationResult], *, execute: bool) -> None:
    expected = {name.lower() for name in fleet.TARGET_ORGANIZATIONS}
    observed = {item.organization.lower() for item in results}
    if len(results) != 61 or len(observed) != 61 or observed != expected:
        raise AppPublisherError("publication result does not cover the exact 61-organization fleet")
    if execute and any(item.verified is not True for item in results):
        raise AppPublisherError("one or more organization repositories were not verified")


def write_reports(
    results: list[base.OrganizationResult],
    *,
    execute: bool,
    app_slug: str,
    json_path: Path | None,
    markdown_path: Path | None,
) -> str:
    report = {
        "schema_version": 1,
        "generated_at": datetime.now(timezone.utc).isoformat(),
        "mode": "execute" if execute else "dry-run",
        "publisher": {
            "kind": "github_app_installation_token",
            "app_slug": app_slug,
            "repository_selection": "all",
            "permissions": REQUIRED_PERMISSIONS,
        },
        "preflight_organizations": 61,
        "repository_name": base.REPOSITORY_NAME,
        "managed_paths": list(base.MANAGED_PATHS),
        "organizations": [item.as_dict() for item in results],
    }
    if json_path:
        json_path.parent.mkdir(parents=True, exist_ok=True)
        json_path.write_text(json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8")

    rendered = base.markdown_report(results, execute=execute)
    rendered += (
        "\n## Publisher identity\n\n"
        f"- GitHub App: `{app_slug}`\n"
        "- Credential: one short-lived installation token per organization\n"
        "- Repository selection preflight: `all` for every organization\n"
        "- Personal access token used: **no**\n"
    )
    if markdown_path:
        markdown_path.parent.mkdir(parents=True, exist_ok=True)
        markdown_path.write_text(rendered, encoding="utf-8")
    return rendered


def main() -> int:
    args = parse_args()
    fleet.validate_registry()
    fleet.install_all_organization_hardening()

    app_id, private_key = read_app_credentials()
    app_jwt = create_app_jwt(app_id, private_key)
    private_key = ""
    app_id = ""

    prepared: list[PreparedInstallation] = []
    try:
        prepared, app_slug = preflight_installations(app_jwt)
        app_jwt = ""
        results = reconcile_prepared_installations(prepared, execute=args.execute)
        verify_result_set(results, execute=args.execute)
        rendered = write_reports(
            results,
            execute=args.execute,
            app_slug=app_slug,
            json_path=args.json_report,
            markdown_path=args.markdown_report,
        )
        print(rendered)
        return 0
    finally:
        app_jwt = ""
        for item in prepared:
            revoke_installation_token(item.token)
            item.token = ""


if __name__ == "__main__":
    raise SystemExit(main())
