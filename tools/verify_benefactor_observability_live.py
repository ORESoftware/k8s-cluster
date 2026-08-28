#!/usr/bin/env python3
"""Collect reproducible live Prometheus evidence for benefactor-backend-rs.

The verifier is intentionally read-only. It queries Prometheus's HTTP API and,
optionally, the backend's direct /metrics endpoint through an operator-created
port-forward. It never reads Kubernetes Secrets or sends Benefactor API traffic.
"""

from __future__ import annotations

import argparse
from datetime import datetime, timezone
import json
from pathlib import Path
import re
import sys
from typing import Any
from urllib.error import HTTPError, URLError
from urllib.parse import urlencode
from urllib.request import Request, urlopen


JOB = "benefactor-backend-rs"
UP_QUERY = f'up{{job="{JOB}"}}'
POSTGRES_READY_QUERY = (
    f'benefactor_backend_dependency_ready{{job="{JOB}",dependency="postgres"}}'
)
REQUIRED_METRICS = (
    "benefactor_backend_dependency_ready",
    "benefactor_backend_pipeline_runs_total",
    "benefactor_backend_sync_mutations_total",
    "http_server_request_count_total",
)
FORBIDDEN_LABEL_OR_METRIC = re.compile(
    r"(?:email|lead_id|contact|crm_id|provider_query|raw_url|access_token|secret)",
    re.IGNORECASE,
)


class VerificationError(RuntimeError):
    """The live observability contract was not satisfied."""


def _request_json(url: str, timeout: float) -> dict[str, Any]:
    request = Request(url, headers={"Accept": "application/json"})
    try:
        with urlopen(request, timeout=timeout) as response:
            payload = json.load(response)
    except (HTTPError, URLError, TimeoutError, json.JSONDecodeError) as error:
        raise VerificationError(f"Prometheus request failed: {error}") from error
    if payload.get("status") != "success":
        raise VerificationError(
            f"Prometheus returned non-success status: {payload.get('status')!r}"
        )
    data = payload.get("data")
    if not isinstance(data, dict) or not isinstance(data.get("result"), list):
        raise VerificationError("Prometheus response is missing data.result")
    return payload


def query(prometheus_url: str, expression: str, timeout: float) -> list[dict[str, Any]]:
    endpoint = f"{prometheus_url.rstrip('/')}/api/v1/query?{urlencode({'query': expression})}"
    return _request_json(endpoint, timeout)["data"]["result"]


def _sample_value(sample: dict[str, Any]) -> float:
    value = sample.get("value")
    if not isinstance(value, list) or len(value) != 2:
        raise VerificationError("Prometheus vector sample has an invalid value")
    try:
        return float(value[1])
    except (TypeError, ValueError) as error:
        raise VerificationError("Prometheus vector sample is not numeric") from error


def require_all_one(name: str, samples: list[dict[str, Any]]) -> list[dict[str, Any]]:
    if not samples:
        raise VerificationError(f"{name} returned no series")
    bad = [sample for sample in samples if _sample_value(sample) != 1.0]
    if bad:
        raise VerificationError(f"{name} expected every series to equal 1")
    return samples


def verify_direct_metrics(metrics_url: str, timeout: float) -> dict[str, Any]:
    request = Request(metrics_url, headers={"Accept": "text/plain"})
    try:
        with urlopen(request, timeout=timeout) as response:
            content_type = response.headers.get("Content-Type", "")
            text = response.read(2_000_000).decode("utf-8", errors="replace")
    except (HTTPError, URLError, TimeoutError) as error:
        raise VerificationError(f"direct /metrics request failed: {error}") from error

    if "text/plain" not in content_type:
        raise VerificationError(
            f"direct /metrics returned unexpected Content-Type {content_type!r}"
        )
    missing = [metric for metric in REQUIRED_METRICS if metric not in text]
    if missing:
        raise VerificationError(
            "direct /metrics is missing required metric families: " + ", ".join(missing)
        )

    exposed_names: set[str] = set()
    for line in text.splitlines():
        if not line or line.startswith("#"):
            continue
        name = line.split("{", 1)[0].split(" ", 1)[0]
        exposed_names.add(name)
        if FORBIDDEN_LABEL_OR_METRIC.search(line.split(" ", 1)[0]):
            raise VerificationError(
                f"direct /metrics exposes a forbidden high-cardinality field: {name}"
            )

    return {
        "url": metrics_url,
        "contentType": content_type,
        "metricFamilyCount": len(exposed_names),
        "requiredMetricFamilies": list(REQUIRED_METRICS),
    }


def verify(
    prometheus_url: str,
    timeout: float = 10.0,
    metrics_url: str | None = None,
) -> dict[str, Any]:
    up = require_all_one("Benefactor Prometheus up query", query(prometheus_url, UP_QUERY, timeout))
    postgres = require_all_one(
        "Benefactor Postgres readiness query",
        query(prometheus_url, POSTGRES_READY_QUERY, timeout),
    )

    evidence: dict[str, Any] = {
        "schema": "benefactor.observability.evidence.v1",
        "verifiedAt": datetime.now(timezone.utc).isoformat(),
        "job": JOB,
        "prometheusUrl": prometheus_url,
        "queries": {
            "up": {"expression": UP_QUERY, "series": len(up), "value": 1},
            "postgresReady": {
                "expression": POSTGRES_READY_QUERY,
                "series": len(postgres),
                "value": 1,
            },
        },
    }
    if metrics_url:
        evidence["directMetrics"] = verify_direct_metrics(metrics_url, timeout)
    return evidence


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--prometheus-url",
        default="http://127.0.0.1:9090",
        help="Prometheus base URL, normally an operator-created port-forward",
    )
    parser.add_argument(
        "--metrics-url",
        help="Optional direct benefactor-backend-rs /metrics port-forward URL",
    )
    parser.add_argument("--timeout", type=float, default=10.0)
    parser.add_argument(
        "--output",
        type=Path,
        help="Optional JSON evidence destination; written only after all checks pass",
    )
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(sys.argv[1:] if argv is None else argv)
    try:
        evidence = verify(args.prometheus_url, args.timeout, args.metrics_url)
    except VerificationError as error:
        print(f"Benefactor observability verification failed: {error}", file=sys.stderr)
        return 1

    rendered = json.dumps(evidence, indent=2, sort_keys=True) + "\n"
    if args.output:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        temporary = args.output.with_suffix(args.output.suffix + ".tmp")
        temporary.write_text(rendered, encoding="utf-8")
        temporary.replace(args.output)
    print(rendered, end="")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
