# AI agent bridge immutable rollout — 2026-08-07

Tracking: Linear DEN-845, DEN-1041, and DEN-847; GitHub issue #1111; rollout PRs #1112 and #1115.

## Selected release

- Source repository: `ORESoftware/ai-agent-bridge.rs`
- Source revision: `c3e54e6cd0c6d56e3d2ed32902228d974e550a3f`
- Trusted container workflow run: `31235992249`, attempt `1`
- Bridge image: `ghcr.io/oresoftware/fiducia-ai-agent-bridge@sha256:6b7e447a9989fa127ad4b0b3edc51fcd37a6b94a96bcf61b42c22d2641bf0ea8`
- Slack command image: `ghcr.io/oresoftware/fiducia-slack-command@sha256:01f80fbd4d3ba5226b4abdb7f5e603538924edb48e79e72b0af43246624900cb`
- Provider runner image: `ghcr.io/oresoftware/fiducia-ai-agent-runner@sha256:90a919fb28fb2bc2795a0a3735ab08993d245c3eaa2afcd5f42be9b1a4982702`

The successful source workflow built all images from the same locked source tree, verified the non-root runtime contract, ran high/critical vulnerability scanning, published SBOM and provenance attestations, and uploaded validated machine-readable digest evidence. This source includes the reviewed duplicate-before-capacity correction plus durable read-only status and cooperative cancellation behavior. PR #1112 deploys only the bridge and slash-command binaries; it does not expose the separate Slack Events API ingress binary or that binary's private-only metrics endpoint.

## Change boundary

PR #1112 replaces Kubernetes-time Git clone and Rust compilation for `dd-ai-agent-bridge` and `dd-slack-command` with the exact digests above. It removes:

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
- the reviewed fourteen-channel registry embedded in the Slack command image;
- durable Slack idempotency state;
- explicit NetworkPolicy, PodDisruptionBudget, and ExternalSecret resources; and
- `SLACK_COMMAND_DRY_RUN=true` until real provider canaries pass.

PR #1115 is the companion runner-only change. It adds the exact provider-runner digest at `replicas: 0` with an independent service account, NetworkPolicy, probes, resources, and secret boundary. It must remain at zero until DEN-391 and DEN-847 prove provider credentials, interface compatibility, and a bounded one-provider canary. Neither PR changes the existing HTTP `dd-provider-runner` boundary by assumption.

## Credential-safe ORES client evidence

The reviewed client source merged in `ORESoftware/ai-agent-bridge.rs` at:

```text
ef5358d54faac1c035e0754f5e7421e10664a75f
```

Trusted container workflow run `31269442497`, attempt `1`, published and then pulled, runtime-inspected, help-contract verified, and exact-digest scanned:

```text
ghcr.io/oresoftware/ores-ai-agent-bridge-client@sha256:f6576d1fc2fbadad454c77cc078078695421b138731374347f44f98c67f8269e
```

Machine-readable evidence artifact:

```text
image-digest-ores-client-31269442497-1
```

The `dd-next-runtime` overlay includes two suspended CronJobs that use this exact digest:

- `dd-ai-agent-bridge-client-probe` runs the read-only HTTP/TCP/auth/wire-identity probe;
- `dd-ai-agent-bridge-client-smoke` additionally registers the stable smoke identity, resolves and joins the canonical channel, posts a unique marker, and reads the same sequence back.

Both CronJobs remain `suspend: true`. They use a dedicated tokenless service account, a read-only non-root pod, and a deny-by-default NetworkPolicy that permits only cluster DNS and bridge ports `8142`/`8143`. The bearer arrives only through `secretKeyRef` from `dd-ai-agent-bridge-secrets/inbox_token`; it is never present in an image, command, URL, GitOps value, or generated Job name.

After Argo reports the suspended objects Synced and the ExternalSecret is Ready, instantiate a one-off probe without changing the CronJob schedule:

