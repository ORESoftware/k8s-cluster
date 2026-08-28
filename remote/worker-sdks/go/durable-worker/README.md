# ORESoftware durable-worker SDK for Go

A dependency-free Go 1.23+ client and long-lived worker loop for the independent durable-worker runtime in `ORESoftware/k8s-cluster`.

This package is hand-authored execution infrastructure. It is intentionally separate from deterministic generated OpenAPI clients under `remote/api-sdks`.

## Safety contract

Delivery is **at least once**. A task can be delivered again after a process crash, timeout, lost response, lease expiry, or scheduler failover. Every external effect must therefore use either:

- a stable idempotency key; or
- a downstream write guarded by `TaskContext.FencingToken()`.

The SDK renews leases and suppresses terminal writes after fencing, but it cannot make an arbitrary external API exactly once.

The client applies these retry boundaries:

| Operation | Client retry behavior |
| --- | --- |
| task/run submission with `idempotencyKey` | retries transient transport and HTTP failures |
| task/run submission without `idempotencyKey` | exactly one request |
| signal delivery | exactly one request |
| worker long poll | exactly one request; an ambiguous outcome stops the loop and lets the lease expire/redeliver |
| run pause/resume/cancel | retried as idempotent state mutations |
| worker registration/heartbeat | retried |
| step start/heartbeat/output/complete/fail | retried with lease identity; HTTP 404/409 becomes `LeaseLostError` |

Redirects are never followed, authorization is sent only in the configured header, JSON responses are bounded, and retry sleeps respect caller cancellation.

## Client

```go
package main

import (
    "context"
    "log"
    "os"

    durableworker "github.com/oresoftware/k8s-cluster/remote/worker-sdks/go/durable-worker"
)

func main() {
    client, err := durableworker.NewClient(
        os.Getenv("DURABLE_WORKER_URL"),
        os.Getenv("DURABLE_WORKER_AUTH_SECRET"),
        durableworker.ClientOptions{},
    )
    if err != nil {
        log.Fatal(err)
    }

    accepted, err := client.SubmitTask(context.Background(), durableworker.JSON{
        "idempotencyKey": "invoice:inv-123:render:v1",
        "taskType":      "invoice:render",
        "queue":         "documents",
        "input":         durableworker.JSON{"invoiceId": "inv-123"},
    })
    if err != nil {
        log.Fatal(err)
    }
    log.Printf("run accepted: %#v", accepted)
}
```

## Worker

```go
summary, err := durableworker.RunWorker(ctx, client, map[string]durableworker.Handler{
    "invoice:render": func(task *durableworker.TaskContext) (durableworker.JSON, error) {
        select {
        case <-task.Context().Done():
            return nil, task.RaiseIfCancelled()
        default:
        }

        _, err := task.Emit("rendering", durableworker.OutputOptions{})
        if err != nil {
            return nil, err
        }

        // Pass this token to a fencing-aware downstream write. A lower token
        // must never overwrite data written under a higher token.
        fencingToken := task.FencingToken()
        _ = fencingToken

        return durableworker.JSON{"documentId": "doc-123"}, nil
    },
}, durableworker.WorkerConfig{
    WorkerID:        "invoice-worker-1",
    Queues:          []string{"documents"},
    Capabilities:    []string{"invoice:render"},
    Slots:           4,
    MaxAssignments:  0, // unbounded
})
```

`RunWorker` only polls when a local slot is available. Cancellation stops new polls but drains work already accepted. `MaxAssignments` enables a deterministic bounded worker for jobs, serverless containers, and tests.

`WorkerSummary.ProtocolErrors` is intentionally distinct from `Failed`: it means a terminal protocol operation remained ambiguous after safe retries, not that the durable server recorded a failed task.

## Failure classification

Return `*WorkerFailure` to control the durable error code and retryability:

```go
return nil, &durableworker.WorkerFailure{
    Code:      "upstream_busy",
    Message:   "document service is saturated",
    Retryable: true,
}
```

An unregistered task type becomes `handler_not_found` and a recovered panic becomes `handler_panic`; both are non-retryable by default. A lost heartbeat cancels the handler context and prevents completion or failure under the stale lease generation.

## Validation

```bash
go test ./... -count=1
go test ./... -race -count=1
go vet ./...
```

The package also consumes `remote/worker-sdks/fixtures/durable-worker-protocol-v1.json`, shared with the TypeScript and Python conformance ratchet.
