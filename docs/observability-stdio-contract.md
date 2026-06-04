# Observability, Stdio, And Telemetry Contract

This repo collects telemetry from two stable boundaries:

- Kubernetes container stdout/stderr, collected by Promtail and stored in Loki.
- Explicit metrics and spans, collected by Prometheus and the OpenTelemetry Collector.

Do not make observability depend on runtime monkey-patching. Auto-instrumentation, require hooks,
fetch/http/client patches, stream replacement, framework patches, Erlang process interception, and
similar hidden behavior are not allowed in first-party services.

## Log Collection

Every service may write ordinary human-readable lines to stdout/stderr. Services that need stronger
correlation should write one compact JSON object per line using this envelope:

```json
{
  "schema": "dd.log.v1",
  "time_unix_nano": "1780000000000000000",
  "severity_text": "INFO",
  "severity_number": 9,
  "body": "task queued",
  "resource_service_name": "dd-dev-server-api",
  "resource_service_namespace": "remote-dev",
  "scope_name": "dev-server",
  "event_name": "agent.task.queued",
  "trace_id": "0123456789abcdef0123456789abcdef",
  "span_id": "0123456789abcdef",
  "attributes": {
    "dd.request.id": "req-123",
    "dd.request.thread_id": "thread-uuid",
    "dd.request.task_id": "task-uuid",
    "stream": "stdout"
  }
}
```

Field mapping:

- `schema`: literal `dd.log.v1`.
- `time_unix_nano`: string nanoseconds since Unix epoch, matching OTLP timestamp encoding.
- `severity_text` / `severity_number`: OpenTelemetry log severity fields. Use the usual OTEL
  numbers when practical (`INFO` around `9`, `WARN` around `13`, `ERROR` around `17`).
- `body`: short message text. Keep it useful after redaction.
- `resource_service_name`: maps to OTel resource attribute `service.name`.
- `resource_service_namespace`: maps to OTel resource attribute `service.namespace`.
- `scope_name`: maps to OTel instrumentation scope name or local logger/module name.
- `event_name`: stable low-cardinality event name.
- `trace_id` / `span_id`: optional lowercase hex IDs when the service has an explicit span.
- `attributes`: JSON object for request/task/provider/runtime details. Do not put secrets here.

Promtail may promote only low-cardinality fields such as `resource_service_name`, `severity_text`,
and `schema` to Loki labels. Request ids, task ids, thread ids, user ids, trace ids, span ids,
container ids, raw paths with ids, and error messages must remain log fields, not stream labels.

## Runtime Guidance

Node.js:

- Use explicit log helper functions or explicit process events.
- It is acceptable to listen with `process.on("warning", ...)` or a service-owned
  `process.on("info", ...)` event if libraries intentionally emit those events.
- Do not replace `console`, `process.stdout`, `process.stderr`, module loading, `fetch`, `http`,
  timers, or framework methods.

Rust:

- Prefer `tracing` and `tracing_subscriber` with explicit formatting.
- Use JSON formatting where service code can support it without broad refactors; otherwise keep
  clear text logs and metrics.
- Replace direct `println!` / `eprintln!` only when the service owner is already touching that code.

Java, Scala, and Spark:

- Prefer Logback or Log4j configuration and appenders.
- Keep stdout/stderr as the container boundary; do not weave, attach agents, or patch runtime
  classes unless a separate human-reviewed design approves it.

Erlang and Gleam:

- Use explicit `logger`, `io`, or owned actor messages for logs and metrics.
- Do not intercept process mailboxes, patch OTP internals, or wrap unrelated processes for telemetry.

Go, Python, Dart, and F#:

- Use the local runtime's explicit logger or stdout/stderr writes.
- Emit the shared JSON envelope only where it is straightforward and useful.

## Child Process Stdio

When a service launches a child process, collect stdout and stderr separately and preserve the
stream name. Prefer line-oriented buffers with bounded maximum line size. If the child emits a JSON
line with `schema: "dd.log.v1"`, forward it as-is after redaction; otherwise wrap or store it as
plain text with `stream` metadata.

Stdio collectors must not swallow child output silently, let an unbounded line exhaust memory,
or merge stdout and stderr in a way that loses error provenance.

## Metrics And Traces

Metrics remain Prometheus-first for most runtimes. Expose `/metrics` and add the service to the
collector or Prometheus scrape list.

Traces are explicit-only. A service may emit OTLP spans directly or through a normal SDK configured
without auto-instrumentation. First-party code should stamp service metadata, request ids, task ids,
thread ids, provider names, and explicit errors on spans where those values already exist.

OTLP logs are optional. The durable baseline is stdout/stderr to Promtail/Loki, with the JSON
envelope above for services that need cross-runtime correlation.

## Critical Events

Critical operational failures may also be published to NATS on `dd.remote.events.critical`, exposed
to services as `NATS_CRITICAL_EVENT_SUBJECT`. The subject is JetStream-backed by
`DD_REMOTE_CRITICAL_EVENTS`, and `dd-remote-queue-consumer` runs a durable logger loop that consumes
the stream, writes compact `dd.log.v1` stderr records, and acknowledges the critical events. Payloads
should remain compact and include a `dd.log.v1`-compatible record or fields that map directly to it.
This channel is for alert-worthy runtime failures such as invalid queue payloads, lost
acknowledgements, dispatch failures, or receipt/idempotency failures. Routine lifecycle and
task-status events stay on `dd.remote.events`.

Do not include secrets, raw prompts, raw auth headers, raw model responses, or high-cardinality
labels in the NATS critical payload. Put request/task/thread identifiers in JSON fields, not in the
subject name.
