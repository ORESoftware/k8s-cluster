#!/usr/bin/env python3
from __future__ import annotations

import json
import sys
import tempfile
import threading
import unittest
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from gha_continuity_status import (  # noqa: E402
    ContractError,
    evaluate,
    fetch_readiness,
    main,
    parse_provider_order,
)


def provider(cloud: str, *, ready: bool = True) -> dict[str, object]:
    return {
        "configured": ready,
        "registered": ready,
        "smokePassed": ready,
        "runnerScaleSetName": "daedalus-ci",
        "runnerGroup": f"daedalus-{cloud}",
    }


def snapshot() -> dict[str, object]:
    return {
        "schemaVersion": "gha-continuity-status.v1",
        "arcProviders": {
            "aws": provider("aws", ready=False),
            "hetzner": provider("hetzner", ready=False),
        },
        "bridge": {
            "ok": False,
            "executionReady": False,
            "webhookExecutionReady": False,
        },
        "buildServer": {"ok": False},
    }


class ContinuityEvaluationTests(unittest.TestCase):
    def test_prefers_hetzner_when_both_arc_lanes_are_certified(self) -> None:
        value = snapshot()
        value["arcProviders"] = {
            "aws": provider("aws"),
            "hetzner": provider("hetzner"),
        }
        result = evaluate(value)
        self.assertTrue(result["ok"])
        self.assertEqual(result["selectedLane"], "arc:hetzner")
        self.assertEqual(result["arc"]["selectedProvider"], "hetzner")

    def test_falls_back_to_aws_when_hetzner_smoke_is_missing(self) -> None:
        value = snapshot()
        value["arcProviders"] = {
            "aws": provider("aws"),
            "hetzner": provider("hetzner", ready=False),
        }
        result = evaluate(value, required_mode="arc")
        self.assertTrue(result["ok"])
        self.assertEqual(result["selectedLane"], "arc:aws")
        self.assertIn(
            "manual runner smoke has not passed",
            result["arc"]["providers"]["hetzner"]["reasons"],
        )

    def test_build_server_is_used_when_arc_is_not_certified(self) -> None:
        value = snapshot()
        value["bridge"] = {
            "ok": True,
            "executionReady": True,
            "webhookExecutionReady": True,
        }
        value["buildServer"] = {"ok": True}
        result = evaluate(value)
        self.assertTrue(result["ok"])
        self.assertEqual(result["selectedLane"], "build-server")

    def test_build_server_path_requires_failure_webhook_readiness(self) -> None:
        value = snapshot()
        value["bridge"] = {
            "ok": True,
            "executionReady": True,
            "webhookExecutionReady": False,
        }
        value["buildServer"] = {"ok": True}
        result = evaluate(value, required_mode="build-server")
        self.assertFalse(result["ok"])
        self.assertTrue(result["failClosed"])
        self.assertIsNone(result["selectedLane"])

    def test_wrong_runner_group_cannot_claim_readiness(self) -> None:
        value = snapshot()
        bad = provider("aws")
        bad["runnerGroup"] = "caller-selected-group"
        value["arcProviders"] = {"aws": bad, "hetzner": provider("hetzner", ready=False)}
        result = evaluate(value, required_mode="arc", provider_order=("aws", "hetzner"))
        self.assertFalse(result["ok"])
        self.assertIn(
            "runnerGroup must be daedalus-aws",
            result["arc"]["providers"]["aws"]["reasons"],
        )

    def test_wrong_scale_set_cannot_claim_readiness(self) -> None:
        value = snapshot()
        bad = provider("hetzner")
        bad["runnerScaleSetName"] = "unreviewed-label"
        value["arcProviders"] = {"aws": provider("aws", ready=False), "hetzner": bad}
        result = evaluate(value, required_mode="arc")
        self.assertFalse(result["ok"])
        self.assertIn(
            "runnerScaleSetName must be daedalus-ci",
            result["arc"]["providers"]["hetzner"]["reasons"],
        )

    def test_provider_order_is_bounded_and_unambiguous(self) -> None:
        self.assertEqual(parse_provider_order("hetzner,aws"), ("hetzner", "aws"))
        with self.assertRaises(ContractError):
            parse_provider_order("aws,aws")
        with self.assertRaises(ContractError):
            parse_provider_order("gcp")
        with self.assertRaises(ContractError):
            parse_provider_order("")

    def test_schema_mismatch_fails_before_selection(self) -> None:
        value = snapshot()
        value["schemaVersion"] = "future"
        with self.assertRaises(ContractError):
            evaluate(value)


class ReadinessHandler(BaseHTTPRequestHandler):
    response_status = 200
    response_body: object = {"ok": True}

    def do_GET(self) -> None:  # noqa: N802
        body = json.dumps(self.response_body).encode("utf-8")
        self.send_response(self.response_status)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, _format: str, *_args: object) -> None:
        return


class EndpointAndCliTests(unittest.TestCase):
    def serve(self, body: object, status: int = 200) -> tuple[ThreadingHTTPServer, str]:
        handler = type("Handler", (ReadinessHandler,), {"response_body": body, "response_status": status})
        server = ThreadingHTTPServer(("127.0.0.1", 0), handler)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        host, port = server.server_address
        return server, f"http://{host}:{port}"

    def test_fetch_readiness_appends_readyz_and_parses_json(self) -> None:
        server, url = self.serve({"ok": True, "executionReady": True})
        try:
            self.assertEqual(fetch_readiness(url, 1.0)["ok"], True)
        finally:
            server.shutdown()
            server.server_close()

    def test_cli_exit_codes_distinguish_ready_unready_and_invalid(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "snapshot.json"
            value = snapshot()
            path.write_text(json.dumps(value), encoding="utf-8")
            self.assertEqual(main(["--snapshot", str(path)]), 2)

            value["arcProviders"] = {
                "aws": provider("aws"),
                "hetzner": provider("hetzner", ready=False),
            }
            path.write_text(json.dumps(value), encoding="utf-8")
            self.assertEqual(main(["--snapshot", str(path), "--require", "arc"]), 0)

            path.write_text("[]", encoding="utf-8")
            self.assertEqual(main(["--snapshot", str(path)]), 3)


if __name__ == "__main__":
    unittest.main(verbosity=2)
