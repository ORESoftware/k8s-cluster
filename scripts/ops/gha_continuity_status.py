#!/usr/bin/env python3
"""Evaluate Daedalus GitHub Actions continuity without accepting runner labels.

The evaluator combines certified ARC lane evidence with the readiness documents
from gha-clone-server and dd-build-server. It is intentionally read-only and
fails closed when no reviewed execution plane is ready.
"""

from __future__ import annotations

import argparse
import json
import sys
import urllib.error
import urllib.request
from pathlib import Path
from typing import Any

SCHEMA_VERSION = "gha-continuity-status.v1"
EXPECTED_SCALE_SET = "daedalus-ci"
EXPECTED_GROUPS = {
    "aws": "daedalus-aws",
    "hetzner": "daedalus-hetzner",
}
DEFAULT_PROVIDER_ORDER = ("hetzner", "aws")


class ContractError(ValueError):
    """Raised when a continuity snapshot is malformed or ambiguous."""


def require(condition: bool, message: str) -> None:
    if not condition:
        raise ContractError(message)


def load_json(path: Path) -> dict[str, Any]:
    value = json.loads(path.read_text(encoding="utf-8"))
    require(isinstance(value, dict), "snapshot must be a JSON object")
    return value


def readiness_url(base: str) -> str:
    trimmed = base.rstrip("/")
    return trimmed if trimmed.endswith("/readyz") else f"{trimmed}/readyz"


def fetch_readiness(base: str, timeout: float) -> dict[str, Any]:
    request = urllib.request.Request(
        readiness_url(base),
        headers={"Accept": "application/json", "User-Agent": "gha-continuity-status/1"},
    )
    try:
        with urllib.request.urlopen(request, timeout=timeout) as response:
            require(response.status == 200, f"readiness returned HTTP {response.status}")
            raw = response.read(128 * 1024 + 1)
    except (urllib.error.URLError, TimeoutError) as error:
        raise ContractError(f"readiness request failed: {error}") from error
    require(len(raw) <= 128 * 1024, "readiness response exceeds 128 KiB")
    try:
        value = json.loads(raw)
    except json.JSONDecodeError as error:
        raise ContractError(f"readiness response is not JSON: {error}") from error
    require(isinstance(value, dict), "readiness response must be a JSON object")
    return value


def parse_provider_order(raw: str) -> tuple[str, ...]:
    providers = tuple(part.strip().lower() for part in raw.split(",") if part.strip())
    require(providers, "provider order cannot be empty")
    require(len(providers) == len(set(providers)), "provider order contains duplicates")
    require(set(providers) <= set(EXPECTED_GROUPS), "provider order may contain only aws and hetzner")
    return providers


def provider_ready(provider: str, value: Any) -> tuple[bool, list[str]]:
    reasons: list[str] = []
    if not isinstance(value, dict):
        return False, ["provider evidence is missing"]
    if value.get("configured") is not True:
        reasons.append("configuration is not certified")
    if value.get("registered") is not True:
        reasons.append("ARC scale set is not registered")
    if value.get("smokePassed") is not True:
        reasons.append("manual runner smoke has not passed")
    if value.get("runnerScaleSetName") != EXPECTED_SCALE_SET:
        reasons.append(f"runnerScaleSetName must be {EXPECTED_SCALE_SET}")
    expected_group = EXPECTED_GROUPS[provider]
    if value.get("runnerGroup") != expected_group:
        reasons.append(f"runnerGroup must be {expected_group}")
    return not reasons, reasons


def bridge_ready(value: Any) -> tuple[bool, list[str]]:
    reasons: list[str] = []
    if not isinstance(value, dict):
        return False, ["gha-clone-server readiness is missing"]
    if value.get("ok") is not True:
        reasons.append("gha-clone-server is not ready")
    if value.get("executionReady") is not True:
        reasons.append("independent execution is not ready")
    if value.get("webhookExecutionReady") is not True:
        reasons.append("failure-webhook execution is not ready")
    return not reasons, reasons


def build_server_ready(value: Any) -> tuple[bool, list[str]]:
    if not isinstance(value, dict):
        return False, ["dd-build-server readiness is missing"]
    if value.get("ok") is not True:
        return False, ["dd-build-server is not ready"]
    return True, []


def evaluate(
    snapshot: dict[str, Any],
    *,
    required_mode: str = "either",
    provider_order: tuple[str, ...] = DEFAULT_PROVIDER_ORDER,
) -> dict[str, Any]:
    require(snapshot.get("schemaVersion") == SCHEMA_VERSION, f"schemaVersion must be {SCHEMA_VERSION}")
    require(required_mode in {"arc", "build-server", "either"}, "invalid required mode")
    require(provider_order, "provider order cannot be empty")
    require(set(provider_order) <= set(EXPECTED_GROUPS), "unknown provider in provider order")

    provider_values = snapshot.get("arcProviders")
    require(isinstance(provider_values, dict), "arcProviders must be a JSON object")

    provider_status: dict[str, Any] = {}
    selected_provider: str | None = None
    for provider in provider_order:
        ready, reasons = provider_ready(provider, provider_values.get(provider))
        provider_status[provider] = {"ready": ready, "reasons": reasons}
        if ready and selected_provider is None:
            selected_provider = provider

    arc_ready = selected_provider is not None
    bridge_ok, bridge_reasons = bridge_ready(snapshot.get("bridge"))
    build_ok, build_reasons = build_server_ready(snapshot.get("buildServer"))
    build_path_ready = bridge_ok and build_ok

    if required_mode == "arc":
        ready = arc_ready
    elif required_mode == "build-server":
        ready = build_path_ready
    else:
        ready = arc_ready or build_path_ready

    selected_lane: str | None = None
    if arc_ready:
        selected_lane = f"arc:{selected_provider}"
    elif build_path_ready:
        selected_lane = "build-server"

    return {
        "schemaVersion": SCHEMA_VERSION,
        "ok": ready,
        "failClosed": not ready,
        "requiredMode": required_mode,
        "selectedLane": selected_lane,
        "arc": {
            "ready": arc_ready,
            "selectedProvider": selected_provider,
            "providerOrder": list(provider_order),
            "providers": provider_status,
        },
        "buildServerPath": {
            "ready": build_path_ready,
            "bridge": {"ready": bridge_ok, "reasons": bridge_reasons},
            "buildServer": {"ready": build_ok, "reasons": build_reasons},
        },
    }


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--snapshot", type=Path, required=True)
    parser.add_argument("--require", choices=("arc", "build-server", "either"), default="either")
    parser.add_argument("--provider-order", default=",".join(DEFAULT_PROVIDER_ORDER))
    parser.add_argument("--bridge-url")
    parser.add_argument("--build-server-url")
    parser.add_argument("--timeout", type=float, default=5.0)
    args = parser.parse_args(argv)

    try:
        snapshot = load_json(args.snapshot)
        if args.bridge_url:
            snapshot["bridge"] = fetch_readiness(args.bridge_url, args.timeout)
        if args.build_server_url:
            snapshot["buildServer"] = fetch_readiness(args.build_server_url, args.timeout)
        result = evaluate(
            snapshot,
            required_mode=args.require,
            provider_order=parse_provider_order(args.provider_order),
        )
    except (ContractError, OSError, json.JSONDecodeError) as error:
        print(json.dumps({"ok": False, "failClosed": True, "error": str(error)}, sort_keys=True))
        return 3

    print(json.dumps(result, indent=2, sort_keys=True))
    return 0 if result["ok"] else 2


if __name__ == "__main__":
    sys.exit(main())
