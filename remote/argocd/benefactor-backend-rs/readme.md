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
