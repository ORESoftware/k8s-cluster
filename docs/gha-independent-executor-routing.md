# Independent CI executor routing across AWS and Hetzner

Issue: DEN-1597  
Parent: DEN-1550  
Capacity and ARC policy: DEN-1549

## Purpose

`gha-clone-server-rs` already provides the independent, fail-closed half of the
GitHub Actions continuity architecture. It compiles a bounded workflow subset to
fixed `dd-build-server` profiles and never forwards caller-selected shell,
action code, runner images, or Kubernetes manifests.

`gha-executor-router` is a transparent placement layer for those fixed-profile
requests. It sits between `gha-clone-server-rs` and one or more reviewed
`dd-build-server` installations:

```text
GitHub hosted / ARC native parity lane
                │
                └── unchanged GitHub Actions orchestration

workflow_run failure webhook / operator API
                │
                ▼
       gha-clone-server-rs
  immutable SHA + fixed profile only
                │
                ▼
       gha-executor-router
    readiness-based placement only
         │                  │
         ▼                  ▼
 AWS dd-build-server   Hetzner dd-build-server
```

The router does not parse workflow YAML, execute repository commands, choose a
profile, build an image, deploy Kubernetes resources, create a GitHub check, or
claim parity with GitHub's proprietary control plane.

## No-duplicate failover boundary

Provider failover is safe only **before** a submission is attempted.

For a new request, the router probes the configured executors in reviewed order.
An executor whose `/readyz` endpoint is not successful is skipped before any
`POST /builds`. This permits AWS-to-Hetzner placement when AWS is already known
to be unavailable.

Once the router attempts `POST /builds`, it does not submit the request to a
second provider unless a future shared coordination protocol can prove the first
provider did not accept it. A connection reset, timeout, HTTP 429, HTTP 5xx,
redirect, oversized response, or invalid accepted response is an ambiguous
outcome: the first build server may have persisted or started the deterministic
request even though the response was lost. Blindly trying the second provider
could execute the same commit twice.

The current response therefore fails closed and instructs an operator or future
reconciler to resolve the unchanged deterministic `requestId` through
`dd-build-server`/Fiducia state. Safe automatic cross-provider resubmission after
an attempted POST requires all of:

1. one shared Fiducia idempotency namespace and fenced authoritative claim;
2. durable request-to-provider assignment visible from both clouds;
3. durable job state that can distinguish unaccepted from accepted-but-unseen;
4. shared or replicated artifact addressing; and
5. a tested takeover protocol that rejects stale fencing tokens.

## Status pinning

An accepted upstream ID is returned as:

```text
<executor-id>~<upstream-build-id>
```

Every subsequent `GET /builds/<id>` is routed only to the accepting executor.
Status failures do not trigger a second submission or switch providers. The
executor ID and provider are included in successful responses and bounded
metrics, but endpoint URLs and credentials are not exposed.

The original build-server `requestId` is forwarded byte-for-byte. This retains
the existing build-server Postgres/Fiducia idempotency contract.

## Request boundary

The router accepts only the build-server fixed-profile subset emitted by the
clone server:

- `schemaVersion=build-server.v1`;
- `jobKind=run-profile`;
- a GitHub HTTPS repository URL;
- a lowercase full 40-hex commit SHA;
- a bounded lowercase fixed profile name; and
- a bounded deterministic request ID.

It rejects image, deployment, executor, Dockerfile, build-argument, context,
and other caller-selected fields even though the broader build server supports
some of them for its separate authenticated operator API.

## Configuration and secret isolation

`GHA_EXECUTOR_ROUTER_EXECUTORS_JSON` is an ordered array. An enabled entry has:

```json
{
  "id": "aws-primary",
  "provider": "aws",
  "enabled": true,
  "url": "http://dd-build-server.default.svc.cluster.local:8100",
  "authPath": "/var/run/secrets/gha-executor-router/aws-build-server-auth"
}
```

A disabled entry must omit `url` and `authPath`, preventing dormant endpoints or
credential paths from silently drifting. Provider names are limited to `aws`
and `hetzner`; IDs, URLs, and authentication paths must be unique.

