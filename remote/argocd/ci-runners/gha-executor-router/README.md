# GHA executor router activation package

This directory is an **inert, digest-gated** Kubernetes package for DEN-1597. It is intentionally absent from active AWS and Hetzner overlays. The Rust service lives in `remote/deployments/gha-clone-server-rs/src/bin/gha-executor-router.rs` and preserves the authenticated `dd-build-server` API expected by `gha-clone-server-rs`.

## What this component is—and is not

The router is not a GitHub Actions control-plane clone and does not parse workflow YAML. Native GitHub Actions compatibility remains the responsibility of official Actions Runner Controller scale sets. The independent Rust lane accepts only the already-reviewed `build-server.v1` fixed-profile requests produced by `gha-clone-server-rs`.

The router owns one narrow decision: which reviewed `dd-build-server` endpoint receives a new deterministic request. It never accepts a caller-selected provider, URL, command, image, runner label, or Kubernetes object.

## Non-duplication invariant

Provider failover is allowed only before an executor has returned HTTP `202 Accepted`.

- connection failures, HTTP 429, and HTTP 5xx may advance from AWS to Hetzner;
- HTTP 4xx is a fixed-contract rejection and fails closed;
- a valid HTTP 202 creates one retained, router-namespaced route;
- an unreadable or malformed HTTP 202 is ambiguous acceptance evidence and is retained rather than retried;
- every later status poll is pinned to the accepting provider;
- polling failure never causes submission to another provider;
- the original deterministic `requestId` is forwarded unchanged and duplicate submissions return the same route.

Cross-provider resumption after acceptance is not enabled. It requires a shared Fiducia-fenced claim, durable shared route/job state, and a shared artifact policy.

## Credentials

Three authorities remain separate:

1. inbound clone-server-to-router authentication;
2. AWS router-to-build-server authentication;
3. Hetzner router-to-build-server authentication.

The ExternalSecrets use separate backing records and mount each value through a distinct read-only file beneath `/var/run/gha-executor-router`. Never put a classic PAT, GitHub App key, or inline build-server secret in the ConfigMap, Deployment, workflow, logs, or Linear.

## Activation checklist

Keep `replicas: 0`, `GHA_EXECUTOR_ROUTER_EXECUTION_ENABLED=false`, and the image digest placeholder until all checks below have recorded evidence:

- [ ] build and sign a dedicated non-root image containing `/usr/local/bin/gha-executor-router`;
- [ ] replace the image placeholder with an immutable digest and retain SBOM, provenance, and vulnerability results;
- [ ] provision the three backing secrets through the approved secret store;
- [ ] replace `build-server.hetzner.example.invalid` with the exact reviewed HTTPS endpoint;
- [ ] confirm the AWS endpoint is the existing fixed-profile `dd-build-server` and that both providers accept the same deterministic request schema;
- [ ] run AWS-first, Hetzner-failover, HTTP-4xx fail-closed, duplicate-request, and pinned-polling smokes;
- [ ] verify artifacts and logs have an explicit provider-independent retention policy;
- [ ] run a provider-loss drill before acceptance and a separate poll-loss drill after acceptance;
- [ ] add this package to the intended Argo CD overlay only after review;
- [ ] point `GHA_CLONE_BUILD_SERVER_URL` at `http://gha-executor-router.gha-continuity.svc.cluster.local:8126` only after the router is ready;
- [ ] enable router execution before clone-server execution, then scale each component to one replica;
- [ ] verify metrics and rollback; do not horizontally scale the in-memory router before Fiducia-backed durable claims exist.

## Rollback

1. disable `GHA_CLONE_WEBHOOK_EXECUTION_ENABLED` and `GHA_CLONE_EXECUTION_ENABLED`;
2. stop new router submissions with `GHA_EXECUTOR_ROUTER_EXECUTION_ENABLED=false`;
3. allow accepted provider-pinned work to finish or reconcile it directly with that provider;
4. scale the router and clone server to zero;
5. restore the clone server's direct AWS build-server URL only after confirming no retained route can be duplicated;
6. rotate any credential involved in the incident and preserve route/metric evidence.

Do not force a post-acceptance job onto the other provider during rollback.
