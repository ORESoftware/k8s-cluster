from __future__ import annotations

import json
import threading
import time
import unittest
from collections import deque
from dataclasses import dataclass
from typing import Any, Mapping

from oresoftware_durable_worker import (
    DurableWorkerClient,
    DurableWorkerError,
    LeaseLostError,
    TransportResponse,
    WorkerConfig,
    WorkerFailure,
    run_worker,
)


@dataclass
class RequestRecord:
    method: str
    url: str
    headers: Mapping[str, str]
    body: bytes | None


class ScriptedTransport:
    def __init__(self, responses: list[TransportResponse | Exception]) -> None:
        self.responses = deque(responses)
        self.requests: list[RequestRecord] = []

    def request(
        self,
        method: str,
        url: str,
        headers: Mapping[str, str],
        body: bytes | None,
        timeout_seconds: float,
        max_response_bytes: int,
    ) -> TransportResponse:
        self.requests.append(RequestRecord(method, url, dict(headers), body))
        response = self.responses.popleft()
        if isinstance(response, Exception):
            raise response
        return response


def response(status: int, payload: Mapping[str, Any], **headers: str) -> TransportResponse:
    return TransportResponse(
        status=status,
        headers=headers,
        body=json.dumps(payload).encode(),
    )


class ClientTests(unittest.TestCase):
    def test_bound_submission_retries_and_unbound_submission_does_not(self) -> None:
        retryable = response(
            503,
            {"code": "busy", "message": "busy", "retryable": True},
        )
        accepted = response(202, {"runId": "run-1", "status": "pending"})
        sleeps: list[float] = []
        transport = ScriptedTransport([retryable, accepted])
        client = DurableWorkerClient(
            "https://workers.example.test",
            "secret-value",
            transport=transport,
            sleep=sleeps.append,
            initial_backoff_seconds=0,
        )
        result = client.submit_task(
            {"idempotencyKey": "stable", "taskType": "demo", "input": {}}
        )
        self.assertEqual(result["runId"], "run-1")
        self.assertEqual(len(transport.requests), 2)
        self.assertNotIn("secret-value", transport.requests[0].url)
        self.assertEqual(transport.requests[0].headers["X-Worker-Auth"], "secret-value")
        self.assertEqual(len(sleeps), 1)

        transport = ScriptedTransport([retryable])
        client = DurableWorkerClient(
            "https://workers.example.test",
            "secret-value",
            transport=transport,
            sleep=lambda _: None,
        )
        with self.assertRaises(DurableWorkerError):
            client.submit_task({"taskType": "demo", "input": {}})
        self.assertEqual(len(transport.requests), 1)

    def test_poll_is_not_retried_after_ambiguous_transport_failure(self) -> None:
        transport = ScriptedTransport([OSError("connection reset")])
        client = DurableWorkerClient(
            "https://workers.example.test",
            "secret-value",
            transport=transport,
            sleep=lambda _: None,
        )
        with self.assertRaises(DurableWorkerError) as caught:
            client.poll_worker("worker-1")
        self.assertTrue(caught.exception.retryable)
        self.assertEqual(len(transport.requests), 1)

    def test_fenced_step_mutation_becomes_lease_lost(self) -> None:
        transport = ScriptedTransport(
            [
                response(
                    409,
                    {
                        "code": "state_conflict",
                        "message": "stale lease generation",
                        "retryable": True,
                    },
                )
            ]
        )
        client = DurableWorkerClient(
            "https://workers.example.test",
            "secret-value",
            transport=transport,
        )
        with self.assertRaises(LeaseLostError):
            client.complete_step(
                "step-1",
                {"workerId": "w", "leaseToken": "t", "leaseGeneration": 1},
                {},
            )

    def test_redirect_is_never_considered_success(self) -> None:
        transport = ScriptedTransport(
            [response(302, {"message": "redirect refused"}, location="https://evil.test")]
        )
        client = DurableWorkerClient(
            "https://workers.example.test",
            "secret-value",
            transport=transport,
        )
        with self.assertRaises(DurableWorkerError) as caught:
            client.get_run("run-1")
        self.assertEqual(caught.exception.status, 302)