Secrets are read from absolute direct-child files beneath
`GHA_EXECUTOR_ROUTER_SECRET_ROOT`. They are never accepted inline in the JSON,
command-line arguments, URLs, logs, metrics, or API responses. Mounted secrets
must be bounded, non-empty, single-line values without NUL, CR, or LF. Production
uses Kubernetes Secret/ExternalSecret files.

Plain HTTP origins are limited to loopback and `.svc.cluster.local`;
cross-cloud endpoints require HTTPS. The outbound HTTP client does not follow
redirects, so an authenticated request cannot be redirected to another host.

The inbound clone-server-to-router credential is separate from the router's AWS
and Hetzner build-server credentials.

## API

- `GET /healthz` — process/configuration state without network probing.
- `GET /readyz` — successful while inert; when execution is enabled, requires at
  least one enabled executor whose `/readyz` succeeds.
- `GET /v1/capabilities` — explicit routing and no-duplicate boundary.
- `GET /metrics` — bounded counters with only `aws` and `hetzner` provider
  labels.
- `POST /builds` — authenticated fixed-profile submission.
- `GET /builds/<executor~job>` — authenticated status from the accepting
  executor only.

## Test contract

The process-level suite starts the real compiled router and two local Axum build
server doubles. It proves:

- AWS is used first when ready and Hetzner receives no POST;
- AWS readiness failure selects Hetzner before submission;
- explicit AWS request rejection does not fall through;
- ambiguous AWS submission does not fall through and upstream bodies are not
  leaked;
- accepted AWS status remains pinned even when polling fails;
- request IDs and provider-specific auth are forwarded exactly;
- arbitrary build/deploy fields are rejected;
- disabled execution is inert while readiness remains healthy; and
- malformed duplicate configuration exits before binding and without printing
  secrets.

Unit tests cover URL, path, secret, executor identity, immutable request, route
ID, constant-time authentication, redirect refusal, and multiline-secret
rejection.

## Exact branch evidence

The router implementation and refreshed lockfile were produced on commit
`0ae1368f99ab9fb7b4ae5b55286ebd752bdbc561` after a pinned Rust 1.90 job ran:

```text
cargo check --all-targets
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets -- --nocapture
```

The tested suite completed with:

- 43 library tests;
- 3 router-binary tests;
- 7 dual-provider router process tests;
- 5 meta HTTP integration tests; and
- 7 workflow-run webhook HTTP integration tests.

The process tests exercise the real compiled router, not only pure planning
functions. The final self-removing workflow run `30872370938` completed
successfully and committed the tested source, lockfile, redirect protection,
multiline-secret rejection, and removal of temporary repair workflows. A normal
reviewed documentation commit then retriggers the permanent continuity workflow
on the same implementation for the PR merge gate.

## Activation sequence

The code PR is safe to merge before infrastructure exists. GitOps activation is
a separate reviewed change and must keep the initial deployment at zero replicas
and execution disabled.

1. Merge and publish an immutable router image with SBOM, provenance, and
   vulnerability evidence.
2. Provision a dedicated inbound router credential and independent AWS/Hetzner
   build-server credentials through External Secrets.
3. Start the router with only AWS enabled and run plan/status tests.
4. Route the clone server's build-server URL to the router while clone-server
   execution remains disabled.
5. Enable clone-server API execution for one immutable meta fixture and verify
   the AWS route.
6. Provision a Hetzner fixed-profile build server with the same reviewed profile
   contracts and a separate credential.
7. Add and enable the Hetzner executor through a reviewed GitOps change.
8. Prove AWS-down-before-submit selects Hetzner and AWS-accepted-then-partitioned
   remains pinned to AWS.
9. Prove artifacts and logs remain provider-addressable, then run rollback.
10. Only after shared Fiducia-fenced assignment exists may automatic takeover
    after ambiguous submission be considered.

## Billing evidence

Recent ORESoftware hosted Ubuntu jobs received runners and executed real steps,
so GitHub-hosted Actions are not currently globally unavailable. This does not
establish a numeric included-minute balance. The exact current-month value is
owned by the separate billing-read GitHub App and `/usage/summary` contract in
DEN-1549; if that read is unavailable, policy fails to certified self-hosted
capacity or hold rather than inventing a number.
