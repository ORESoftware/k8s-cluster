# Browser runners → prebuilt-image cutover runbook

## Goal

Move the two browser-automation deployments off the **in-pod build from a
hand-maintained node checkout** and onto **prebuilt images**, killing the
source-drift crash-loop that took them down for weeks:

- `dd-selenium-server` — the `selenium-api` container runs
  `maven:3.9.9-eclipse-temurin-17` and does `cd /opt/dd-next-1/remote/deployments/selenium-server && mvn package` on start.
- `dd-browser-test-server` — runs `mcr.microsoft.com/playwright` and does
  `pnpm install && pnpm run build` on start.

Both mount the node hostPath `/home/ec2-user/codes/dd/dd-next-1` for source.

## Why this is staged, not one-shot

Both deployments are **currently `2/2` Ready** — do not regress them. Three
constraints make a blind flip unsafe:

1. **Multi-node image coherence.** The dd `<name>:latest` pattern
   (`imagePullPolicy: IfNotPresent`, no registry prefix) resolves from a node's
   **local** containerd store, so the image must exist on **every node the pod
   may schedule to** (`dd-k8s-{fsn1,nbg1,hel1,wrk1,wrk2}`). `dd-image-builder`
   builds into one node's `k8s.io` namespace; the build must be repeated per
   node, or the pod pinned with a `nodeSelector` to the build node.
2. **The reference is parked.** `dd-ocr-rs` (the closest prebuilt-`:latest`
   service) sits at `replicas: 0`, so the pattern is not currently proven in
   live multi-node use here. Treat coherence as unsolved until verified.
3. **Not testable off-cluster.** The builder path needs real containerd. The
   Dockerfiles, however, **are** verified to build locally (see below), so the
   artifact is known-good even though the in-cluster build is not dry-runnable.

## Root causes this fixes (evidence, 2026-07-25)

- `dd-selenium-server` had ~6,600 restarts: `selenium-api` crashed on
  `cd .../selenium-server: No such file or directory` because the node checkout
  lacked the source. It recovered only once the dir was repopulated by hand —
  which will drift again. Prebuilt image removes the dependency entirely.
- `dd-browser-test-server` was `0/2`, pods `Evicted`: **node disk pressure** on
  the overcommitted control-plane node `dd-k8s-nbg1` (flapping
  `DiskPressure`, memory limits ~340% of allocatable). The on-node `pnpm`
  build + `node_modules` add to that pressure; a prebuilt image removes the
  build cost, and the pod should be kept off the pressured control-plane nodes
  (see step 5).

## Dockerfiles

Both ship correct multi-stage Dockerfiles that produce slim runtime images. The
`selenium-server` Dockerfile was **built and smoke-tested locally**
(2026-07-25): the resulting image (525 MB, JRE + shaded jar) starts and serves
`GET :8105/healthz` → `{"ok":true,"service":"dd-selenium-server",...}`, so the
prebuilt-image path is confirmed viable for the API container.
`browser-test-server` is a standard Playwright multi-stage build (reviewed, not
built locally — its base image is multi-GB); build it once on-cluster before
cutover.

```sh
# selenium API (JRE runtime, no Chromium — the Grid container owns the browser)
docker build -f remote/deployments/selenium-server/Dockerfile \
  -t dd-selenium-server-api:latest remote/deployments/selenium-server

# browser-test-server (Playwright runtime, node dist/server.js)
docker build -f remote/deployments/browser-test-server/Dockerfile \
  -t dd-browser-test-server:latest remote/deployments/browser-test-server
```

## Cutover

1. **Register build slugs.** Add both images to
   `remote/databases/pg/seeds/container-pool-app-config.sql` (same shape as the
   existing dd image slugs), each pointing at its Dockerfile + build context
   above.
2. **Build on every candidate node.** Trigger a build via `dd-image-builder`
   (`POST /api/container-pool/images/:slug/build-test`) and confirm the tag
   lands: `ctr -n k8s.io images ls | grep dd-selenium-server-api`. Repeat per
   node, **or** add a `nodeSelector` pinning each deployment to the node you
   built on. Do not proceed until the image is present wherever the pod can land.
3. **Cut over `dd-selenium-server`** (`dd-selenium-server.deployment.yaml`): in
   the `selenium-api` container replace `image:` with
   `dd-selenium-server-api:latest` + `imagePullPolicy: IfNotPresent`; delete the
   `command:`/`args:` build script (the image `ENTRYPOINT` runs the jar) and the
   `repo` `volumeMount`. Leave the `selenium` Grid container and the `dshm`
   volume untouched; drop the now-unused `repo` volume. Roll (the deployment is
   already `maxUnavailable: 0`); verify `GET :8105/healthz` → `{"ok":true}` and
   `2/2` before touching the second.
4. **Cut over `dd-browser-test-server`** (`dd-browser-test-server.deployment.yaml`):
   replace `image:` with `dd-browser-test-server:latest` + `IfNotPresent`;
   delete the `command:`/`args:` build script (image `CMD` is `node dist/server.js`)
   and the `repo` volume/mount; keep the `tmp` emptyDir. Roll; verify
   `GET :8104/healthz` and `2/2`.
5. **Keep the heavy pod off the pressured control-plane nodes.** Independently of
   the image work, `dd-browser-test-server` should prefer the worker nodes
   (`dd-k8s-wrk1`/`wrk2`) over the overcommitted control-plane nodes, or the
   `nbg1` disk pressure must be relieved (prune stale containerd images / raise
   ephemeral-storage headroom). Verify worker capacity before pinning.

## Rollback

Revert the deployment manifest. The in-pod-build version is the known-good
`2/2` state; the builder image can stay registered (idle) safely.

## Verification checklist

- `ctr -n k8s.io images ls` shows both tags on every schedulable node.
- `kubectl get deploy dd-selenium-server dd-browser-test-server` → `2/2`.
- `:8105/healthz` and `:8104/healthz` return `{"ok":true}`.
- Pods carry the new `image:` and **no** `/opt/dd-next-1` mount
  (`kubectl get pod <p> -o jsonpath='{.spec.containers[*].image}'`).
- A browser session still drives through the Grid (see
  `daedalus-fab-e2e/tests/remote-grid.e2e.mjs`).
