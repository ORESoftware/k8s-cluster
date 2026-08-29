# ORESoftware durable-worker Rust SDK

An async Rust 1.85+ client and bounded long-lived worker loop for `dd-durable-worker-server`.

The runtime delivers work **at least once**. External effects must use a stable idempotency key or reject stale writes with `TaskContext::fencing_token()`.

## Client

```rust
use oresoftware_durable_worker::{Client, ClientOptions, JsonObject};
use serde_json::json;

# async fn example() -> Result<(), Box<dyn std::error::Error>> {
let client = Client::new(
    "https://durable-workers.internal",
    "mounted-secret",
    ClientOptions::default(),
)?;
let mut task = JsonObject::new();
task.insert("idempotencyKey".into(), json!("invoice-42:v1"));
task.insert("taskType".into(), json!("invoice.render"));
task.insert("queue".into(), json!("documents"));
task.insert("input".into(), json!({"invoiceId":"42"}));
let run = client.submit_task(task).await?;
# let _ = run;
# Ok(())
# }
```

Retry boundaries are intentionally narrow:

- task and DAG submission retry only when `idempotencyKey` is present and non-empty;
- worker polling and signals are sent once because a lost response can already contain an authoritative lease or accepted signal;
- lease mutations may retry because worker ID, lease token, and lease generation identify the operation;
- redirects are disabled in `ReqwestTransport`, preventing the worker credential from being forwarded to another origin;
- response bodies are read incrementally and rejected above the configured limit.

`Transport` is public so tests and specialized runtimes can supply their own HTTP implementation. The default `ReqwestTransport` supports HTTP and HTTPS through rustls.

## Worker

```rust
use oresoftware_durable_worker::{
    Cancellation, Client, ClientOptions, Handler, JsonObject, TaskContext,
    Worker, WorkerConfig,
};
use serde_json::json;
use std::{collections::HashMap, sync::Arc};

# async fn example() -> Result<(), Box<dyn std::error::Error>> {
let client = Arc::new(Client::new(
    "http://dd-durable-worker-server:8152",
    "mounted-secret",
    ClientOptions::default(),
)?);
let render: Handler = Arc::new(|context: TaskContext| Box::pin(async move {
    context.emit("loading invoice", "progress", false).await?;
    context.check_cancelled()?;
    let mut result = JsonObject::new();
    result.insert("documentKey".into(), json!("invoices/42.pdf"));
    Ok(result)
}));
let worker = Worker::new(
    client,
    HashMap::from([("invoice.render".into(), render)]),
    WorkerConfig {
        worker_id: "rust-documents-1".into(),
        queues: vec!["documents".into()],
        capabilities: vec!["pdf".into()],
        slots: 4,
        ..WorkerConfig::default()
    },
)?;
let summary = worker.run(Cancellation::default()).await?;
# let _ = summary;
# Ok(())
# }
```

The worker owns registration, bounded local slot admission, worker TTL heartbeats, independent step heartbeats, deterministic progress chunk identities, drain signaling, and stale-terminal suppression. A failed or fenced step heartbeat cancels the task context; after cancellation the worker sends neither completion nor failure under that stale generation.

Raise `WorkerFailure::new(code, message, retryable)` for expected handler failures. Keep downstream writes idempotent or fenced even when the handler itself is deterministic.

## Verification

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
```

The repository workflow runs both the declared MSRV and current stable Rust, consumes the shared protocol fixture, repeats the lease-fencing cancellation test, audits dependency duplication, scans credential-shaped source, and publishes a deterministic source archive plus SHA-256 checksum after merge to `dev`.