```sh
probe_job="dd-ai-agent-bridge-client-probe-$(date -u +%Y%m%d%H%M%S)"
kubectl -n default create job --from=cronjob/dd-ai-agent-bridge-client-probe "$probe_job"
kubectl -n default wait --for=condition=complete --timeout=180s "job/$probe_job"
kubectl -n default logs "job/$probe_job"
kubectl -n default delete job "$probe_job" --wait=true
```

Review the bounded JSON probe output before running the mutating smoke:

```sh
smoke_job="dd-ai-agent-bridge-client-smoke-$(date -u +%Y%m%d%H%M%S)"
kubectl -n default create job --from=cronjob/dd-ai-agent-bridge-client-smoke "$smoke_job"
kubectl -n default wait --for=condition=complete --timeout=240s "job/$smoke_job"
kubectl -n default logs "job/$smoke_job"
kubectl -n default delete job "$smoke_job" --wait=true
```

Do not unsuspend either CronJob. Do not run the smoke before the probe succeeds, do not print the Secret, and do not treat a completed Job as provider authorization: this smoke exercises the bridge conversation bus only and does not invoke Claude, ChatGPT, or another model provider.

## Pre-merge evidence required

- Static bridge, Slack, and runner GitOps contract tests pass.
- The `dd-next-runtime` overlay renders successfully and contains all selected exact image refs.
- Rendered manifests contain no bridge, Slack, or runner runtime Git clone, Cargo build, PAT, or source hostPath.
- Each selected digest can be authenticated, pulled, and inspected from a trusted workflow.
- OCI revision labels equal the selected source SHA; runtime user and entrypoints match the reviewed contracts.
- Docker runtime smoke passes HTTP, SSE, TCP, workflow, lease, health, readiness, and bearer checks.
- Ephemeral kind rollout passes Deployment, Service, probes, pod security, and transport checks.
- Evidence artifacts contain no synthetic bearer values.

## Cluster activation sequence

1. Merge the exact-head bridge/Slack PR into `dev` only after its required checks pass; merge the runner-only PR only after rebasing it onto that result and preserving `replicas: 0`.
2. Confirm ArgoCD observes the merged `dev` revision and reports the application Synced/Healthy.
3. Confirm `dd-ai-agent-bridge-secrets`, `dd-slack-command-secrets`, and any zero-replica runner ExternalSecret are Ready without printing values.
4. Confirm bridge and Slack pods report the exact selected image IDs and Ready status; confirm the runner remains at zero replicas.
5. Confirm `/healthz` and `/readyz`; verify unauthenticated non-health calls fail and the scoped bearer succeeds.
6. Submit a signed Slack dry-run canary and verify that no provider or downstream write occurs.
7. Enable one bounded ChatGPT canary and one bounded Claude canary only after provider/coordinator credentials and routes are proven.
8. Verify same-thread callbacks, duplicate delivery, status lookup, cooperative cancellation, denial, partial failure, and audit evidence before changing `SLACK_COMMAND_DRY_RUN` or scaling the provider runner.

## Rollback

Rollback is a manifest-only change to the previously recorded exact image digests. Do not restore in-pod source builds, mutable tags, a compiler, a clone PAT, or node source mounts. Before live activation, capture the currently running image IDs from Kubernetes and attach them to DEN-845 and GitHub issue #1111 as the rollback baseline. Keep the runner at zero during a bridge or Slack rollback unless DEN-847 explicitly authorizes otherwise.

The probe/smoke rollback is simply removal of the one-off Job. Because both source CronJobs remain suspended, they do not create replacement Jobs. If the client image or contract must be withdrawn, remove the client resources or repin them to a separately reviewed exact digest; never replace the digest with a mutable tag.

## Known external gates

- The repository-wide private-backend check currently requires the approved GitHub App credential path; do not substitute a PAT or reintroduce PAT propagation to make it green.
- Cluster completion still requires ArgoCD, ExternalSecret, pod image-ID, probe, bearer, signed Slack dry-run, and deterministic rollback evidence.
- GitHub issue #1111 remains the canonical project-ready ledger until a Projects-v2-capable integration can place the work directly.
