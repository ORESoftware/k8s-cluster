#!/usr/bin/env python3
"""Classify metadata-only AI Agent Bridge public-ingress evidence.

The classifier never reads or records response bodies. It augments the existing
credential-free probe evidence with a stable incident class while preserving the
probe's fail-closed exit status.
"""

from __future__ import annotations

import argparse
import copy
import json
import os
import tempfile
from pathlib import Path
from typing import Any, Iterable

SCHEMA = "ai-agent-bridge-public-ingress-diagnosis/v1"
EXPECTED_ROUTE_STATUSES = {401, 404, 405}


def _observed_statuses(payload: dict[str, Any]) -> list[int]:
    statuses: list[int] = []
    for observation in payload.get("observations", []):
        if not isinstance(observation, dict):
            continue
        status = observation.get("observed_status")
        if isinstance(status, int):
            statuses.append(status)
        elif isinstance(status, str) and status.isdigit():
            statuses.append(int(status))
    return statuses


def _transport_failure(payload: dict[str, Any]) -> bool:
    failures = payload.get("failures", [])
    if not isinstance(failures, list):
        return True
    transport_markers = (
        "dns_resolution_failed",
        "tls_verification_failed",
        "transport_failed_exit_",
        "missing_remote_ip",
        "curl_tls_verification_",
        "unexpected_effective_url",
    )
    return any(
        isinstance(failure, str)
        and any(marker in failure for marker in transport_markers)
        for failure in failures
    )


def classify(payload: dict[str, Any]) -> dict[str, Any]:
    result = copy.deepcopy(payload)
    dns = result.get("dns") if isinstance(result.get("dns"), dict) else {}
    addresses = dns.get("addresses") if isinstance(dns, dict) else []
    dns_ready = isinstance(addresses, list) and bool(addresses)
    tls_verified = result.get("tls_verified") is True
    observations = result.get("observations")
    observation_count = len(observations) if isinstance(observations, list) else 0
    statuses = _observed_statuses(result)
    passed = result.get("passed") is True

    if passed:
        classification = "healthy_fail_closed_routing"
        summary = "DNS, TLS, and all unsigned Slack route expectations passed."
        owner = "none"
        origin_reachable: bool | None = True
    elif (
        dns_ready
        and tls_verified
        and observation_count > 0
        and len(statuses) == observation_count
        and set(statuses) == {522}
    ):
        classification = "cloudflare_origin_unreachable"
        summary = (
            "The public edge completed DNS and TLS, but every reviewed route "
            "returned HTTP 522 before the application contract was reached."
        )
        owner = "edge-origin-path"
        origin_reachable = False
    elif not dns_ready or not tls_verified or _transport_failure(result):
        classification = "edge_transport_failure"
        summary = "DNS, TLS, or credential-free HTTPS transport failed before route validation."
        owner = "dns-tls-edge"
        origin_reachable = None
    elif statuses and all(status in EXPECTED_ROUTE_STATUSES for status in statuses):
        classification = "incomplete_application_evidence"
        summary = "Expected fail-closed statuses were observed, but another evidence invariant failed."
        owner = "probe-contract"
        origin_reachable = True
    else:
        classification = "application_contract_failure"
        summary = "The edge answered, but one or more application route expectations did not match."
        owner = "application-routing"
        origin_reachable = True

    result["diagnosis"] = {
        "schema": SCHEMA,
        "classification": classification,
        "summary": summary,
        "owner": owner,
        "dns_ready": dns_ready,
        "edge_tls_reachable": tls_verified,
        "origin_application_reachable": origin_reachable,
        "observation_count": observation_count,
        "observed_statuses": statuses,
        "response_bodies_recorded": False,
        "recommended_checks": (
            [
                "verify proxied DNS resolves to the intended ingress origin",
                "inspect ingress-nginx service load-balancer address and endpoints",
                "inspect dd-slack-command service and endpoint slices",
                "probe the origin directly with the api.fiducia.cloud SNI and Host header",
                "inspect firewall, security-group, and network path from the edge to the origin",
            ]
            if classification == "cloudflare_origin_unreachable"
            else []
        ),
    }
    return result


