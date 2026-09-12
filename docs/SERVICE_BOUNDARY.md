# Web/API boundary deployment contract

This platform rule applies to traditional browser web services and JSON APIs
deployed through Argo CD. It does not prescribe distributed-node protocols.
Application repositories own behavior; this repository owns ingress,
NetworkPolicy, TLS, Argo desired state, and deployment sequencing.

| Connection | Platform requirement |
| --- | --- |
| Direct database read | allow only a scoped read principal and explicit egress; never publish a database service through ingress |
| Stateless HTTP/JSON | default web-to-API path: ClusterIP API, readiness-gated ingress only for intended public hosts, request timeout and trace propagation |
| Stateful TCP | explicit mTLS/service policy, bounded WebSocket/gRPC timeouts, reconnect/health behavior, and per-service NetworkPolicy |
| NATS/MQ | outbox consumers use internal subjects, idempotent event IDs, dead-letter/observability policy; no public broker exposure |

Create the API/Web Service, NetworkPolicy, health probes, TLS certificate, and
host allowlist before creating edge DNS. Argo applies reviewed desired state;
CI validates manifests but never performs imperative production deployment.
