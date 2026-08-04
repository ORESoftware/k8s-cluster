# GHA executor router GitOps and activation

Linear: DEN-1597  
Parent: DEN-1550  
Core router: pull request #665
Last reviewed: 2026-08-04

## Purpose

The independent GitHub Actions compatibility lane is split into three bounded
services:

1. `gha-clone-server-rs` parses only the reviewed workflow subset and emits an
   immutable repository SHA plus a fixed build profile.
2. `gha-executor-router` selects one reviewed execution authority before a
   submission and pins accepted status to that provider.
3. `dd-build-server` executes only its fixed-profile registry.

Actions Runner Controller remains the native-parity lane. The independent lane
does not claim parity with GitHub expressions, matrices, marketplace actions,
environments, checks, artifacts, caches, OIDC, macOS, or Windows semantics.

## Rendered resources

The shared `remote/argocd/dd-next-runtime/kustomization.yaml` renders:

- `dd-gha-executor-router.configmap.yaml`
- `dd-gha-executor-router.externalsecret.yaml`
- `dd-gha-executor-router.deployment.yaml`
- `dd-gha-executor-router.service.yaml`
- `dd-gha-executor-router.networkpolicy.yaml`

The router and clone server both remain at `replicas: 0`. Router execution,
clone-server API execution, and clone-server webhook execution all remain
`false`.

The clone server no longer has direct NetworkPolicy egress to `dd-build-server`.
It authenticates to `dd-gha-executor-router:8126`; only the router can reach the
AWS build server on port 8100.

## Initial provider inventory

The checked-in ordered inventory declares:

- `aws-primary`: enabled, in-cluster `dd-build-server`, mounted authentication
  file, first placement authority;
- `hetzner-secondary`: disabled and intentionally has no URL or authentication
  path.

A disabled entry cannot accumulate dormant endpoint or credential state. A
separate reviewed activation change must add the Hetzner HTTPS origin,
`authPath`, mounted secret item, and CIDR policy if the route is private.

## Secret-file contract

External Secrets backing object:

```text
dd/remote-dev/gha-executor-router-secrets
```

Initial required properties:

- `inbound_auth`: clone server to router only;
- `aws_build_server_auth`: router to AWS build server only.

The Kubernetes Secret projects them as direct-child, mode-0400 files:

```text
/var/run/secrets/gha-executor-router/inbound-auth
/var/run/secrets/gha-executor-router/aws-build-server-auth
```

The route inventory contains paths, not secret values. The router rejects
traversal, indirect children, duplicate paths, missing files, short values,
symlink escapes, and oversized credential data.

The clone-server ExternalSecret intentionally does not import the old direct
`build_server_auth` value. The planner receives only the router inbound
credential; provider credentials remain confined to the router Secret.

`hetzner_build_server_auth` is not requested or mounted while the Hetzner entry
is disabled. Do not put a classic GitHub personal access token in this backing
secret; GitHub API reads use the separate short-lived GitHub App installation
token owned by the clone server.

## Network boundary

Clone server egress:

- cluster DNS;
- router service TCP 8126;
- public HTTPS for GitHub API/source bootstrap.

Router egress:

- cluster DNS;
- AWS `dd-build-server` TCP 8100;
- public HTTPS for source bootstrap and a future reviewed Hetzner endpoint.

Metadata, loopback, carrier-grade NAT, and RFC1918 ranges are excluded from the
public-HTTPS rule. A private AWS-to-Hetzner VPN route requires a narrow,
explicit CIDR change; the broad public rule cannot be repurposed to access
private networks.

Neither service receives a Kubernetes service-account token, hostPath, Docker
socket, containerd socket, BuildKit socket, or cloud credential.

## No-duplicate routing rule

Provider selection occurs only before `POST /builds`:

- the router probes executors in reviewed order;
- an executor already failing readiness may be skipped;
- after any submission attempt, a timeout, reset, 429, 5xx, invalid response,
  or response-body failure is ambiguous and is never submitted to another
  provider;
- a 4xx contract rejection fails closed;
- a successful accepted ID is namespaced to the executor and every status read
  remains pinned to it.

Automatic takeover after an attempted submission requires a shared durable
assignment, a Fiducia-fenced claim, shared job visibility, and shared artifact
addressing. This GitOps layer does not weaken that requirement.

## CI evidence contract

The path-scoped `GHA continuity server` workflow runs:

- Rust 1.90 formatting;
- strict Clippy over all targets;
- all crate tests;
- the named `executor_router_http` real-process suite;
- actionlint for both continuity workflows;
- TypeScript GitOps and security contracts;
- credential-pattern rejection.

Static contracts fail when:

- either deployment is scaled above zero;
- any execution gate is enabled;
- router credentials are supplied as environment values;
- host runtime sockets or hostPath appear;
- the clone server regains direct build-server egress;
- Hetzner gains a dormant URL/authPath while disabled;
- router resources leave the shared render;
- secret properties or paths drift.

## Activation sequence

1. Merge the core router and this stacked GitOps change in order.
2. Build immutable clone-server and router images; retain SBOM, provenance, and
   vulnerability evidence; replace source compilation and pin digests.
3. Provision the two initial backing-secret properties without displaying them.
4. Verify the ExternalSecret is Ready and the AWS auth matches the scoped build
   server identity.
5. Keep both Deployments at zero and render the AWS and Hetzner ArgoCD targets.
6. Scale only the router to one with execution still false; verify `/healthz`,
   `/readyz`, capabilities, and bounded metrics.
7. Scale the clone server to one with both execution gates false; run plan-only
   and meta-dogfood fixtures.
8. Enable router execution and submit one exact immutable AWS fixed-profile
   smoke through the router API; retain logs and artifacts.
9. Enable clone-server API execution for one exact allowlisted workflow and
   prove the same AWS route.
10. Provision a Hetzner fixed-profile build server, digest-pinned runner images,
    scoped credential, TLS origin, artifact addressing, egress policy, and
    rollback evidence.
11. In one reviewed change, add the Hetzner URL/authPath and secret-file item.
12. Prove AWS readiness failure selects Hetzner before submission; prove an AWS
    rejection or ambiguous attempted submission never reaches Hetzner.
13. Enable webhook execution only after HMAC, exact workflow-path, delivery-ID,
    replay, deduplication, and recursion controls pass.
14. Add durable cross-provider assignment and Fiducia fencing before considering
    takeover after an attempted submission.

## Rollback

- disable clone-server webhook execution, then API execution;
- disable router execution;
- scale clone server and router to zero;
- drain accepted jobs before removing or renaming an executor identity;
- preserve logs, artifacts, request IDs, and workflow history;
- restore hosted or certified ARC routing rather than bypassing required checks.

## Actions allowance evidence

Current `ORESoftware/k8s-cluster` pull-request jobs have acquired hosted Ubuntu
runners and executed real steps, so GitHub-hosted Actions are not globally
unavailable. This does not reveal the numeric included-minute balance for every
organization. Exact usage, spending limits, payment state, and organization
policy remain the billing-read GitHub App and administrative-settings boundary
tracked in DEN-1549.
