#!/usr/bin/env python3
"""Run the Canonical preflight with Cloudflare account-owned token semantics.

Cloudflare's 2026 account-owned `cfat_` tokens are verified through
`GET /accounts/{account_id}/tokens/verify`; the user-token endpoint is a
separate credential family. This adapter keeps the existing no-write inventory
surface intact while selecting the correct reviewed endpoint and preserving
partial evidence when a least-privilege token cannot read one product surface.
"""
from __future__ import annotations

import importlib.util
import re
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
    """GET-only client that adds only the account-token verification endpoint."""

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


def _require_dict(value: Any, label: str) -> dict[str, Any]:
    if not isinstance(value, dict):
        raise CORE.PreflightError(f"Cloudflare {label} did not return an object")
    return value


def _require_list(value: Any, label: str) -> list[Any]:
    if not isinstance(value, list):
        raise CORE.PreflightError(f"Cloudflare {label} did not return a list")
    return value


def cloudflare_inventory(
    values: dict[str, str], contract: dict[str, Any]
) -> tuple[dict[str, Any], list[str]]:
    """Inventory every readable Canonical surface and retain denied sections."""

    blockers: list[str] = []
    desired = contract["cloudflare"]
    account_id = values["cloudflare_account_id"]
    client = AccountTokenCloudflareClient(
        values["cloudflare_api_token"], account_id
    )

    _, token_value = client.get(
        client.account_token_verify_path,
        label="account-owned token verification",
    )
    token = _require_dict(token_value, "account-owned token verification")
    if token.get("status") != "active":
        raise CORE.PreflightError("Cloudflare account-owned API token is not active")

    _, account_value = client.get(
        f"/accounts/{account_id}", label="account verification"
    )
    account = _require_dict(account_value, "account verification")
    if account.get("id") != account_id:
        raise CORE.PreflightError(
            "Cloudflare account-owned token did not resolve to the reviewed account"
        )

    _, zones_value = client.get(
        "/zones",
        query={
            "name": desired["zone_name"],
            "account.id": account_id,
            "per_page": "50",
        },
        label="canonical.plus zone lookup",
    )
    zones = _require_list(zones_value, "canonical.plus zone lookup")
    exact_zones = [
        zone
        for zone in zones
        if isinstance(zone, dict)
        and zone.get("name") == desired["zone_name"]
        and isinstance(zone.get("account"), dict)
        and zone["account"].get("id") == account_id
    ]
    if len(exact_zones) != 1:
        raise CORE.PreflightError(
            f"expected one canonical.plus zone in the reviewed account; found {len(exact_zones)}"
        )
    zone = exact_zones[0]
    zone_id = zone.get("id")
    if not isinstance(zone_id, str) or not re.fullmatch(r"[0-9a-f]{32}", zone_id):
        raise CORE.PreflightError("canonical.plus zone has an invalid identifier")
    client.bind_zone(zone_id)

    script_status, scripts_value = client.get(
        f"/accounts/{account_id}/workers/scripts",
        label="Worker script inventory",
        optional_statuses=(403,),
    )
    scripts_readable = script_status == 200
    scripts: list[Any] = []
    if scripts_readable:
        scripts = _require_list(scripts_value, "Worker script inventory")
    else:
        blockers.append(
            "account token lacks read access to Cloudflare Worker script inventory"
        )

    matching_scripts = [
        script
        for script in scripts
        if isinstance(script, dict)
        and (
            script.get("id") == desired["worker_script"]
            or script.get("name") == desired["worker_script"]
        )
    ]
    if len(matching_scripts) > 1:
        raise CORE.PreflightError("the exact Canonical Worker script is ambiguous")
    worker_exists: bool | None = len(matching_scripts) == 1 if scripts_readable else None
    if scripts_readable and not worker_exists:
        blockers.append("Worker script canonical-plus-auth-edge is not deployed")

    settings_status: int | None = None
    if worker_exists:
        settings_status, _ = client.get(
            f"/accounts/{account_id}/workers/scripts/{desired['worker_script']}/settings",
            label="Worker production settings",
            optional_statuses=(403, 404),
        )
        if settings_status == 403:
            blockers.append(
                "account token lacks read access to the exact Worker production settings"
            )
        elif settings_status == 404:
            blockers.append(
                "the exact Worker exists but its top-level production settings were not found"
            )

    route_status, routes_value = client.get(
        f"/zones/{zone_id}/workers/routes",
        label="canonical.plus Worker routes",
        optional_statuses=(403,),
    )
    routes_readable = route_status == 200
    routes: list[Any] = []
    route_results: list[dict[str, Any]] = []
    unexpected_patterns: list[str] = []
    if routes_readable:
        routes = _require_list(routes_value, "canonical.plus Worker routes")
        for pattern in desired["routes"]:
            matches = [
                route
                for route in routes
                if isinstance(route, dict) and route.get("pattern") == pattern
            ]
            if len(matches) > 1:
                raise CORE.PreflightError(
                    f"multiple exact Worker routes exist for {pattern}"
                )
            owner = matches[0].get("script") if matches else None
            conflict = bool(matches and owner != desired["worker_script"])
            if not matches:
                blockers.append(f"missing exact Worker route: {pattern}")
            elif conflict:
                blockers.append(
                    f"exact Worker route is owned by another script: {pattern}"
                )
            route_results.append(
                {
                    "pattern": pattern,
                    "readable": True,
                    "exists": bool(matches),
                    "script": owner,
                    "conflict": conflict,
                    "id_sha256": CORE.sha256(matches[0].get("id"))
                    if matches
                    else None,
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
    else:
        blockers.append(
            "account token lacks read access to canonical.plus Worker routes"
        )
        route_results = [
            {
                "pattern": pattern,
                "readable": False,
                "exists": None,
                "script": None,
                "conflict": None,
                "id_sha256": None,
            }
            for pattern in desired["routes"]
        ]

    dns_results: list[dict[str, Any]] = []
    for name in desired["dns_names"]:
        dns_status, records_value = client.get(
            f"/zones/{zone_id}/dns_records",
            query={"name": name, "per_page": "100"},
            label=f"DNS lookup for {name}",
            optional_statuses=(403,),
        )
        if dns_status == 403:
            blockers.append(f"account token lacks DNS read access for {name}")
            dns_results.append(
                {"name": name, "readable": False, "exists": None, "record": None}
            )
            continue
        records = _require_list(records_value, f"DNS lookup for {name}")
        exact = [
            record
            for record in records
            if isinstance(record, dict) and record.get("name") == name
        ]
        if len(exact) > 1:
            raise CORE.PreflightError(
                f"multiple exact DNS records exist for {name}"
            )
        if not exact:
            blockers.append(f"missing exact DNS record: {name}")
            dns_results.append(
                {"name": name, "readable": True, "exists": False, "record": None}
            )
            continue
        record = exact[0]
        content = str(record.get("content", ""))
        dns_results.append(
            {
                "name": name,
                "readable": True,
                "exists": True,
                "record": {
                    "id_sha256": CORE.sha256(record.get("id")),
                    "type": record.get("type"),
                    "proxied": record.get("proxied"),
                    "proxiable": record.get("proxiable"),
                    "ttl": record.get("ttl"),
                    "content_sha256": CORE.sha256(content),
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
                "family": "account-owned",
                "status": token.get("status"),
                "id_sha256": CORE.sha256(token.get("id")),
                "expires_on": token.get("expires_on"),
            },
            "account": {
                "id_sha256": CORE.sha256(account.get("id")),
                "name": account.get("name"),
                "type": account.get("type"),
            },
            "zone": {
                "id_sha256": CORE.sha256(zone_id),
                "name": zone.get("name"),
                "status": zone.get("status"),
                "type": zone.get("type"),
                "account_id_sha256": CORE.sha256(
                    (zone.get("account") or {}).get("id")
                ),
            },
            "worker": {
                "script": desired["worker_script"],
                "environment": desired["worker_environment"],
                "script_inventory_readable": scripts_readable,
                "exists": worker_exists,
                "settings_status": settings_status,
                "created_on": script.get("created_on"),
                "modified_on": script.get("modified_on"),
            },
            "routes_readable": routes_readable,
            "routes": route_results,
            "unexpected_canonical_route_patterns": unexpected_patterns,
            "dns": dns_results,
            "write_performed": False,
            "ready_for_write": False,
        },
        blockers,
    )


CORE.CloudflareClient = AccountTokenCloudflareClient
CORE.validate_bundle = validate_account_bundle
CORE.cloudflare_inventory = cloudflare_inventory


def main() -> int:
    return CORE.main()


if __name__ == "__main__":
    raise SystemExit(main())
