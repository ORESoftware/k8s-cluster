# Benefactor backend GitOps deployment

This directory is the production deployment source of truth for
`benefactor-cc/backend.rs`.

Release flow:

1. Backend GitHub Actions runs formatting, Clippy, tests, and a container build.
2. A successful `main` build is published to the private GHCR package.
3. The workflow uses a repository-specific deploy key to replace the image
   entry in `kustomization.yaml` with the exact `sha256:` digest and commits
   that change to `k8s-cluster@dev`.
4. Argo CD application `benefactor-backend-rs` reconciles this directory with
   pruning and self-healing enabled.

The pod runs the prebuilt non-root image. It does not clone source, receive a
GitHub source token, or compile Rust at startup.

## Private registry credential

`benefactor-ghcr.externalsecret.yaml` reads AWS Secrets Manager entry
`dd/benefactor/ghcr-pull`, property `dockerconfigjson`, and creates the
`default/benefactor-ghcr` image-pull secret. The stored token should have only
`read:packages`, expires 2026-10-16, must be rotated before expiry, and must
never be committed.

## Bootstrap or recovery

Apply the Argo application once, then let Argo own the workload:

```sh
kubectl apply -f remote/argocd/apps/benefactor-backend-rs.application.yaml
kubectl -n argocd get application benefactor-backend-rs
```

Do not apply the Deployment directly during ordinary releases.

## Prometheus contract and live evidence

The backend exposes `/metrics` on its HTTP port. The Service and pod both carry
explicit scrape annotations, while central Prometheus has the stable
`benefactor-backend-rs` job and target-down, dependency, pipeline, workload,
CPU, and memory alerts.

Static CI renders both kustomizations and runs the Prometheus wiring contracts.
To collect the live evidence required by DEN-677, create local-only
port-forwards in separate terminals:

```sh
kubectl -n observability port-forward svc/dd-prometheus 9090:9090
kubectl -n default port-forward svc/benefactor-backend-rs 18135:80
```

Then run the read-only verifier:

```sh
python3 tools/verify_benefactor_observability_live.py \
  --prometheus-url http://127.0.0.1:9090 \
  --metrics-url http://127.0.0.1:18135/metrics \
  --output tmp/benefactor-observability-evidence.json
```

The command succeeds only when Prometheus reports `up == 1`, the backend's
bounded `postgres` readiness gauge is `1`, the direct endpoint contains the
required metric families, and no metric or label name exposes email, lead,
contact, CRM, provider-query, raw-URL, credential, or secret fields. It writes
the evidence file atomically only after all checks pass. The evidence contains
metric names and aggregate values, never application payloads or Kubernetes
Secrets.
