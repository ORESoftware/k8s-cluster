# ORESoftware durable-worker Python SDK

A dependency-free Python 3.11+ client and long-lived worker loop for `dd-durable-worker-server`.

The SDK preserves the runtime's **at-least-once** contract. External side effects must use their own idempotency keys or reject stale writes using `TaskContext.fencing_token`.

## Client

```python
from oresoftware_durable_worker import DurableWorkerClient

client = DurableWorkerClient(
    "https://durable-workers.internal",
    auth_secret="mounted-secret",
)

run = client.submit_task({
    "idempotencyKey": "invoice-42:v1",
    "taskType": "invoice.render",
    "queue": "documents",
    "input": {"invoiceId": "42"},
})
```

Automatic retries are deliberately narrow:

- reads and idempotent lease mutations may retry transient failures;
- task/DAG submission retries require `idempotencyKey`;
- worker polling is sent once because a lost response may already contain an authoritative lease;
- redirects are refused so the worker secret cannot be forwarded to another origin.

## Worker

```python
from oresoftware_durable_worker import (
    DurableWorkerClient,
    TaskContext,
    WorkerConfig,
    run_worker,
)

client = DurableWorkerClient("http://dd-durable-worker-server:8152", "mounted-secret")

def render_invoice(context: TaskContext):
    context.emit("loading invoice")
    # Guard downstream writes with context.fencing_token or an idempotency key.
    context.raise_if_cancelled()
    return {"documentKey": "invoices/42.pdf"}

summary = run_worker(
    client,
    {"invoice.render": render_invoice},
    WorkerConfig(
        worker_id="python-documents-1",
        queues=["documents"],
        capabilities=["pdf"],
        slots=4,
    ),
)
```

The worker owns registration, worker TTL heartbeats, independent step lease heartbeats, local slot admission, progress chunk identities, draining, and stale-terminal suppression. A fenced heartbeat marks the handler context cancelled and prevents completion/failure from being sent under the stale lease.

Raise `WorkerFailure(code, message, retryable=...)` to classify expected handler failures. Unhandled exceptions are reported as non-retryable `handler_error` failures.

## Tests

```bash
PYTHONPATH=src python -m unittest discover -s tests -v
python -m compileall -q src tests examples
```
