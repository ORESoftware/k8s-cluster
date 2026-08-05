"""Dependency-free Python SDK for the ORESoftware durable-worker runtime.

The service provides at-least-once delivery. Callers must make external effects
idempotent or guard them with the assignment's fencing token.
"""

from __future__ import annotations

import json
import random
import threading
import time
import urllib.error
import urllib.parse
import urllib.request
from concurrent.futures import FIRST_COMPLETED, Future, ThreadPoolExecutor, wait
from dataclasses import dataclass, field
from http.client import HTTPMessage
from typing import Any, Callable, Mapping, MutableMapping, Protocol, Sequence, TypeAlias

__all__ = [
    "DurableWorkerClient",
    "DurableWorkerError",
    "Handler",
    "LeaseLostError",
    "TaskContext",
    "Transport",
    "TransportResponse",
    "UrllibTransport",
    "WorkerConfig",
    "WorkerFailure",
    "WorkerSummary",
    "run_worker",
]

JsonObject: TypeAlias = MutableMapping[str, Any]
Handler: TypeAlias = Callable[["TaskContext"], Mapping[str, Any] | None]

_TRANSIENT_STATUSES = frozenset({408, 425, 429, 500, 502, 503, 504})
_DEFAULT_MAX_RESPONSE_BYTES = 2 * 1024 * 1024


@dataclass(frozen=True, slots=True)
class TransportResponse:
    status: int
    headers: Mapping[str, str]
    body: bytes


class Transport(Protocol):
    def request(
        self,
        method: str,
        url: str,
        headers: Mapping[str, str],
        body: bytes | None,
        timeout_seconds: float,
        max_response_bytes: int,
    ) -> TransportResponse: ...


class _NoRedirectHandler(urllib.request.HTTPRedirectHandler):
    def redirect_request(  # type: ignore[override]
        self,
        req: urllib.request.Request,
        fp: Any,
        code: int,
        msg: str,
        headers: HTTPMessage,
        newurl: str,
    ) -> None:
        return None


class UrllibTransport:
    """Small synchronous HTTP transport that refuses redirects.

    Refusing redirects prevents the worker authorization header from being
    forwarded to an unreviewed origin.
    """

    def __init__(self) -> None:
        self._opener = urllib.request.build_opener(_NoRedirectHandler())

    def request(
        self,
        method: str,
        url: str,
        headers: Mapping[str, str],
        body: bytes | None,
        timeout_seconds: float,
        max_response_bytes: int,
    ) -> TransportResponse:
        request = urllib.request.Request(
            url=url,
            data=body,
            headers=dict(headers),
            method=method,
        )
        try:
            response = self._opener.open(request, timeout=timeout_seconds)
        except urllib.error.HTTPError as error:
            payload = _read_bounded(error, max_response_bytes)
            return TransportResponse(
                status=error.code,
                headers={key.lower(): value for key, value in error.headers.items()},
                body=payload,
            )
        with response:
            payload = _read_bounded(response, max_response_bytes)
            return TransportResponse(
                status=response.status,
                headers={key.lower(): value for key, value in response.headers.items()},
                body=payload,
            )


def _read_bounded(response: Any, max_response_bytes: int) -> bytes:
    payload = response.read(max_response_bytes + 1)
    if len(payload) > max_response_bytes:
        raise DurableWorkerError(
            code="response_too_large",
            message=f"response exceeded {max_response_bytes} bytes",
            retryable=False,
        )
    return payload


class DurableWorkerError(RuntimeError):
    def __init__(
        self,
        code: str,
        message: str,
        *,
        status: int | None = None,
        retryable: bool = False,
    ) -> None:
        super().__init__(message)
        self.code = code
        self.status = status
        self.retryable = retryable


class LeaseLostError(DurableWorkerError):
    def __init__(self, message: str, *, status: int | None = 409) -> None:
        super().__init__(
            "lease_lost",
            message,
            status=status,
            retryable=False,
        )


class WorkerFailure(RuntimeError):
    """Handler-selected failure classification sent to the control plane."""

    def __init__(self, code: str, message: str, *, retryable: bool = False) -> None:
        super().__init__(message)
        self.code = code
        self.retryable = retryable


