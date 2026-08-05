#!/usr/bin/env python3
"""Run the fleet .github policy publisher with repository Actions App secrets.

This adapter never accepts a PAT or OAuth token. It validates one GitHub App
credential pair from the trusted repository secret context, requires an
all-repositories installation for every production organization, and limits
installation tokens to repository administration, contents, and metadata.
The underlying publisher remains responsible for fail-closed fleet mapping,
managed-block preservation, repository creation, writes, and read-after-write
verification.
"""

from __future__ import annotations

import argparse
import hashlib
import os
from pathlib import Path
import re
import sys
import tempfile
from typing import Any

SCRIPT_DIR = Path(__file__).resolve().parent
if str(SCRIPT_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIR))

import publish_org_dotgithub_policy_with_protected_apps as publisher

APP_ID_ENV = "K8S_SUBMODULE_APP_ID"
PRIVATE_KEY_ENV = "K8S_SUBMODULE_APP_PRIVATE_KEY"
PRIVATE_KEY_PATTERN = re.compile(
    r"^-----BEGIN (?:RSA )?PRIVATE KEY-----\n.+\n-----END (?:RSA )?PRIVATE KEY-----\s*$",
    re.DOTALL,
)
REQUIRED_TOKEN_PERMISSIONS = {
    "administration": "write",
    "contents": "write",
    "metadata": "read",
}


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--report", type=Path, required=True)
    parser.add_argument("--self-test", action="store_true")
    return parser.parse_args()


def expand_literal_newlines(value: str) -> str:
    if "\\n" in value and "\n" not in value.strip():
        return value.replace("\\n", "\n")
    return value


def normalize_private_key(value: str) -> str:
    normalized = expand_literal_newlines(value).strip() + "\n"
    if PRIVATE_KEY_PATTERN.fullmatch(normalized) is None:
        raise RuntimeError("repository Actions App private key is not a PEM private key")
    if len(normalized.encode("utf-8")) > 1_048_576:
        raise RuntimeError("repository Actions App private key is too large")
    return normalized


def load_repository_app() -> publisher.AppCredential:
    app_id = os.environ.pop(APP_ID_ENV, "").strip()
    raw_private_key = os.environ.pop(PRIVATE_KEY_ENV, "")
    if not app_id.isdigit() or int(app_id) <= 0 or len(app_id) > 32:
        raise RuntimeError("repository Actions App ID is missing or invalid")
    if not raw_private_key:
        raise RuntimeError("repository Actions App private key is missing")
    private_key = normalize_private_key(raw_private_key)
    fingerprint = hashlib.sha256(private_key.encode("utf-8")).hexdigest()

    with tempfile.TemporaryDirectory(prefix="org-dotgithub-repository-app-validation-") as temporary:
        directory = Path(temporary)
        try:
            app_jwt = publisher.protected_selector.mint_app_jwt(
                app_id, private_key, directory
            )
        except ValueError as error:
            raise RuntimeError("repository Actions App credential could not sign a JWT") from error
        status, document = publisher.api_request("GET", "/app", app_jwt)

    if status != 200 or not isinstance(document, dict):
        raise RuntimeError(f"repository Actions App identity validation failed: HTTP {status}")
    if document.get("id") != int(app_id):
        raise RuntimeError("repository Actions App ID does not match the validated App")
    app_slug = document.get("slug")
    if not isinstance(app_slug, str) or not app_slug:
        raise RuntimeError("validated repository Actions App has no slug")

    return publisher.AppCredential(
        app_id=app_id,
        private_key=private_key,
        private_key_fingerprint=fingerprint,
        app_slug=app_slug,
    )


def direct_discovery(app: publisher.AppCredential) -> tuple[list[publisher.AppCredential], dict[str, Any]]:
    return [app], {
        "credential_source": "repository_actions_secrets",
        "validated_apps": 1,
    }


def mint_least_privilege_installation_token(
    installation: publisher.OrganizationInstallation, directory: Path
) -> str:
    app_jwt = publisher.app_jwt(installation.app, directory)
    status, document = publisher.api_request(
        "POST",
        f"/app/installations/{installation.installation_id}/access_tokens",
        app_jwt,
        {"permissions": REQUIRED_TOKEN_PERMISSIONS},
    )
    if status != 201 or not isinstance(document, dict):
        raise RuntimeError(
            f"could not mint token for {installation.organization}: HTTP {status}"
        )
    token = document.get("token")
    permissions = document.get("permissions")
    if not isinstance(token, str) or not token or not isinstance(permissions, dict):
        raise RuntimeError(f"invalid token document for {installation.organization}")
    observed = {
        name: str(permissions.get(name, "none"))
        for name in REQUIRED_TOKEN_PERMISSIONS
    }
    if observed != REQUIRED_TOKEN_PERMISSIONS:
        publisher.api_request("DELETE", "/installation/token", token)
        raise RuntimeError(
            f"insufficient token permissions for {installation.organization}: {observed}"
        )
    return token


def self_test() -> None:
    assert len(publisher.PRODUCTION_ORGANIZATIONS) == 45
    assert all(
        not organization.casefold().endswith("-test")
        for organization in publisher.PRODUCTION_ORGANIZATIONS
    )
    assert REQUIRED_TOKEN_PERMISSIONS == {
        "administration": "write",
        "contents": "write",
        "metadata": "read",
    }
    assert expand_literal_newlines("alpha\\nbeta") == "alpha\nbeta"
    assert expand_literal_newlines("alpha\nbeta") == "alpha\nbeta"
    assert "greater than 99.1%" in publisher.POLICY_BLOCK
    assert "greater than 99.7%" in publisher.POLICY_BLOCK
    assert "`dev` is the integration branch" in publisher.POLICY_BLOCK
    assert "*-infra" in publisher.POLICY_BLOCK
    print("repository-App fleet policy adapter self-test: ok")


def main() -> int:
    args = parse_args()
    if args.self_test:
        self_test()
        return 0

    app = load_repository_app()
    publisher.REQUIRED_TOKEN_PERMISSIONS = REQUIRED_TOKEN_PERMISSIONS
    publisher.discover_app_credentials = lambda _region: direct_discovery(app)
    publisher.mint_installation_token = mint_least_privilege_installation_token
    sys.argv = [
        str(SCRIPT_DIR / "publish_org_dotgithub_policy_with_protected_apps.py"),
        "--report",
        str(args.report),
    ]
    return publisher.main()


if __name__ == "__main__":
    raise SystemExit(main())