class FakeWorkerClient:
    def __init__(self, assignment: Mapping[str, Any], *, fence_heartbeat: bool = False) -> None:
        self.assignment = assignment
        self.fence_heartbeat = fence_heartbeat
        self.poll_count = 0
        self.calls: list[tuple[str, Any]] = []
        self.handler_started = threading.Event()
        self.cancel_observed = threading.Event()

    def register_worker(self, payload: Mapping[str, Any]) -> Mapping[str, Any]:
        self.calls.append(("register", dict(payload)))
        return {"workerId": payload["workerId"]}

    def heartbeat_worker(self, worker_id: str, *, drain: bool | None = None) -> Mapping[str, Any]:
        self.calls.append(("worker-heartbeat", {"workerId": worker_id, "drain": drain}))
        return {"ok": True}

    def poll_worker(self, worker_id: str, *, wait_ms: int = 30_000) -> Mapping[str, Any]:
        self.poll_count += 1
        if self.poll_count == 1:
            return {"assignment": dict(self.assignment), "retryAfterMs": 1}
        return {"assignment": None, "retryAfterMs": 1}

    def start_step(self, step_id: str, lease: Mapping[str, Any]) -> Mapping[str, Any]:
        self.calls.append(("start", {"stepId": step_id, **dict(lease)}))
        return {"ok": True}

    def heartbeat_step(self, step_id: str, lease: Mapping[str, Any]) -> Mapping[str, Any]:
        self.calls.append(("step-heartbeat", {"stepId": step_id, **dict(lease)}))
        if self.fence_heartbeat:
            raise LeaseLostError("fenced")
        return {"ok": True}

    def append_step_output(
        self,
        step_id: str,
        lease: Mapping[str, Any],
        *,
        chunk_id: str,
        chunk: str,
        stream: str = "progress",
        final_chunk: bool = False,
    ) -> Mapping[str, Any]:
        self.calls.append(
            (
                "output",
                {
                    "stepId": step_id,
                    **dict(lease),
                    "chunkId": chunk_id,
                    "chunk": chunk,
                    "stream": stream,
                    "finalChunk": final_chunk,
                },
            )
        )
        return {"ok": True}

    def complete_step(
        self,
        step_id: str,
        lease: Mapping[str, Any],
        result: Mapping[str, Any] | None = None,
    ) -> Mapping[str, Any]:
        self.calls.append(("complete", {"stepId": step_id, **dict(lease), "result": result}))
        return {"ok": True}

    def fail_step(
        self,
        step_id: str,
        lease: Mapping[str, Any],
        *,
        code: str,
        message: str,
        retryable: bool,
    ) -> Mapping[str, Any]:
        self.calls.append(
            (
                "fail",
                {
                    "stepId": step_id,
                    **dict(lease),
                    "code": code,
                    "message": message,
                    "retryable": retryable,
                },
            )
        )
        return {"ok": True}


def assignment() -> Mapping[str, Any]:
    return {
        "runId": "run-1",
        "stepId": "step-1",
        "stepKey": "task",
        "taskType": "demo",
        "queue": "default",
        "input": {"value": 7},
        "attempt": 1,
        "leaseToken": "lease-token",
        "leaseGeneration": 3,
        "fencingToken": 9,
        "leaseExpiresAtMs": int(time.time() * 1000) + 30_000,
        "timeoutMs": 60_000,
        "affinityKey": None,
    }


class WorkerLoopTests(unittest.TestCase):
    def config(self) -> WorkerConfig:
        return WorkerConfig(
            worker_id="worker-1",
            queues=["default"],
            capabilities=["demo"],
            slots=1,
            max_assignments=1,
            worker_heartbeat_ms=5,
            step_heartbeat_ms=5,
            poll_wait_ms=1,
        )

    def test_worker_streams_progress_and_completes_with_same_generation(self) -> None:
        client = FakeWorkerClient(assignment())

        def handler(context: Any) -> Mapping[str, Any]:
            context.emit("working")
            time.sleep(0.012)
            return {"answer": context.input["value"] * 2}

        summary = run_worker(client, {"demo": handler}, self.config())  # type: ignore[arg-type]
        self.assertEqual(summary.accepted, 1)
        self.assertEqual(summary.completed, 1)
        self.assertEqual(summary.failed, 0)
        operations = [name for name, _ in client.calls]
        self.assertIn("step-heartbeat", operations)
        self.assertIn("output", operations)
        self.assertIn("complete", operations)
        self.assertEqual(operations[-1], "worker-heartbeat")
        terminal = next(payload for name, payload in client.calls if name == "complete")
        self.assertEqual(terminal["leaseGeneration"], 3)
        self.assertEqual(terminal["result"], {"answer": 14})

    def test_fenced_heartbeat_cancels_handler_and_suppresses_terminal_mutations(self) -> None:
        client = FakeWorkerClient(assignment(), fence_heartbeat=True)

        def handler(context: Any) -> Mapping[str, Any]:
            deadline = time.time() + 1
            while time.time() < deadline and not context.cancelled.is_set():
                time.sleep(0.002)
            client.cancel_observed.set()
            context.raise_if_cancelled()
            return {"unexpected": True}

        summary = run_worker(client, {"demo": handler}, self.config())  # type: ignore[arg-type]
        self.assertTrue(client.cancel_observed.is_set())
        self.assertEqual(summary.lease_lost, 1)
        operations = [name for name, _ in client.calls]
        self.assertNotIn("complete", operations)
        self.assertNotIn("fail", operations)

    def test_handler_failure_uses_explicit_retryability(self) -> None:
        client = FakeWorkerClient(assignment())

        def handler(context: Any) -> None:
            raise WorkerFailure("upstream_busy", "try later", retryable=True)

        summary = run_worker(client, {"demo": handler}, self.config())  # type: ignore[arg-type]
        self.assertEqual(summary.failed, 1)
        failure = next(payload for name, payload in client.calls if name == "fail")
        self.assertEqual(failure["code"], "upstream_busy")
        self.assertTrue(failure["retryable"])
        self.assertNotIn("complete", [name for name, _ in client.calls])

    def test_missing_handler_is_terminal_and_non_retryable(self) -> None:
        client = FakeWorkerClient(assignment())
        summary = run_worker(client, {}, self.config())  # type: ignore[arg-type]
        self.assertEqual(summary.failed, 1)
        failure = next(payload for name, payload in client.calls if name == "fail")
        self.assertEqual(failure["code"], "handler_not_found")
        self.assertFalse(failure["retryable"])


if __name__ == "__main__":
    unittest.main()
