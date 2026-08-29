# canonical.cloud self-hosted browser runners — open follow-ups

Self-hosted Chromium GitHub Actions runners for the `canonical-cloud` org, so the
Playwright + Puppeteer browser e2e in `canonical-web-server.rs` and
`canonical-marketing-site.web` can run on-cluster. Added as **reviewable
scaffolding, NOT yet deployed** (captured Jul 2026).

Pieces (see `remote/argocd/ci-runners/README.md` for the deploy walkthrough):
- `remote/argocd/apps/canonical-ci-arc-controller.application.yaml` — ARC
  controller (`gha-runner-scale-set-controller`) → ns `arc-systems`, sync-wave 0.
- `remote/argocd/apps/canonical-browser-runner-set.application.yaml` —
  `gha-runner-scale-set` for the org → ns `arc-runners`, sync-wave 5, label
  `runs-on: canonical-browser`.
- `remote/deployments/canonical-ci-runner/Dockerfile` — ARC runner + Node 22 +
  Chromium + headless libs.

The app repos ship an **opt-in** `browser-e2e-selfhosted.yml` (`workflow_dispatch`
only), so nothing depends on these runners until they exist and someone triggers
it. The blocking browser e2e stays on ubuntu-latest.

> Scope note: this is separate from the vendored canonical stack at
> `remote/deployments/canonical-cloud` (a secondary submodule of this repo —
> never edit it there). These runner additions live only under
> `remote/argocd/` + `remote/deployments/canonical-ci-runner/`.

---

## 1. Nothing is deployed or verified end-to-end (HIGH — the headline)

The manifests validate as YAML and mirror the working ubuntu-latest CI, but the
runners have **never registered or run a job**. Before trusting the self-hosted
path, walk the README checklist and confirm a green `browser-e2e-selfhosted` run.

## 2. Unpinned / unverified versions and images (HIGH)

Placeholders that must be checked against reality before first sync:
- ARC chart `targetRevision: 0.12.1` (both apps) — confirm the current
  `gha-runner-scale-set{,-controller}` release; controller and runner-set chart
  versions **must match**.
- Runner base image `ghcr.io/actions/actions-runner:2.320.0` — verify the tag
  exists; ideally pin by digest.
- Runner image `ghcr.io/canonical-cloud/canonical-ci-runner:browser` **does not
  exist yet** — nothing builds or pushes it (see §4).

**Shore up:** pin all three by digest once verified; the rest of the cluster
pins actions by SHA, do the same for images here.

## 3. Manual prerequisites that should be reconciled (MED)

- **GitHub auth secret** `canonical-cloud-arc-github` (ns `arc-runners`) is
  created by hand in the README. Every other app secret flows through External
  Secrets (`remote/argocd/secrets/`) — move this there so it's reconciled and
  not a snowflake. GitHub App preferred over a PAT.
- **OCI Helm repo registration** (`argocd repo add … --enable-oci`) is a manual
  one-time step; capture it as an `argocd-cm`/repo manifest instead so a cluster
  rebuild doesn't lose it.

## 4. No image build/publish pipeline (MED)

The runner image is hand-built per the README. It should have a workflow (like
the existing `remote-dev-ecr-rebuild.yml` / `refresh-remote-web-home-local-image.yml`)
that builds `remote/deployments/canonical-ci-runner/Dockerfile` and pushes on
change, so security updates to Chromium/Node actually ship. Until then the image
will rot.

## 5. Runner hardening not yet applied (MED)

The controller container has a locked-down `securityContext`; the **runner**
container does not (it needs a writable workspace, so `readOnlyRootFilesystem`
isn't a drop-in). Still missing for `arc-runners`:
- ResourceQuota / LimitRange on the namespace (a runaway job can currently take
  the whole node — see the AWS single-node history in
  `aws-learning-cluster-followups.md`).
- NetworkPolicy: browser jobs execute arbitrary repo code; restrict egress to
  what CI needs (GitHub, npm, crates.io) and deny cluster-internal traffic.
- `minRunners: 0` means cold-start latency on the first job; raise to 1 if that
  matters, at the cost of an idle pod.
- `containerMode.type: ""` runs the job in the runner container (no per-job
  sandbox). Acceptable for trusted first-party repos; revisit if untrusted PRs
  from forks ever target these runners.

## 6. Capacity / placement (LOW)

Runner requests are `500m / 1Gi` (limit `2 / 4Gi`) with `maxRunners: 4` → up to
`8 CPU / 16Gi` under full fan-out. Confirm the target node(s) have headroom
alongside the `dd` platform before raising `maxRunners`, and consider a
nodeSelector/toleration so browser jobs land off the latency-sensitive nodes.

## 7. Label/name coupling to double-check (LOW)

`runs-on: canonical-browser` depends on the runner-set's `runnerScaleSetName`
(`canonical-browser`) matching. The Application also sets `releaseName:
canonical-browser`; verify the deployed set actually surfaces under that label in
**org → Settings → Actions → Runners** and update both workflows if it differs.
