# AI agent bridge Prometheus contract

The central `dd-prometheus` workload scrapes the AI bridge at:

```text
http://dd-ai-agent-bridge.default.svc.cluster.local:8142/metrics
```

The source endpoint is pinned by the `remote/deployments/ai-agent-bridge` gitlink to merged bridge revision `234a3c640bba688b69fb37f9f1e72daa6fbf8b1f`. Prometheus is not permitted to use bridge TCP port `8143`.

## Network boundary

`dd-ai-agent-bridge.networkpolicy.yaml` allows scrape ingress only from pods that satisfy both:

- namespace label `kubernetes.io/metadata.name=observability`;
- pod label `app=dd-prometheus`.

The metrics endpoint is intentionally unauthenticated like `/healthz` and `/readyz`; NetworkPolicy is therefore part of the security contract. Application HTTP and TCP APIs remain protected by bridge authentication.

## Alert contract

The observability rules cover:

- missing and down scrape targets;
- bounded-resource capacity above 80%;
- HTTP admission rejection under capacity pressure;
- shed best-effort persistence writes;
- conflicting, owner-mismatched, or stale-fencing lease events;
- server/transport errors from a configured external control plane.

The control-plane alert uses `and on()` to combine the dependency-configured gauge with the aggregate error signal without relying on incompatible label sets.

## Cardinality and privacy

Scrape and alert labels are limited to fixed service, severity, result, reason, dependency, persistence mode, and resource enums. They do not include prompts, model output, messages, channels, agent keys, providers, models, repositories, paths, workflow IDs, request IDs, credentials, or user identifiers.

## Operational evidence

Committed configuration is validated with focused Node tests, Kustomize rendering for both observability and runtime overlays, and `promtool` from Prometheus 2.55.1. Live target evidence remains a deployment gate under DEN-845: DEN-680 is not complete until central Prometheus reports `up{job="dd-ai-agent-bridge"} == 1` for the controlled rollout.

Provider-runner metrics are separate under DEN-1227 and must not be inferred from bridge metrics.
