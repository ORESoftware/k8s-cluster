# GHA independent executor routing

Linear: DEN-1550  
Last reviewed: 2026-08-03

## Scope

`gha-executor-router` is the stateless routing boundary between
`gha-clone-server-rs` and the existing fixed-profile `dd-build-server`
executors. It provides continuity for the independent workflow lane when
GitHub-hosted runner allocation is unavailable.

It does not replace GitHub's workflow control plane. Actions Runner Controller
(ARC) remains the native-parity lane for expressions, matrices, marketplace
actions, checks, artifacts, caches, environments and other GitHub-owned
semantics. The router accepts only the independent lane's already validated
`jobKind=run-profile` requests.

## Routing contract

Executor inventory is supplied through `GHA_EXECUTOR_ROUTES_JSON`. The value is
secret-managed because it can contain private service origins, but it must not
contain credentials. Each route refers to a separate environment variable via
`authEnv`.

Example shape:

```json
[
  {
    "name": "aws-primary",
    "provider": "aws",
    "url": "http://dd-build-server.default.svc.cluster.local:8100",
    "authEnv": "GHA_EXECUTOR_AWS_AUTH",
    "priority": 10,
    "profiles": ["*"]
  },
  {
    "name": "hetzner-secondary",
    "provider": "hetzner",
    "url": "https://approved-build-server.example.invalid",
    "authEnv": "GHA_EXECUTOR_HETZNER_AUTH",
    "priority": 20,
    "profiles": [
      "rust-verify",
      "node-verify",
      "python-verify",
      "flutter-verify",
      "playwright",
      "puppeteer"
    ]
  }
]
```

The placeholder hostname above is documentation only. Activation must use an
operator-reviewed TLS origin or another NetworkPolicy-compatible route. Do not
commit a live private endpoint to this file.

Routes are sorted by ascending priority and then by name. A route is eligible
only when its fixed-profile allowlist contains the requested profile or `*`.
Unknown providers, duplicate names, malformed origins, missing auth variables,
and empty profile sets fail startup.

## Admission boundary

The router accepts only requests that prove all of the following:

- `jobKind` is exactly `run-profile`;
- `gitRef` is an immutable 40-hex commit SHA;
- `profile` is a bounded fixed-profile identifier;
- `requestId` is present as the cross-executor idempotency identity;
- the body remains under the configured byte limit;
- the caller presents the internal build-server auth secret.

The router forwards the original validated JSON without adding caller-selected
commands, images, manifests or cloud credentials.

## Failover semantics

Failover is deliberately narrower than ordinary HTTP retry behavior.

A secondary executor may be attempted only after:

- a TCP/TLS connection failure before an upstream response exists; or
- an explicit upstream `429`, `502`, `503` or `504` response.

The router never fails over after:

- a `202 Accepted` response;
- a timeout or transport error after a connection may have delivered the body;
- an authentication or authorization failure;
- a validation failure;
- any other upstream response;
- an accepted response whose JSON or job ID is malformed.

This boundary prevents one logical workflow job from being submitted to both
AWS and Hetzner after an ambiguous outcome. The deterministic `requestId`
remains available to executor-side persistence and reconciliation, but it is
not used as permission to retry an ambiguous request.

## Stateless status routing

After an executor accepts a job, the router returns an HMAC-SHA256 signed job ID
that contains only:

- token version;
- route name;
- upstream job ID.

`GET /builds/<signed-id>` verifies the signature, resolves the original route,
and proxies status to that exact executor. A caller cannot edit a token to move
a job between AWS and Hetzner. Removing a route makes its outstanding tokens
return `410 Gone`; route retirement therefore requires draining or a reviewed
compatibility window.

## Kubernetes deployment

The router runs as a sidecar in the `dd-gha-clone-server` pod:

- planner endpoint: `0.0.0.0:8125`;
- router endpoint: `0.0.0.0:8126` inside the pod;
- planner dispatch URL: `http://127.0.0.1:8126`;
- no service-account token;
- no Docker, containerd or BuildKit socket;
- no hostPath;
- all Linux capabilities dropped;
- deployment remains at `replicas: 0` and both execution flags remain false.

AWS remains the first independent execution authority because the current
`dd-build-server`, persistence, artifacts and build runtime already exist
there. Hetzner becomes a secondary independent executor only after it exposes
the same fixed-profile API and satisfies the activation gates below.

## Secret properties

Backing secret: `dd/remote-dev/gha-clone-server-secrets`

Router-specific properties:

- `executor_routing_secret` — at least 32 random characters for signed route IDs;
- `executor_routes_json` — reviewed route metadata with no credentials;
- `hetzner_build_server_auth` — scoped secret accepted only by the Hetzner
  build server.

The existing `build_server_auth` is used for the planner-to-router hop and the
AWS build-server route. A later rotation may split those two identities without
changing the API.

Never store a classic GitHub PAT in these properties. GitHub workflow reads use
the separate short-lived GitHub App installation token.

## Activation gates

1. Merge the router source, tests, workflow and GitOps contract.
2. Build immutable images instead of relying on in-pod source compilation, then
   pin their digests.
3. Provision the expanded ExternalSecret without displaying values.
4. Keep the deployment at zero replicas and render both AWS and Hetzner GitOps.
5. Validate route JSON and secret references in a non-production namespace.
6. Scale to one replica with independent and webhook execution still disabled.
7. Verify `/readyz` and authenticated `/v1/executors` without logging secrets or
   private origins.
8. Run plan-only fixtures through the mirror.
9. Submit one immutable fixed-profile smoke to AWS and retain logs/artifacts.
10. Make Hetzner return an explicit capacity response in a controlled fixture
    and prove the secondary route executes exactly once.
11. Prove a validation rejection and an accepted primary response never touch
    the secondary route.
12. Enable API execution for an exact repository/workflow allowlist.
13. Enable webhook execution only after HMAC, delivery deduplication and replay
    evidence.

## Remaining multi-cloud authority work

Before Hetzner can be an authoritative independent executor, it still needs:

- the same immutable fixed-profile runner images;
- shared or replicated artifact storage with provenance;
- persistent request-ID reconciliation;
- Fiducia-fenced ownership for concurrent claims;
- equivalent egress, secret, log-retention and workload-isolation policy;
- a tested drain procedure for route rotation or removal.

Until those controls are complete, ARC can use Hetzner for ephemeral
non-privileged native-parity runners while AWS remains the authoritative
independent fixed-profile executor.

## GitHub Actions allowance evidence

A failed job before runner setup can indicate runner-allocation or billing
controls, but it does not reveal the exact included-minute balance. Exact
remaining minutes, spending limits, payment state and organization Actions
policy must be checked in GitHub billing/administrative settings. The continuity
system is useful regardless of which administrative condition blocked hosted
allocation.
