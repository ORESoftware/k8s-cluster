# DES web route consolidation

Tracking: [Linear DEN-1936](https://linear.app/denman/issue/DEN-1936/des-webrsk8s-cluster-consolidate-public-des-pages-under-des)  
Application PR: [discrete-event-systems/des-web.rs#10](https://github.com/discrete-event-systems/des-web.rs/pull/10)  
GitOps PR: [ORESoftware/k8s-cluster#872](https://github.com/ORESoftware/k8s-cluster/pull/872)

## Decision

`/des` is the one public browser and HTTP API namespace for discrete-event-system pages. The dynamic application source belongs to the `discrete-event-systems` GitHub organization; this repository owns only Kubernetes, gateway compatibility, rollout, and observability integration.

The previous public path was split across three concerns:

1. the main gateway proxied `/des/` to `dd-des-simulator`;
2. newer MASH pages lived in `discrete-event-systems/des-web.rs`;
3. selected pages linked back to `/des-rs/*` or `/out/*`.

The consolidated flow is:

```text
browser /des/*
  -> dd-remote-gateway (auth + strip /des/)
  -> dd-des-simulator Service :8099 (stable compatibility name)
  -> selector app=dd-des-web
  -> dd-des-web pod :8130
  -> dd-des-rs :8112 only for engine solve/stream work
  -> Postgres/RDS only for optional persisted catalog/run state
```

`dd-des-web` is the canonical workload and Service. `dd-des-simulator` is now only a compatibility Service alias because the existing gateway already names it in both AWS and Hetzner deployments. This cuts over the implementation without changing a public URL or copying another large nginx location block.

## Canonical public routes

| Route | Purpose |
|---|---|
| `/des/` | catalog |
| `/des/models` | model-family index |
| `/des/games/soccer` | tournament and learning pages |
| `/des/games/soccer/planner` | rotation planner |
| `/des/games/elevator` | FEL dispatch learning |
| `/des/games/elevator/player` | elevator playback |
| `/des/tools/routing` | VRP/TSP solver UI |
| `/des/labs/factory-floor-track3t` | Track3t factory-floor lab |
| `/des/runs/{run_id}` | stable cross-model run entry |
| `/des/artifacts/{artifact_id}` | generated/vendored output |
| `/des/api/v1/catalog` | machine-readable route/ownership contract |
| `/des/api/v1/solve` | routing solve API |
| `/des/api/v1/solve/{id}` | routing result API |

The gateway strips `/des/` before forwarding. Kubernetes sets `DES_PUBLIC_PATH_MODE=mounted`, so `des-web.rs` rewrites first-party links, htmx endpoints, forms, JavaScript URL strings, and redirects back to `/des/*`. Direct local development keeps service-local roots such as `/soccer` and `/routing`.

## Compatibility

- `/des-rs/*` remains an engine/debug compatibility surface. New browser links must not use it.
- `/out/*` remains a generated-output compatibility surface. New application routes use `/des/artifacts/*`.
- `/des/music` remains an explicit legacy redirect to `/des-rs/music` until a DES-owned music page is implemented.
- The old `dd-des-simulator` Deployment stays at zero replicas during the transition. Its Service name remains because it is the stable gateway upstream.

Compatibility traffic should be measured before removal. GET/HEAD page aliases may later become permanent redirects; mutation or streaming routes must never be redirected across services implicitly.

## GitOps objects

- `dd-des-web.deployment.yaml`: two replicas, non-root/read-only runtime, probes, resources, mounted-path mode, optional database secret, source revision, and immutable image digest.
- `dd-des-web.service.yaml`: canonical in-cluster service on port 8130.
- `dd-des-simulator.service.yaml`: gateway compatibility alias on port 8099 selecting `dd-des-web` pods.
- `dd-des-web.networkpolicy.yaml`: gateway ingress; DNS, DES-engine, private Postgres, and public HTTPS egress only.
- `dd-des-web.pdb.yaml`: at least one replica remains available during voluntary disruption.

## Immutable application evidence

The Deployment is pinned to the exact successful application head, not a mutable channel:

- source revision: `77741ec8b5331617f71416748ef5f06846e43a5d`
- image: `ghcr.io/discrete-event-systems/des-web.rs:sha-77741ec8b5331617f71416748ef5f06846e43a5d@sha256:c3b32a5ef767bcdba515c8199fce363871ba2916e4c824609a09a37b3adc02e5`
- application CI: formatting, Clippy with warnings denied, locked build/tests, Postgres seed/schema convergence, `dpm verify`, and Playwright route contracts passed
- publication: Buildx produced provenance and an SBOM, then pushed the digest above from the same source revision

The repository contract test rejects mutable `main` and `latest` image tags and verifies that the source annotation, SHA tag, and digest-pinned image remain internally consistent.

## Rollout

1. `des-web.rs#10` passes Rust, schema, Playwright, and immutable-image checks at the pinned source revision.
2. The exact image digest is pinned in `dd-des-web.deployment.yaml`.
3. Rebase this GitOps change semantically onto current `main`, then run route/manifest tests and render the `dd-next-runtime` kustomization.
4. Merge the application PR before, or atomically with, this GitOps PR.
5. Sync `dd-next-runtime` in the AWS and Hetzner Argo CD control planes.
6. Verify `/des/`, every canonical page above, `/des/api/v1/catalog`, `/des/healthz`, and `/des/readyz` from both public entry points.
7. Verify planner/solve delegation to `dd-des-rs`, database-backed fragments, and degraded operation when no database URL is configured.
8. Record request counts for `/des-rs/*`, `/out/*`, and `/des/music` before scheduling compatibility removal.

## Rollback

A rollback does not require a gateway change:

1. restore `dd-des-simulator.service.yaml` to selector `app: dd-des-simulator`;
2. temporarily scale `dd-des-simulator.deployment.yaml` above zero;
3. sync `dd-next-runtime` and verify `/des/`;
4. keep the failed `dd-des-web` revision available for logs and comparison.

The canonical public URL remains `/des/` throughout rollout and rollback.
