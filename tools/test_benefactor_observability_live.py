"""Hermetic tests for the Benefactor live Prometheus verifier."""

from __future__ import annotations

from contextlib import contextmanager
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
from pathlib import Path
import tempfile
import threading
import unittest
from urllib.parse import parse_qs, urlparse

from tools.verify_benefactor_observability_live import (
    POSTGRES_READY_QUERY,
    UP_QUERY,
    VerificationError,
    main,
    verify,
)


METRICS = """# TYPE benefactor_backend_dependency_ready gauge
benefactor_backend_dependency_ready{dependency="postgres"} 1
# TYPE benefactor_backend_pipeline_runs_total counter
benefactor_backend_pipeline_runs_total{outcome="success",pipeline="gmail-poll"} 1
# TYPE benefactor_backend_sync_mutations_total counter
benefactor_backend_sync_mutations_total{operation="create",outcome="committed"} 1
# TYPE http_server_request_count_total counter
http_server_request_count_total{http_request_method="GET",http_route="/metrics",http_response_status_code="200"} 1
"""


class FixtureHandler(BaseHTTPRequestHandler):
    query_results: dict[str, list[dict]] = {}
    api_status = "success"
    metrics = METRICS
    metrics_content_type = "text/plain; version=0.0.4; charset=utf-8"

    def do_GET(self):  # noqa: N802 - standard library callback name
        parsed = urlparse(self.path)
        if parsed.path == "/api/v1/query":
            expression = parse_qs(parsed.query).get("query", [""])[0]
            payload = {
                "status": self.api_status,
                "data": {
                    "resultType": "vector",
                    "result": self.query_results.get(expression, []),
                },
            }
            body = json.dumps(payload).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
            return
        if parsed.path == "/metrics":
            body = self.metrics.encode()
            self.send_response(200)
            self.send_header("Content-Type", self.metrics_content_type)
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
            return
        self.send_error(404)

    def log_message(self, _format, *_args):
        return


@contextmanager
def fixture_server(
    *,
    up: str = "1",
    ready: str = "1",
    include_ready: bool = True,
    api_status: str = "success",
    metrics: str = METRICS,
):
    FixtureHandler.query_results = {
        UP_QUERY: [{"metric": {"job": "benefactor-backend-rs"}, "value": [1, up]}],
        POSTGRES_READY_QUERY: (
            [
                {
                    "metric": {
                        "job": "benefactor-backend-rs",
                        "dependency": "postgres",
                    },
                    "value": [1, ready],
                }
            ]
            if include_ready
            else []
        ),
    }
    FixtureHandler.api_status = api_status
    FixtureHandler.metrics = metrics
    server = ThreadingHTTPServer(("127.0.0.1", 0), FixtureHandler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        host, port = server.server_address
        yield f"http://{host}:{port}"
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=5)


class BenefactorLiveVerifierTest(unittest.TestCase):
    def test_success_captures_prometheus_and_direct_metrics_evidence(self) -> None:
        with fixture_server() as origin:
            evidence = verify(origin, timeout=2, metrics_url=f"{origin}/metrics")

        self.assertEqual(evidence["schema"], "benefactor.observability.evidence.v1")
        self.assertEqual(evidence["queries"]["up"]["value"], 1)
        self.assertEqual(evidence["queries"]["postgresReady"]["value"], 1)
        self.assertGreaterEqual(evidence["directMetrics"]["metricFamilyCount"], 4)

    def test_target_down_fails_closed(self) -> None:
        with fixture_server(up="0") as origin:
            with self.assertRaisesRegex(VerificationError, "expected every series to equal 1"):
                verify(origin, timeout=2)

    def test_missing_dependency_readiness_fails_closed(self) -> None:
        with fixture_server(include_ready=False) as origin:
            with self.assertRaisesRegex(VerificationError, "returned no series"):
                verify(origin, timeout=2)

    def test_prometheus_non_success_status_fails_closed(self) -> None:
        with fixture_server(api_status="error") as origin:
            with self.assertRaisesRegex(VerificationError, "non-success status"):
                verify(origin, timeout=2)

    def test_direct_metrics_reject_sensitive_label_names(self) -> None:
        unsafe = METRICS + 'benefactor_backend_pipeline_runs_total{email="a@example.invalid"} 1\n'
        with fixture_server(metrics=unsafe) as origin:
            with self.assertRaisesRegex(VerificationError, "forbidden high-cardinality field"):
                verify(origin, timeout=2, metrics_url=f"{origin}/metrics")

    def test_main_writes_evidence_only_after_success(self) -> None:
        with fixture_server() as origin, tempfile.TemporaryDirectory() as temporary:
            output = Path(temporary) / "evidence.json"
            status = main(
                [
                    "--prometheus-url",
                    origin,
                    "--metrics-url",
                    f"{origin}/metrics",
                    "--timeout",
                    "2",
                    "--output",
                    str(output),
                ]
            )
            self.assertEqual(status, 0)
            self.assertEqual(
                json.loads(output.read_text(encoding="utf-8"))["job"],
                "benefactor-backend-rs",
            )


if __name__ == "__main__":
    unittest.main()
