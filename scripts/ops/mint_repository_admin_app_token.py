#!/usr/bin/env python3
"""Mint a short-lived repository-admin GitHub App installation token.

The helper delegates protected credential discovery and live installation
validation to ``select_hypesiege_github_app_from_protected_sources.py``. Despite
that selector's historical filename, it accepts any organization. This wrapper
never prints the private key or token and writes both only to mode-0600 files in
a caller-owned temporary directory.
"""

from __future__ import annotations

import argparse
import importlib.util
import json
import os
import pathlib
import subprocess
import sys
import tempfile
from types import ModuleType
from typing import Any


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--selector", type=pathlib.Path, required=True)
    parser.add_argument("--organization", required=True)
    parser.add_argument("--token-out", type=pathlib.Path, required=True)
    parser.add_argument("--evidence-out", type=pathlib.Path, required=True)
    parser.add_argument("--region", default=os.environ.get("AWS_REGION", "us-east-1"))
    return parser.parse_args()


def load_selector(path: pathlib.Path) -> ModuleType:
    spec = importlib.util.spec_from_file_location("repository_admin_app_selector", path)
    if spec is None or spec.loader is None:
        raise SystemExit(f"unable to load selector from {path}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def write_private(path: pathlib.Path, value: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(value, encoding="utf-8")
    path.chmod(0o600)


def main() -> int:
    args = parse_args()
    selector = args.selector.resolve()
    if not selector.is_file():
        raise SystemExit(f"selector does not exist: {selector}")

    with tempfile.TemporaryDirectory(prefix="repository-admin-app-token-") as temporary:
        work = pathlib.Path(temporary)
        app_id_path = work / "app-id"
        private_key_path = work / "private-key.pem"
        selector_evidence_path = work / "selector-evidence.json"

        subprocess.run(
            [
                sys.executable,
                str(selector),
                "--organization",
                args.organization,
                "--app-id-out",
                str(app_id_path),
                "--private-key-out",
                str(private_key_path),
                "--evidence-out",
                str(selector_evidence_path),
                "--region",
                args.region,
            ],
            check=True,
            text=True,
            env=os.environ.copy(),
        )

        module = load_selector(selector)
        selector_evidence: dict[str, Any] = json.loads(
            selector_evidence_path.read_text(encoding="utf-8")
        )
        app_id = app_id_path.read_text(encoding="utf-8").strip()
        private_key = private_key_path.read_text(encoding="utf-8")
        installation_id = selector_evidence.get("installation_id")
        if not isinstance(installation_id, int) or installation_id <= 0:
            raise SystemExit("selector evidence has no valid installation_id")

        app_jwt = module.mint_app_jwt(app_id, private_key, work)
        status, token_document = module.request_json(
            "POST",
            f"/app/installations/{installation_id}/access_tokens",
            app_jwt,
            {},
        )
        if status != 201 or not isinstance(token_document, dict):
            raise SystemExit(f"failed to mint installation token: HTTP {status}")
        token = token_document.get("token")
        permissions = token_document.get("permissions")
        expires_at = token_document.get("expires_at")
        if not isinstance(token, str) or len(token) < 20 or any(ch.isspace() for ch in token):
            raise SystemExit("GitHub returned an invalid installation token")
        if not isinstance(permissions, dict):
            raise SystemExit("GitHub returned no installation permissions")

        required = {
            "administration": "write",
            "contents": "write",
            "pull_requests": "write",
            "metadata": "read",
        }
        observed = {name: str(permissions.get(name, "none")) for name in required}
        if observed != required:
            module.request_json("DELETE", "/installation/token", token)
            raise SystemExit(
                "installation token lacks required permissions: "
                + json.dumps(observed, sort_keys=True)
            )

        write_private(args.token_out, token + "\n")
        evidence = {
            **selector_evidence,
            "token_permissions": observed,
            "token_expires_at": expires_at,
            "token_written": True,
        }
        args.evidence_out.parent.mkdir(parents=True, exist_ok=True)
        args.evidence_out.write_text(
            json.dumps(evidence, indent=2, sort_keys=True) + "\n",
            encoding="utf-8",
        )
        print(
            "minted repository-admin installation token "
            f"app={selector_evidence.get('app_slug')} installation={installation_id}"
        )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
