# dd-build-server architectural hardening — ready-to-apply patches

*2026-07-21. Companion to [`../build-server-hardening.md`](../build-server-hardening.md).*

These are the four **architectural** findings from the security audit. Unlike the
code fixes (already in the build-server crate), each of these is a
production/cluster change with real blast radius, so they live **here as
reviewable proposals rather than in the ArgoCD-synced `remote/argocd/` tree** —
nothing in this directory is picked up by `dd-next-runtime.application.yaml`
(`targetRevision: dev`, `prune + selfHeal`). Apply them deliberately, in order,
after reading each header.

| # | Finding | Severity | Artifact | Auto-applies? |
|---|---------|----------|----------|---------------|
| 1 | Deployment `cargo run`s `origin/dev` at pod start; Dockerfile/ECR image unused | CRITICAL | [`01-ecr-image-deployment.md`](01-ecr-image-deployment.md) | No — documented diff |
| 2 | `create deployments` in unlabeled `default` = SA escalation | HIGH | [`02-dd-builds-namespace-rbac.yaml`](02-dd-builds-namespace-rbac.yaml) | No — new namespace |
| 3 | Same SA can author hostPath/privileged/foreign-SA PodSpecs | HIGH | [`03-restrict-podspec-vap.yaml`](03-restrict-podspec-vap.yaml) | No — cluster-scoped policy |
| 4 | containerd/buildkit host sockets = node-root | CRITICAL | [`04-rootless-buildkit-sketch.yaml`](04-rootless-buildkit-sketch.yaml) | No — sketch |
| 5 | fiducia coordination fails open, key likely unprovisioned | HIGH | [`05-fiducia-coordination-enable.md`](05-fiducia-coordination-enable.md) | No — provisioning steps |

## Suggested order

1. **Patch 5 first** — it's the cheapest and highest-confidence: verify whether
   the fiducia key exists and stop the silent fail-open. Pure config + one AWS
   Secrets Manager entry.
2. **Patch 1** — move to the ECR image. Removes the biggest RCE surface (push to
   `dev` = node-root) and dissolves the `GH_PAT`-in-argv finding.
3. **Patches 2 + 3 together** — dedicated namespace, scoped RBAC, and the
   admission policy that makes the remaining deploy rights non-escalating.
4. **Patch 4 last** — the largest lift (rootless BuildKit), removes the
   node-root socket mounts entirely. Until then, patches 2+3 bound the damage.

Each artifact is self-contained and annotated with the exact wiring change
(kustomization entry, env flip, secret key) and a rollback note.
