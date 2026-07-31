# External queue ingress through `dd-nats-bridge`

Linear: DEN-440

## Boundary

NATS is an internal Kubernetes implementation detail. A server outside the
cluster must not receive NATS credentials or network access, import a NATS
client library, configure `NATS_URL`/subjects/streams, or dial TCP 4222.
External producers submit queue messages over HTTPS to the Rust
`remote/nats-bridge` deployment.

In-cluster consumers and explicitly reviewed in-cluster producers may continue
to use NATS directly while account/nkey authorization is rolled out. This does
not authorize raw NATS access from another cloud, Vercel, a developer machine,
a standalone VM, Apps Script, or a third-party worker platform.

## HTTP contract

```http
POST /v1/queues/vapi-task HTTP/1.1
Authorization: Bearer <client-scoped-token>
X-Bridge-Client: external-vapi
Idempotency-Key: example-request-0001
Content-Type: application/json

{"call_id":"...","destination":"..."}
```

Successful durable acceptance returns `202`:

```json
{
  "ok": true,
  "route": "vapi-task",
  "message_id": "example-request-0001"
}
```

The external contract never contains a NATS subject, stream, server URL,
credential, JetStream API detail, or upstream error string.

## Route and client authorization

Routes are non-secret and mounted from `dd-nats-bridge-routes`. Each route maps
to one exact subject and one expected JetStream stream. Wildcards, `$SYS`,
`$JS`, arbitrary subject suffixes, and core-NATS fallback are not permitted.

Client configuration is secret JSON mounted at
`/var/run/secrets/nats-bridge/clients.json` from the
`BRIDGE_CLIENTS_JSON` key in `dd/messaging/nats-bridge-secrets`:

```json
{
  "external-vapi": {
    "token": "<redacted>",
    "routes": ["vapi-task"]
  }
}
```

The redacted value above is not valid configuration. Generate at least 32 random
bytes directly in the secret manager and never commit the result. Tokens must be
unique. Rotate one client independently by changing its token, waiting for
External Secrets and the Deployment rollout, updating the producer, and removing
the old credential. Never paste client JSON or token values into Git, Linear,
logs, shell history, support tickets, or chat.

## Durability and idempotency

Every accepted request must include an 8-128 byte `Idempotency-Key` containing
only ASCII letters, digits, `-`, `_`, `.`, or `:`. The bridge namespaces it by
client and route and sends it as `Nats-Msg-Id`. It also sends
`Nats-Expected-Stream`, so a missing or incorrect stream fails the request
instead of silently publishing through core NATS.

The bridge returns success only after the JetStream acknowledgement arrives.
Timeout, no-stream, and publish failures return stable 5xx error codes without
raw NATS details. Clients may retry the same request with the same idempotency
key.

## Public ingress activation gates

`nats-bridge.ingress.template.yaml` is intentionally absent from the messaging
Kustomization. Activate it only in a dedicated PR after all of the following:

1. choose a stable hostname and provision a trusted TLS certificate;
2. seed `BRIDGE_CLIENTS_JSON` and verify the ExternalSecret is Ready;
3. deploy the hardened binary and verify `/healthz`, `/readyz`, and `/metrics`
   from inside the cluster;
4. add ingress-controller namespace access to the bridge NetworkPolicy while
   preserving the direct-NATS NetworkPolicy boundary;
5. replace the invalid template hostname and TLS secret name;
6. retain HTTPS redirect, 1 MiB ingress cap, 10 rps limit, and bounded timeouts;
7. run negative tests for no token, wrong client, wrong route, missing/invalid
   idempotency key, invalid JSON, oversized body, no stream, timeout,
   unauthorized subject injection, and duplicate retry behavior;
8. confirm access/error logs never contain Authorization, client tokens,
   payloads, query strings, internal subjects, or stream names;
9. exercise an external producer with a harmless test route before enabling a
   production queue.

Do not expose `/publish/:subject` externally.
Do not expose NATS ports 4222/6222/8222/7777 through this ingress. The deployed
binary serves only `/healthz`, `/readyz`, `/metrics`, and named
`/v1/queues/:route` requests.

## External-server migration procedure

For each external producer:

1. inventory the current NATS dependency, environment variables, credentials,
   subjects, retry semantics, message schema, and deployment location;
2. create a named route and a client-specific route grant;
3. add a small HTTPS client with explicit connect/request timeouts, bounded
   retries, TLS verification, and one stable idempotency key per logical
   operation;
4. shadow or dual-write only when duplicate side effects are safely deduplicated;
5. verify JetStream consumer processing and tracing end-to-end;
6. remove the NATS library, `NATS_*` variables, subjects, credentials, and port
   4222 access from the external repository and deployment;
7. add CI assertions preventing direct NATS from returning;
8. rotate/remove the old NATS credential and network permission.

A migration is incomplete while the external server can still connect directly
to NATS, even if the HTTP path also exists. Record each remaining producer
migration as a child of DEN-440, and keep credential/ingress activation separate
from source-only client migrations.
