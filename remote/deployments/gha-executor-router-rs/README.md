# gha-executor-router-rs

`gha-executor-router-rs` is the fail-closed provider-routing boundary between
`gha-clone-server-rs` and operator-reviewed `dd-build-server` instances in AWS
and Hetzner.

It is deliberately **not** a workflow parser, scheduler, shell executor, image
builder, or GitHub runner. The workflow mirror continues to compile only its
reviewed GitHub Actions subset into fixed build-server profiles. This service
only decides which configured executor receives an immutable fixed-profile
request.

Linear: [DEN-1597](https://linear.app/denman/issue/DEN-1597/ci-continuity-add-fail-closed-awshetzner-independent-executor-routing)

## Non-duplication invariant

Failover is allowed only before an executor returns HTTP `202 Accepted`.

- transport errors, HTTP 429, and HTTP 5xx may advance to the next configured
  executor;
- other HTTP 4xx responses fail closed without fallback;
- an unexpected successful status or malformed 202 body fails closed because
  the upstream may already have accepted work;
- once accepted, the returned external id is namespaced as
  `<executor-id>~<upstream-build-id>`;
- every later status request is pinned to that exact executor;
- a polling failure never causes submission to another provider.

Cross-provider recovery after acceptance requires shared durable job state,
shared artifacts, and a Fiducia-fenced claim. It is not implemented here.

## Accepted request surface

The public submission endpoint remains `POST /builds`, but input is narrower
than the complete build-server schema:

```json
{
  "schemaVersion": "build-server.v1",
  "jobKind": "run-profile",
  "repoUrl": "https://github.com/ORESoftware/k8s-cluster.git",
  "gitRef": "0123456789abcdef0123456789abcdef01234567",
  "profile": "rust-verify",
  "requestId": "gha-clone:plan-id:job-id"
}
```

Unknown fields are rejected. Callers cannot supply an endpoint, provider,
command, script, image, build arguments, deployment manifest, arbitrary header,
or credential. The repository revision must be a full lowercase commit SHA.
The downstream build server remains authoritative for repository/profile
allowlists and execution.

## Configuration

```text
HOST=0.0.0.0
PORT=8126
GHA_EXECUTOR_ROUTER_EXECUTION_ENABLED=false
GHA_EXECUTOR_ROUTER_AUTH_SECRET_FILE=/var/run/secrets/gha-executor-router/operator-auth
GHA_EXECUTOR_ROUTER_EXECUTORS_JSON=[
  {
    "id": "aws",
    "provider": "aws",
    "baseUrl": "http://dd-build-server.default.svc.cluster.local:8100",
    "authSecretFile": "/var/run/secrets/gha-executor-router/aws-auth"
  },
  {
    "id": "hetzner",
    "provider": "hetzner",
    "baseUrl": "https://builds.hetzner.example.com",
    "authSecretFile": "/var/run/secrets/gha-executor-router/hetzner-auth"
  }
]
```

Production endpoints require HTTPS. Plain HTTP is accepted only for loopback
tests or Kubernetes service DNS. URLs may not contain credentials, query
strings, fragments, or paths. Executor ids, providers, URLs, and mounted secret
files must all be unique. At most one AWS and one Hetzner endpoint are accepted
by the initial contract.

Credential files must be absolute, regular, non-symlink files containing 1 to
8192 bytes. Execution-enabled startup fails when operator auth, any executor
secret, or every configured endpoint is incomplete. Disabled mode remains
ready for inert GitOps installation without reading unavailable secret values.

## Endpoints

- `POST /builds` — validate, deduplicate, and route a fixed-profile request;
- `GET /builds/<namespaced-id>` — poll only the executor that accepted it;
- `GET /capabilities` — machine-readable failover boundary;
- `GET /healthz` — non-secret configuration inventory;
- `GET /readyz` — activation readiness;
- `GET /metrics` — submission, fallback, rejection, dedupe, route, and pinned
  polling counters.

Inbound authentication accepts the existing `x-build-server-auth` header and
uses a constant-time digest comparison. Upstream response bodies and secret
values are never included in client errors.

## Tests

The Rust suite starts real local Axum build-server doubles and proves:

1. AWS acceptance prevents any Hetzner request;
2. AWS transport, 429, and 5xx failures advance to Hetzner;
3. AWS 4xx rejection fails closed without Hetzner fallback;
4. concurrent duplicate request ids produce one upstream submission;
5. later duplicates return the existing route;
6. accepted AWS jobs remain pinned when polling fails;
7. external ids cannot collide across executors;
8. upstream error bodies are redacted;
9. URL, provider, endpoint, and secret-path authority is validated;
10. mutable revisions, non-profile jobs, and unknown arbitrary execution fields
    are rejected.

The GitHub Actions workflow runs formatting, tests, Clippy with warnings denied,
and static no-arbitrary-command checks on a hosted Ubuntu runner. A manual
opt-in repeats the same contract on `[self-hosted, linux, sonus-ci]` after the
AWS/Hetzner ARC scale set is registered.

## GitOps state

The deployment is intentionally installed with `replicas: 0` and
`GHA_EXECUTOR_ROUTER_EXECUTION_ENABLED=false`. The ExternalSecret names three
separate values—operator auth, AWS build-server auth, and Hetzner build-server
auth—and mounts them as files. No service-account token, host path, Docker or
containerd socket, cloud credential, or inline secret is mounted.

Activation requires:

1. immutable image build, SBOM, provenance, scan, and digest pin;
2. reachable AWS and Hetzner fixed-profile build servers;
3. separate credentials and repository/profile allowlists;
4. AWS acceptance smoke and Hetzner pre-acceptance failover smoke;
5. proof that accepted-job polling never resubmits;
6. rollback by disabling execution and scaling the deployment to zero.
