# AI agent bridge immutable rollout — 2026-08-07

Tracking: Linear DEN-845 and DEN-1041; GitHub issue #1111.

## Selected release

- Source repository: `ORESoftware/ai-agent-bridge.rs`
- Source revision: `01abb601a3b6a6cfa917094daf17cb9fe1c54f21`
- Trusted container workflow run: `31215966809`, attempt `1`
- Bridge image: `ghcr.io/oresoftware/fiducia-ai-agent-bridge@sha256:daa438cb75d9409821f40ea5698a85ae2970d6c6b33b8fe66a9e05a87b28aaec`
- Slack command image: `ghcr.io/oresoftware/fiducia-slack-command@sha256:cba2bf92408589df478ebfb19ce3db01d7eafbbf05e2a0ead3afb843a690b72d`

The trusted source workflow built each image from one locked source tree, verified the non-root distroless runtime contract, ran Trivy high/critical scanning, published SBOM and provenance attestations, and uploaded validated machine-readable digest evidence.

## Change boundary

This rollout replaces Kubernetes-time Git clone and Rust compilation for `dd-ai-agent-bridge` and `dd-slack-command` with the exact digests above. It removes:

- the Rust builder image and init container;
- the optional `GH_PAT` clone path;
- node source `hostPath` access;
- source, Cargo registry, Git database, and target build volumes;
- runtime `git clone` and `cargo build` commands; and
- mutable source/tag resolution from deployment and smoke contracts.

It retains:

- non-root UID/GID/fsGroup `65532`;
- read-only root filesystems, dropped capabilities, and RuntimeDefault seccomp;
- startup, readiness, and liveness probes;
- secret-backed bridge, coordinator, and Slack bearers;
- Slack app/workspace identity enforcement;
- the reviewed thirteen-channel registry embedded in the Slack command image;
- durable Slack idempotency state;
- explicit NetworkPolicy, PodDisruptionBudget, and ExternalSecret resources; and
- `SLACK_COMMAND_DRY_RUN=true` until real provider canaries pass.

The existing HTTP `dd-provider-runner` remains scaled to zero. The newly published Rust provider-runner image is not substituted for that HTTP service until its interface is reconciled and certified; a digest match alone is not semantic compatibility.

## Pre-merge evidence required

- Static bridge and Slack GitOps contract tests pass.
- The `dd-next-runtime` overlay renders successfully and contains both exact image refs.
- Rendered manifests contain no bridge/Slack runtime Git clone, Cargo build, PAT, or source hostPath.
- The exact bridge digest can be authenticated, pulled, and inspected from the k8s-cluster workflow.
- OCI revision label equals the selected source SHA; runtime user and entrypoint match the reviewed contract.
- Docker runtime smoke passes HTTP, SSE, TCP, workflow, lease, health, readiness, and bearer checks.
- Ephemeral kind rollout passes Deployment, Service, probes, pod security, and transport checks.
- Evidence artifacts contain no synthetic bearer values.

## Cluster activation sequence

1. Merge the exact-head PR into `dev` only after all required checks pass.
2. Confirm ArgoCD observes the merged `dev` revision and reports the application Synced/Healthy.
3. Confirm `dd-ai-agent-bridge-secrets` and `dd-slack-command-secrets` are Ready without printing values.
4. Confirm both pods report the exact selected image IDs and Ready status.
5. Confirm `/healthz` and `/readyz`; verify unauthenticated non-health calls fail and the scoped bearer succeeds.
6. Submit a signed Slack dry-run canary and verify that no provider or downstream write occurs.
7. Enable one bounded ChatGPT canary and one bounded Claude canary only after provider/coordinator credentials and routes are proven.
8. Verify same-thread callbacks, idempotency, cancellation, denial, partial-failure, and audit evidence before changing `SLACK_COMMAND_DRY_RUN`.

## Rollback

Rollback is a manifest-only change to the previously recorded exact image digests. Do not restore in-pod source builds, mutable tags, a compiler, a clone PAT, or node source mounts. Before live activation, capture the currently running image IDs from Kubernetes and attach them to DEN-845 and GitHub issue #1111 as the rollback baseline.

## Known external gate

The connected GitHub App does not expose GHCR package-visibility metadata or GitHub Projects-v2 mutations. The PR workflows must therefore prove package pull authorization directly, and GitHub issue #1111 remains the canonical project-ready item until the Projects-capable app is restored.