@dataclass(frozen=True, slots=True)
class _LeaseIdentity:
    worker_id: str
    lease_token: str
    lease_generation: int

    @classmethod
    def from_assignment(cls, worker_id: str, assignment: Mapping[str, Any]) -> "_LeaseIdentity":
        try:
            token = str(assignment["leaseToken"])
            generation = int(assignment["leaseGeneration"])
        except (KeyError, TypeError, ValueError) as error:
            raise DurableWorkerError(
                "invalid_assignment",
                "assignment is missing a valid lease token or generation",
                retryable=False,
            ) from error
        return cls(worker_id=worker_id, lease_token=token, lease_generation=generation)

    def payload(self) -> JsonObject:
        return {
            "workerId": self.worker_id,
            "leaseToken": self.lease_token,
            "leaseGeneration": self.lease_generation,
        }


class DurableWorkerClient:
    """Synchronous client with narrowly bounded automatic retries.

    Mutations are retried only when the protocol gives them an idempotent
    identity. Unbound submissions and worker polls are deliberately sent once.
    """

    def __init__(
        self,
        base_url: str,
        auth_secret: str,
        *,
        auth_header: str = "X-Worker-Auth",
        timeout_seconds: float = 30.0,
        max_retries: int = 3,
        initial_backoff_seconds: float = 0.1,
        max_backoff_seconds: float = 2.0,
        max_response_bytes: int = _DEFAULT_MAX_RESPONSE_BYTES,
        transport: Transport | None = None,
        sleep: Callable[[float], None] = time.sleep,
        random_source: random.Random | None = None,
    ) -> None:
        parsed = urllib.parse.urlsplit(base_url)
        if parsed.scheme not in {"http", "https"} or not parsed.netloc:
            raise ValueError("base_url must be an absolute http or https URL")
        if parsed.username is not None or parsed.password is not None:
            raise ValueError("base_url must not contain user information")
        if parsed.query or parsed.fragment:
            raise ValueError("base_url must not contain a query or fragment")
        if not auth_secret or "\n" in auth_secret or "\r" in auth_secret:
            raise ValueError("auth_secret must be a non-empty single-line value")
        if not auth_header or any(character.isspace() for character in auth_header):
            raise ValueError("auth_header must be a valid single token")
        if timeout_seconds <= 0:
            raise ValueError("timeout_seconds must be positive")
        if max_retries < 0:
            raise ValueError("max_retries must be non-negative")
        if max_response_bytes <= 0:
            raise ValueError("max_response_bytes must be positive")

        base_path = parsed.path.rstrip("/")
        self.base_url = urllib.parse.urlunsplit(
            (parsed.scheme, parsed.netloc, base_path, "", "")
        )
        self.auth_secret = auth_secret
        self.auth_header = auth_header
        self.timeout_seconds = timeout_seconds
        self.max_retries = max_retries
        self.initial_backoff_seconds = max(0.0, initial_backoff_seconds)
        self.max_backoff_seconds = max(self.initial_backoff_seconds, max_backoff_seconds)
        self.max_response_bytes = max_response_bytes
        self.transport = transport or UrllibTransport()
        self._sleep = sleep
        self._random = random_source or random.Random()

    def _request(
        self,
        method: str,
        path: str,
        payload: Mapping[str, Any] | None = None,
        *,
        idempotent: bool,
        lease_sensitive: bool = False,
    ) -> Any:
        if not path.startswith("/"):
            raise ValueError("path must be absolute")
        body = None
        headers = {
            self.auth_header: self.auth_secret,
            "accept": "application/json",
            "user-agent": "oresoftware-durable-worker-python/0.1.0",
        }
        if payload is not None:
            body = json.dumps(payload, separators=(",", ":"), sort_keys=True).encode("utf-8")
            headers["content-type"] = "application/json"

        attempts = 1 + (self.max_retries if idempotent else 0)
        for attempt in range(attempts):
            try:
                response = self.transport.request(
                    method,
                    f"{self.base_url}{path}",
                    headers,
                    body,
                    self.timeout_seconds,
                    self.max_response_bytes,
                )
            except DurableWorkerError:
                raise
            except Exception as error:
                if idempotent and attempt + 1 < attempts:
                    self._sleep(self._backoff(attempt, None))
                    continue
                raise DurableWorkerError(
                    "transport_error",
                    f"durable-worker request failed: {error}",
                    retryable=True,
                ) from error

            decoded = _decode_json(response.body)
            if 200 <= response.status < 300:
                return decoded

            error = _http_error(response.status, decoded, lease_sensitive=lease_sensitive)
            if (
                idempotent
                and attempt + 1 < attempts
                and response.status in _TRANSIENT_STATUSES
                and error.retryable
            ):
                self._sleep(self._backoff(attempt, response.headers.get("retry-after")))
                continue
            raise error

        raise AssertionError("request retry loop exhausted unexpectedly")

    def _backoff(self, attempt: int, retry_after: str | None) -> float:
        if retry_after:
            try:
                return min(self.max_backoff_seconds, max(0.0, float(retry_after)))
            except ValueError:
                pass
        ceiling = min(
            self.max_backoff_seconds,
            self.initial_backoff_seconds * (2**attempt),
        )
        return self._random.uniform(ceiling / 2, ceiling) if ceiling > 0 else 0.0

    def submit_task(self, task: Mapping[str, Any]) -> Mapping[str, Any]:
        return self._request(
            "POST",
            "/api/v1/tasks",
            task,
            idempotent=bool(task.get("idempotencyKey")),
        )

    def submit_run(self, run: Mapping[str, Any]) -> Mapping[str, Any]:
        return self._request(
            "POST",
            "/api/v1/runs",
            run,
            idempotent=bool(run.get("idempotencyKey")),
        )

    def get_run(self, run_id: str) -> Mapping[str, Any]:
        return self._request(
            "GET",
            f"/api/v1/runs/{_segment(run_id)}",
            idempotent=True,
        )

    def signal_run(
        self, run_id: str, signal_name: str, payload: Mapping[str, Any] | None = None
    ) -> Mapping[str, Any]:
        return self._request(
            "POST",
            f"/api/v1/runs/{_segment(run_id)}/signals/{_segment(signal_name)}",
            {"payload": dict(payload or {})},
            idempotent=False,
        )

    def pause_run(self, run_id: str) -> Mapping[str, Any]:
        return self._run_mutation(run_id, "pause")

    def resume_run(self, run_id: str) -> Mapping[str, Any]:
        return self._run_mutation(run_id, "resume")

    def cancel_run(self, run_id: str) -> Mapping[str, Any]:
        return self._run_mutation(run_id, "cancel")

    def _run_mutation(self, run_id: str, operation: str) -> Mapping[str, Any]:
        return self._request(
            "POST",
            f"/api/v1/runs/{_segment(run_id)}/{operation}",
            {},
            idempotent=True,
        )

    def register_worker(self, registration: Mapping[str, Any]) -> Mapping[str, Any]:
        return self._request(
            "POST",
            "/api/v1/workers/register",
            registration,
            idempotent=True,
        )

    def heartbeat_worker(self, worker_id: str, *, drain: bool | None = None) -> Mapping[str, Any]:
        payload: JsonObject = {}
        if drain is not None:
            payload["drain"] = drain
        return self._request(
            "POST",
            f"/api/v1/workers/{_segment(worker_id)}/heartbeat",
            payload,
            idempotent=True,
        )

    def poll_worker(self, worker_id: str, *, wait_ms: int = 30_000) -> Mapping[str, Any]:
        if wait_ms < 0:
            raise ValueError("wait_ms must be non-negative")
        return self._request(
            "POST",
            f"/api/v1/workers/{_segment(worker_id)}/poll?waitMs={wait_ms}",
            {},
            idempotent=False,
        )

    def start_step(self, step_id: str, lease: Mapping[str, Any]) -> Mapping[str, Any]:
        return self._lease_mutation(step_id, "start", lease)

    def heartbeat_step(self, step_id: str, lease: Mapping[str, Any]) -> Mapping[str, Any]:
        return self._lease_mutation(step_id, "heartbeat", lease)

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
        payload = dict(lease)
        payload.update(
            {
                "chunkId": chunk_id,
                "chunk": chunk,
                "stream": stream,
                "finalChunk": final_chunk,
            }
        )
        return self._request(
            "POST",
            f"/api/v1/steps/{_segment(step_id)}/output",
            payload,
            idempotent=True,
            lease_sensitive=True,
        )

    def complete_step(
        self,
        step_id: str,
        lease: Mapping[str, Any],
        result: Mapping[str, Any] | None = None,
    ) -> Mapping[str, Any]:
        payload = dict(lease)
        payload["result"] = dict(result or {})
        return self._request(
            "POST",
            f"/api/v1/steps/{_segment(step_id)}/complete",
            payload,
            idempotent=True,
            lease_sensitive=True,
        )

    def fail_step(
        self,
        step_id: str,
        lease: Mapping[str, Any],
        *,
        code: str,
        message: str,
        retryable: bool,
    ) -> Mapping[str, Any]:
        payload = dict(lease)
        payload.update({"code": code, "message": message, "retryable": retryable})
        return self._request(
            "POST",
            f"/api/v1/steps/{_segment(step_id)}/fail",
            payload,
            idempotent=True,
            lease_sensitive=True,
        )

    def _lease_mutation(
        self, step_id: str, operation: str, lease: Mapping[str, Any]
    ) -> Mapping[str, Any]:
        return self._request(
            "POST",
            f"/api/v1/steps/{_segment(step_id)}/{operation}",
            lease,
            idempotent=True,
            lease_sensitive=True,
        )


