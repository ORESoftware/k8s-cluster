#!/usr/bin/env python3
"""Run the Canonical preflight with Cloudflare account-owned token semantics.

Cloudflare's 2026 account-owned `cfat_` tokens are verified through
`GET /accounts/{account_id}/tokens/verify`; the user-token endpoint is a
separate credential family. This adapter keeps the existing no-write inventory
surface intact while selecting the correct reviewed endpoint.
"""
from __future__ import annotations

import importlib.util
import sys
from pathlib import Path
from typing import Any

CORE_PATH = Path(__file__).with_name("canonical-control-plane-preflight.py")
CORE = sys.modules.get("canonical_control_plane_preflight")
if CORE is None:
    SPEC = importlib.util.spec_from_file_location(
        "canonical_control_plane_preflight_core", CORE_PATH
    )
    if SPEC is None or SPEC.loader is None:
        raise RuntimeError("failed to load Canonical control-plane preflight core")
    CORE = importlib.util.module_from_spec(SPEC)
    SPEC.loader.exec_module(CORE)


class AccountTokenCloudflareClient(CORE.CloudflareClient):
    """GET-only client that redirects only the token-verification endpoint."""

    @property
    def account_token_verify_path(self) -> str:
        return f"/accounts/{self._account_id}/tokens/verify"

    def _allowed(self, path: str) -> bool:
        return path == self.account_token_verify_path or super()._allowed(path)

    def get(
        self,
        path: str,
        *,
        query: dict[str, str] | None = None,
        label: str,
        optional_statuses: tuple[int, ...] = (),
    ) -> tuple[int, Any | None]:
        if path == "/user/tokens/verify":
            path = self.account_token_verify_path
            label = "account-owned token verification"
        return super().get(
            path,
            query=query,
            label=label,
            optional_statuses=optional_statuses,
        )


_original_validate_bundle = CORE.validate_bundle


def validate_account_bundle(bundle: dict[str, Any], expected_account_hash: str) -> dict[str, str]:
    values = _original_validate_bundle(bundle, expected_account_hash)
    if not values["cloudflare_api_token"].startswith("cfat_"):
        raise CORE.PreflightError("Cloudflare credential is not an account-owned cfat_ token")
    return values


CORE.CloudflareClient = AccountTokenCloudflareClient
CORE.validate_bundle = validate_account_bundle


def main() -> int:
    return CORE.main()


if __name__ == "__main__":
    raise SystemExit(main())
