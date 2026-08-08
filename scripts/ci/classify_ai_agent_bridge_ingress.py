#!/usr/bin/env python3
"""Classify metadata-only AI agent bridge public-ingress probe evidence."""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path
from typing import Any

CLOUDFLARE_FAILURES = {
    520: (
        "cloudflare_origin_unknown_error",
        "Inspect the origin gateway and application logs.",
    ),
    521: (
        "cloudflare_origin_connection_refused",
        "Restore the origin listener on ports 80/443 and verify the Cloudflare origin address.",
    ),
    522: (
        "cloudflare_origin_connection_timeout",
        "Verify the origin host is running, routable, and accepting Cloudflare connections on ports 80/443.",
    ),
    523: (
        "cloudflare_origin_unreachable",
        "Verify the configured origin address and upstream routing.",
    ),
    524: (
        "cloudflare_origin_response_timeout",
        "Inspect the origin gateway and upstream service latency after connection establishment.",
    ),
    525: (
        "cloudflare_origin_tls_handshake_failure",
        "Repair the origin TLS listener and certificate chain used by Cloudflare.",
    ),
    526: (
        "cloudflare_origin_certificate_invalid",
        "Replace or correct the origin certificate trusted by Cloudflare.",
    ),
    530: (
        "cloudflare_origin_resolution_failure",
        "Inspect Cloudflare origin resolution and zone configuration.",
    ),
}


class EvidenceError(RuntimeError):
    """Raised when probe evidence cannot be classified safely."""


def _load_evidence(path: Path) -> dict[str, Any]:
    payload = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(payload, dict):
        raise EvidenceError("probe evidence must be a JSON object")
    observations = payload.get("observations")
    if not isinstance(observations, list):
        raise EvidenceError("probe evidence observations must be an array")
    return payload


def _status_values(payload: dict[str, Any]) -> list[int]:
    statuses: list[int] = []
    for index, observation in enumerate(payload["observations"]):
        if not isinstance(observation, dict):
            raise EvidenceError(f"observation {index} must be an object")
        status = observation.get("observed_status")
        if isinstance(status, int):
            statuses.append(status)
        elif isinstance(status, str) and status.isdigit():
            statuses.append(int(status))
        else:
            raise EvidenceError(
                f"observation {index} has invalid status: {status!r}"
            )
    return statuses


def classify(payload: dict[str, Any]) -> dict[str, Any]:
    statuses = _status_values(payload)
    dns = payload.get("dns") if isinstance(payload.get("dns"), dict) else {}
    addresses = dns.get("addresses") if isinstance(dns.get("addresses"), list) else []
    tls_verified = payload.get("tls_verified") is True
    remote_ips = sorted(
        {
            observation.get("remote_ip")
            for observation in payload["observations"]
            if isinstance(observation, dict)
            and isinstance(observation.get("remote_ip"), str)
            and observation.get("remote_ip")
        }
    )
    edge_reachable = bool(addresses) and tls_verified and bool(remote_ips)

    if payload.get("passed") is True:
        classification = "healthy"
        origin_reachability = "healthy_through_edge"
        recommended_action = "No remediation required."
    elif statuses and len(set(statuses)) == 1 and statuses[0] in CLOUDFLARE_FAILURES:
        classification, recommended_action = CLOUDFLARE_FAILURES[statuses[0]]
        origin_reachability = "failed"
    elif not statuses or all(status == 0 for status in statuses):
        classification = "public_transport_failure"
        origin_reachability = "unknown"
        recommended_action = (
            "Inspect DNS, edge TLS, and runner network reachability before "
            "application routes."
        )
    elif any(status in CLOUDFLARE_FAILURES for status in statuses):
        classification = "mixed_cloudflare_origin_failure"
        origin_reachability = "failed_or_unstable"
        recommended_action = (
            "Inspect Cloudflare origin health and compare route-specific "
            "upstream behavior."
        )
    else:
        classification = "route_contract_failure"
        origin_reachability = "reachable_or_unknown"
        recommended_action = (
            "Inspect route status and response-contract drift at the measured "
            "revision."
        )

    return {
        "schema_version": 1,
        "host": payload.get("host"),
        "probe_passed": payload.get("passed") is True,
        "classification": classification,
        "edge_dns_resolved": bool(addresses),
        "edge_tls_verified": tls_verified,
        "edge_reachable": edge_reachable,
        "origin_reachability": origin_reachability,
        "observed_statuses": statuses,
        "all_observed_statuses_equal": bool(statuses) and len(set(statuses)) == 1,
        "remote_ips": remote_ips,
        "response_bodies_recorded": False,
        "credential_material_required": False,
        "recommended_action": recommended_action,
    }


def missing_evidence_diagnosis(path: Path) -> dict[str, Any]:
    return {
        "schema_version": 1,
        "host": None,
        "probe_passed": False,
        "classification": "probe_evidence_missing",
        "edge_dns_resolved": False,
        "edge_tls_verified": False,
        "edge_reachable": False,
        "origin_reachability": "unknown",
        "observed_statuses": [],
        "all_observed_statuses_equal": False,
        "remote_ips": [],
        "response_bodies_recorded": False,
        "credential_material_required": False,
        "recommended_action": (
            "Inspect why the probe did not write metadata evidence at "
            f"{path.name}."
        ),
    }


def write_json(path: Path, payload: dict[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_suffix(path.suffix + ".tmp")
    temporary.write_text(
        json.dumps(payload, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )
    temporary.replace(path)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--evidence", required=True)
    parser.add_argument("--output", required=True)
    args = parser.parse_args()

    evidence_path = Path(args.evidence)
    output_path = Path(args.output)
    try:
        diagnosis = (
            classify(_load_evidence(evidence_path))
            if evidence_path.is_file()
            else missing_evidence_diagnosis(evidence_path)
        )
        write_json(output_path, diagnosis)
        print(json.dumps(diagnosis, sort_keys=True))
        return 0
    except (EvidenceError, OSError, json.JSONDecodeError) as error:
        print(f"ERROR: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