def _decode_json(payload: bytes) -> Any:
    if not payload:
        return {}
    try:
        return json.loads(payload.decode("utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise DurableWorkerError(
            "invalid_response",
            "durable-worker returned a non-JSON response",
            retryable=False,
        ) from error


def _http_error(status: int, payload: Any, *, lease_sensitive: bool) -> DurableWorkerError:
    body = payload if isinstance(payload, Mapping) else {}
    message = str(body.get("message") or f"durable-worker returned HTTP {status}")
    if lease_sensitive and status in {404, 409}:
        return LeaseLostError(message, status=status)
    return DurableWorkerError(
        str(body.get("code") or "http_error"),
        message,
        status=status,
        retryable=bool(body.get("retryable", status in _TRANSIENT_STATUSES)),
    )


def _segment(value: str) -> str:
    if not value:
        raise ValueError("path identifier must be non-empty")
    return urllib.parse.quote(value, safe="")


@dataclass(slots=True)
class WorkerConfig:
    worker_id: str
    queues: Sequence[str]
    capabilities: Sequence[str] = ()
    labels: Mapping[str, Any] = field(default_factory=dict)
    slots: int = 1
    ttl_ms: int = 45_000
    poll_wait_ms: int = 30_000
    worker_heartbeat_ms: int = 15_000
    step_heartbeat_ms: int = 15_000
    max_assignments: int | None = None
    idle_sleep_ms: int = 100

    def validate(self) -> None:
        if not self.worker_id:
            raise ValueError("worker_id must be non-empty")
        if not self.queues:
            raise ValueError("at least one queue is required")
        if self.slots <= 0:
            raise ValueError("slots must be positive")
        if self.ttl_ms <= 0:
            raise ValueError("ttl_ms must be positive")
        if self.poll_wait_ms < 0:
            raise ValueError("poll_wait_ms must be non-negative")
        if self.worker_heartbeat_ms <= 0 or self.step_heartbeat_ms <= 0:
            raise ValueError("heartbeat intervals must be positive")
        if self.max_assignments is not None and self.max_assignments < 0:
            raise ValueError("max_assignments must be non-negative")


@dataclass(frozen=True, slots=True)
class WorkerSummary:
    accepted: int
    completed: int
    failed: int
    lease_lost: int


class _MutableSummary:
    def __init__(self) -> None:
        self._lock = threading.Lock()
        self.accepted = 0
        self.completed = 0
        self.failed = 0
        self.lease_lost = 0

    def increment(self, field_name: str) -> None:
        with self._lock:
            setattr(self, field_name, getattr(self, field_name) + 1)

    def snapshot(self) -> WorkerSummary:
        with self._lock:
            return WorkerSummary(
                accepted=self.accepted,
                completed=self.completed,
                failed=self.failed,
                lease_lost=self.lease_lost,
            )


class TaskContext:
    def __init__(
        self,
        client: DurableWorkerClient,
        worker_id: str,
        assignment: Mapping[str, Any],
        lease: _LeaseIdentity,
        cancelled: threading.Event,
    ) -> None:
        self.client = client
        self.worker_id = worker_id
        self.assignment = assignment
        self.cancelled = cancelled
        self._lease = lease
        self._output_sequence = 0
        self._output_lock = threading.Lock()

    @property
    def run_id(self) -> str:
        return str(self.assignment["runId"])

    @property
    def step_id(self) -> str:
        return str(self.assignment["stepId"])

    @property
    def input(self) -> Mapping[str, Any]:
        value = self.assignment.get("input", {})
        return value if isinstance(value, Mapping) else {}

    @property
    def fencing_token(self) -> int:
        return int(self.assignment["fencingToken"])

    def raise_if_cancelled(self) -> None:
        if self.cancelled.is_set():
            raise LeaseLostError("task lease is no longer authoritative")

    def emit(
        self,
        chunk: str,
        *,
        stream: str = "progress",
        chunk_id: str | None = None,
        final_chunk: bool = False,
    ) -> Mapping[str, Any]:
        self.raise_if_cancelled()
        with self._output_lock:
            self._output_sequence += 1
            selected_chunk_id = chunk_id or (
                f"{self.step_id}:{self._lease.lease_generation}:{self._output_sequence}"
            )
        try:
            return self.client.append_step_output(
                self.step_id,
                self._lease.payload(),
                chunk_id=selected_chunk_id,
                chunk=chunk,
                stream=stream,
                final_chunk=final_chunk,
            )
        except LeaseLostError:
            self.cancelled.set()
            raise


def run_worker(
    client: DurableWorkerClient,
    handlers: Mapping[str, Handler],
    config: WorkerConfig,
    *,
    stop_event: threading.Event | None = None,
) -> WorkerSummary:
    """Run a bounded, lease-aware worker loop.

    The function returns after ``stop_event`` is set or ``max_assignments`` have
    been accepted and all accepted handlers have reached a terminal local state.
    """

    config.validate()
    external_stop = stop_event or threading.Event()
    heartbeat_stop = threading.Event()
    summary = _MutableSummary()

    client.register_worker(
        {
            "workerId": config.worker_id,
            "queues": list(config.queues),
            "capabilities": list(config.capabilities),
            "labels": dict(config.labels),
            "slots": config.slots,
            "ttlMs": config.ttl_ms,
            "drain": False,
        }
    )

    worker_heartbeat = threading.Thread(
        target=_worker_heartbeat_loop,
        args=(client, config, heartbeat_stop, external_stop),
        name=f"{config.worker_id}-heartbeat",
        daemon=True,
    )
    worker_heartbeat.start()

    futures: set[Future[None]] = set()
    try:
        with ThreadPoolExecutor(
            max_workers=config.slots,
            thread_name_prefix=f"{config.worker_id}-task",
        ) as executor:
            while True:
                done = {future for future in futures if future.done()}
                for future in done:
                    future.result()
                futures.difference_update(done)

                limit_reached = (
                    config.max_assignments is not None
                    and summary.snapshot().accepted >= config.max_assignments
                )
                if (external_stop.is_set() or limit_reached) and not futures:
                    break
                if external_stop.is_set() or limit_reached:
                    wait(futures, timeout=0.05, return_when=FIRST_COMPLETED)
                    continue
                if len(futures) >= config.slots:
                    wait(futures, timeout=0.05, return_when=FIRST_COMPLETED)
                    continue

                poll = client.poll_worker(config.worker_id, wait_ms=config.poll_wait_ms)
                assignment = poll.get("assignment") if isinstance(poll, Mapping) else None
                if assignment is None:
                    retry_after_ms = int(poll.get("retryAfterMs", config.idle_sleep_ms))
                    external_stop.wait(max(0, retry_after_ms) / 1000)
                    continue
                if not isinstance(assignment, Mapping):
                    raise DurableWorkerError(
                        "invalid_assignment",
                        "poll response contained a non-object assignment",
                    )

                summary.increment("accepted")
                futures.add(
                    executor.submit(
                        _execute_assignment,
                        client,
                        handlers,
                        config,
                        assignment,
                        summary,
                    )
                )
    finally:
        heartbeat_stop.set()
        worker_heartbeat.join(timeout=max(1.0, config.worker_heartbeat_ms / 1000 + 1.0))
        try:
            client.heartbeat_worker(config.worker_id, drain=True)
        except DurableWorkerError:
            pass

    return summary.snapshot()


def _worker_heartbeat_loop(
    client: DurableWorkerClient,
    config: WorkerConfig,
    heartbeat_stop: threading.Event,
    external_stop: threading.Event,
) -> None:
    interval = config.worker_heartbeat_ms / 1000
    while not heartbeat_stop.wait(interval):
        try:
            client.heartbeat_worker(config.worker_id, drain=external_stop.is_set())
        except DurableWorkerError as error:
            if not error.retryable:
                external_stop.set()
                return


def _execute_assignment(
    client: DurableWorkerClient,
    handlers: Mapping[str, Handler],
    config: WorkerConfig,
    assignment: Mapping[str, Any],
    summary: _MutableSummary,
) -> None:
    step_id = str(assignment.get("stepId", ""))
    task_type = str(assignment.get("taskType", ""))
    if not step_id or not task_type:
        summary.increment("failed")
        return

    lease = _LeaseIdentity.from_assignment(config.worker_id, assignment)
    cancelled = threading.Event()
    heartbeat_stop = threading.Event()

    try:
        client.start_step(step_id, lease.payload())
    except LeaseLostError:
        summary.increment("lease_lost")
        return

    heartbeat = threading.Thread(
        target=_step_heartbeat_loop,
        args=(client, step_id, lease, config.step_heartbeat_ms, heartbeat_stop, cancelled),
        name=f"{config.worker_id}-{step_id}-heartbeat",
        daemon=True,
    )
    heartbeat.start()

    context = TaskContext(client, config.worker_id, assignment, lease, cancelled)
    try:
        handler = handlers.get(task_type)
        if handler is None:
            raise WorkerFailure(
                "handler_not_found",
                f"no handler registered for task type {task_type}",
                retryable=False,
            )
        result = handler(context)
        context.raise_if_cancelled()
        client.complete_step(step_id, lease.payload(), dict(result or {}))
        summary.increment("completed")
    except LeaseLostError:
        cancelled.set()
        summary.increment("lease_lost")
    except WorkerFailure as error:
        if _report_failure(
            client,
            step_id,
            lease,
            cancelled,
            code=error.code,
            message=str(error),
            retryable=error.retryable,
        ):
            summary.increment("failed")
        else:
            summary.increment("lease_lost")
    except Exception as error:
        if _report_failure(
            client,
            step_id,
            lease,
            cancelled,
            code="handler_error",
            message=f"{type(error).__name__}: {error}",
            retryable=False,
        ):
            summary.increment("failed")
        else:
            summary.increment("lease_lost")
    finally:
        heartbeat_stop.set()
        heartbeat.join(timeout=max(1.0, config.step_heartbeat_ms / 1000 + 1.0))


def _step_heartbeat_loop(
    client: DurableWorkerClient,
    step_id: str,
    lease: _LeaseIdentity,
    interval_ms: int,
    heartbeat_stop: threading.Event,
    cancelled: threading.Event,
) -> None:
    interval = interval_ms / 1000
    while not heartbeat_stop.wait(interval):
        try:
            client.heartbeat_step(step_id, lease.payload())
        except DurableWorkerError:
            cancelled.set()
            return


def _report_failure(
    client: DurableWorkerClient,
    step_id: str,
    lease: _LeaseIdentity,
    cancelled: threading.Event,
    *,
    code: str,
    message: str,
    retryable: bool,
) -> bool:
    if cancelled.is_set():
        return False
    try:
        client.fail_step(
            step_id,
            lease.payload(),
            code=code,
            message=message,
            retryable=retryable,
        )
        return True
    except LeaseLostError:
        cancelled.set()
        return False
