# canonical.cloud self-hosted browser-e2e runners

Chromium-capable GitHub Actions runners for the `canonical-cloud` org, so the
Playwright + Puppeteer browser e2e in `canonical-web-server.rs` and
`canonical-marketing-site.web` can run on this cluster instead of (or in
addition to) GitHub-hosted `ubuntu-latest`.

Built on GitHub's **Actions Runner Controller (ARC)** `gha-runner-scale-set`
model: ephemeral runners register on demand and are torn down after each job.

## Pieces

| Manifest | What it deploys |
| --- | --- |
| [`../apps/canonical-ci-arc-controller.application.yaml`](../apps/canonical-ci-arc-controller.application.yaml) | ARC controller Helm chart → namespace `arc-systems` (sync-wave 0) |
| [`../apps/canonical-browser-runner-set.application.yaml`](../apps/canonical-browser-runner-set.application.yaml) | `gha-runner-scale-set` for the org, using our Chromium image → namespace `arc-runners` (sync-wave 5) |
| [`../../deployments/canonical-ci-runner/Dockerfile`](../../deployments/canonical-ci-runner/Dockerfile) | Runner image: ARC runner + Node 22 + Chromium + headless libs |

The runner-set is addressable from workflows as **`runs-on: canonical-browser`**
(the `runnerScaleSetName`). The app repos ship an opt-in workflow —
`.github/workflows/browser-e2e-selfhosted.yml`, `workflow_dispatch` only — so
nothing depends on these runners until you deploy them and trigger it.

## One-time setup

1. **Build & push the runner image** to wherever the cluster pulls from
   (the runner-set references `ghcr.io/canonical-cloud/canonical-ci-runner:browser`):

   ```sh
   docker build -t ghcr.io/canonical-cloud/canonical-ci-runner:browser \
     remote/deployments/canonical-ci-runner
   docker push ghcr.io/canonical-cloud/canonical-ci-runner:browser
   ```

2. **Create the GitHub auth secret** in `arc-runners`. A GitHub App (scoped to
   the org, with the Actions + Self-hosted runners permissions) is preferred
   over a PAT:

   ```sh
   kubectl create namespace arc-runners
   kubectl create secret generic canonical-cloud-arc-github \
     --namespace arc-runners \
     --from-literal=github_app_id=<APP_ID> \
     --from-literal=github_app_installation_id=<INSTALLATION_ID> \
     --from-file=github_app_private_key=<path/to/app-private-key.pem>
   ```

   (PAT alternative: `--from-literal=github_token=<classic PAT with repo+admin:org>`.)
   Wire this through External Secrets like the other app secrets instead of
   `kubectl create` if you want it reconciled — see `remote/argocd/secrets/`.

3. **Register the OCI Helm repo** if Argo CD hasn't got it yet:

   ```sh
   argocd repo add ghcr.io/actions/actions-runner-controller-charts \
     --type helm --enable-oci
   ```

4. **Sync** `canonical-ci-arc-controller`, then `canonical-browser-runner-set`
   (the sync-waves already order them). Confirm registration:

   ```sh
   kubectl -n arc-runners get autoscalingrunnerset,ephemeralrunner
   ```

   The set should also appear under the org's **Settings → Actions → Runners**.

## Using it from a workflow

```yaml
jobs:
  browser-e2e:
    runs-on: canonical-browser   # the runnerScaleSetName above
```

Because the image already ships Chromium and exports `PLAYWRIGHT_CHROMIUM` /
`CHROME_PATH` / `PUPPETEER_EXECUTABLE_PATH`, the harnesses' `chromeExecutablePath()`
picks up the OS Chromium and no `playwright install` / Puppeteer download is
needed. Bump `maxRunners` in the runner-set values to raise concurrency.
