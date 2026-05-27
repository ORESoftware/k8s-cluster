# dd-rust-network-mutex (deployment overlay)

Operator-facing landing for the Rust port of `live-mutex` running on
`dd-next-runtime`. The application source lives in the submodule at
[`remote/submodules/rust-network-mutex-rs`](../../submodules/rust-network-mutex-rs)
(GitHub: [`ORESoftware/live-mutex-rs`](https://github.com/ORESoftware/live-mutex-rs)).
This directory is for **YAML / kustomize / runbook** material only — no
Rust source, no Cargo manifest.

## Where the canonical YAML lives

The `argocd` app for `dd-next-runtime` is the source of truth for the
running broker, so the deployment + service manifests sit under that
overlay:

- [`remote/argocd/dd-next-runtime/dd-rust-network-mutex.deployment.yaml`](../../argocd/dd-next-runtime/dd-rust-network-mutex.deployment.yaml)
- [`remote/argocd/dd-next-runtime/dd-rust-network-mutex.service.yaml`](../../argocd/dd-next-runtime/dd-rust-network-mutex.service.yaml)

Those files are referenced from
[`remote/argocd/dd-next-runtime/kustomization.yaml`](../../argocd/dd-next-runtime/kustomization.yaml)
and rolled out by the `dd-next-runtime` ArgoCD application.

The thin overlay in [`k8s/ec2/`](./k8s/ec2/) re-exports those same
manifests via `kustomize` so an operator can run

```bash
# kustomize refuses to load files outside the kustomization directory
# by default; the --load-restrictor flag opts into the cross-directory
# reference. We deliberately keep the YAML at the argocd path so there
# is exactly one source of truth.
kubectl apply -k remote/deployments/rust-network-mutex-rs/k8s/ec2 \
    --load-restrictor LoadRestrictionsNone
```

against an ad-hoc cluster (or `kubectl diff -k …` for review) without
needing to remember the argocd path.

## Container image

The broker is published on Docker Hub at
[`oresoftware/live-mutex-rs`](https://hub.docker.com/r/oresoftware/live-mutex-rs).
Tags follow the `Cargo.toml` version of the submodule
(currently `0.1.123`). The image is built from the multi-stage
`Dockerfile` at the root of the submodule
([source](https://github.com/ORESoftware/live-mutex-rs/blob/dev/Dockerfile)).

To rebuild and push (once Docker Hub credentials are available):

```bash
cd remote/submodules/rust-network-mutex-rs
VERSION="$(cargo pkgid | sed -E 's/.*#//')"   # e.g. 0.1.123
docker buildx build \
    --platform linux/amd64,linux/arm64 \
    --tag oresoftware/live-mutex-rs:"${VERSION}" \
    --tag oresoftware/live-mutex-rs:latest \
    --push .
```

After the push, bump the `image:` field in
`remote/argocd/dd-next-runtime/dd-rust-network-mutex.deployment.yaml`
to the new tag and let ArgoCD reconcile.

## Env-var contract

The broker reads everything from environment variables; see the
"Environment variables" table in
[the submodule's readme](../../submodules/rust-network-mutex-rs/readme.md#environment-variables)
for the full list. The deployment manifest sets the production
defaults (TCP `:6970`, HTTP `:6971`, default TTL 4 s, `LMX_AUTH_TOKEN`
sourced from the `dd-agent-secrets` bundle when present).

## Health checks

The broker exposes:

- TCP `:6970` — newline-delimited JSON wire protocol (probed via
  `tcpSocket` during pod startup before the HTTP listener is up).
- HTTP `:6971/healthz` and `:6971/readyz` — startup, readiness, and
  liveness probes target these paths.
- HTTP `:6971/metrics` — Prometheus exposition (`dd_rust_network_mutex_*`),
  scraped via the standard `prometheus.io/scrape` annotations on the
  Service.