def write_atomic(path: Path, payload: dict[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary: Path | None = None
    try:
        with tempfile.NamedTemporaryFile(
            mode="w",
            encoding="utf-8",
            prefix=f".{path.name}.",
            suffix=".tmp",
            dir=path.parent,
            delete=False,
        ) as handle:
            temporary = Path(handle.name)
            json.dump(payload, handle, indent=2, sort_keys=True)
            handle.write("\n")
            handle.flush()
            os.fsync(handle.fileno())
        os.replace(temporary, path)
        temporary = None
    finally:
        if temporary is not None:
            temporary.unlink(missing_ok=True)


def fixture(*, passed: bool, statuses: Iterable[int], dns: bool = True, tls: bool = True) -> dict[str, Any]:
    status_list = list(statuses)
    return {
        "schema_version": 1,
        "host": "api.fiducia.cloud",
        "dns": {"addresses": ["192.0.2.1"] if dns else [], "error": None},
        "tls_verified": tls,
        "observations": [
            {
                "name": f"route-{index}",
                "observed_status": status,
                "response_body_recorded": False,
                "valid": passed,
            }
            for index, status in enumerate(status_list)
        ],
        "failures": [] if passed else ["synthetic_failure"],
        "response_bodies_recorded": False,
        "passed": passed,
    }


def self_test() -> None:
    healthy = classify(fixture(passed=True, statuses=[401, 401, 401, 405, 404]))
    assert healthy["diagnosis"]["classification"] == "healthy_fail_closed_routing"

    outage = classify(fixture(passed=False, statuses=[522, 522, 522, 522, 522]))
    assert outage["diagnosis"]["classification"] == "cloudflare_origin_unreachable"
    assert outage["diagnosis"]["edge_tls_reachable"] is True
    assert outage["diagnosis"]["origin_application_reachable"] is False
    assert len(outage["diagnosis"]["recommended_checks"]) == 5

    dns_failure = classify(fixture(passed=False, statuses=[], dns=False, tls=False))
    assert dns_failure["diagnosis"]["classification"] == "edge_transport_failure"

    application = classify(fixture(passed=False, statuses=[500, 500, 500]))
    assert application["diagnosis"]["classification"] == "application_contract_failure"

    incomplete = fixture(passed=False, statuses=[401, 401, 401, 405, 404])
    incomplete["failures"] = ["response_body_exceeds_4096_bytes"]
    classified = classify(incomplete)
    assert classified["diagnosis"]["classification"] == "incomplete_application_evidence"

    raw = json.dumps(outage).lower()
    for forbidden in (
        "response_body\":",
        "slack_signing_secret",
        "authorization: bearer",
        "ghp_",
        "github_pat_",
    ):
        assert forbidden not in raw

    print(
        "public ingress classifier self-test passed: healthy + 522 origin outage + "
        "edge transport + application mismatch + incomplete evidence"
    )


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--input", type=Path)
    parser.add_argument("--output", type=Path)
    parser.add_argument("--self-test", action="store_true")
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(list(argv) if argv is not None else list(__import__("sys").argv[1:]))
    if args.self_test:
        self_test()
        return 0
    if args.input is None or args.output is None:
        raise SystemExit("--input and --output are required unless --self-test is used")
    payload = json.loads(args.input.read_text(encoding="utf-8"))
    if not isinstance(payload, dict):
        raise SystemExit("public ingress evidence must be a JSON object")
    classified = classify(payload)
    write_atomic(args.output, classified)
    diagnosis = classified["diagnosis"]
    print(
        json.dumps(
            {
                "classification": diagnosis["classification"],
                "dns_ready": diagnosis["dns_ready"],
                "edge_tls_reachable": diagnosis["edge_tls_reachable"],
                "origin_application_reachable": diagnosis["origin_application_reachable"],
                "passed": classified.get("passed") is True,
            },
            sort_keys=True,
        )
    )
    return 0 if classified.get("passed") is True else 1


if __name__ == "__main__":
    raise SystemExit(main())
