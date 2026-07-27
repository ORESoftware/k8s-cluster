# Security policy

Report vulnerabilities privately through GitHub private vulnerability reporting once enabled.

Do not open public issues containing credentials, device tokens, Web Push subscription URLs, tenant identifiers, or production payloads.

## Credential handling

Provider credentials must be supplied at runtime through workload identity or Kubernetes External Secrets. Logs, traces, metrics, HTTP responses, and NATS results must redact capability-bearing targets and secrets.
