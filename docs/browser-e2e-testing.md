# Browser e2e testing — Playwright / Puppeteer / Selenium

*Live-run + findings, 2026-07-25. Covers the on-cluster browser-automation
deployments, how to drive them end to end on both the AWS EC2 and Hetzner
runtimes, the results of an actual run, and the operational findings.*

## The deployments

The cluster runs a small fleet of browser-automation services (source under
`remote/deployments/`, manifests under `remote/argocd/dd-next-runtime/`):

| Service | Image / stack | Port | Drivers | Role |
| --- | --- | --- | --- | --- |
| `dd-browser-test-server` | Node + Fastify (Playwright Noble base) | 8104 | Playwright, Puppeteer, Selenium | On-demand UI scenarios behind one `POST /run` API |
| `dd-selenium-server` | Java/Vert.x + `selenium/standalone-chromium` sidecar | 8105 (API), 4444 (Grid, internal-only) | Selenium (dedicated) | First-class Selenium fidelity, isolated browser process |
| `dd-web-scraper` | Node | 8097 | — | One-shot HTML scraping (sibling, not e2e) |
| `dd-browser-job-runner` | Rust (`browser-job-runner-rs`) | — | — | Queue-driven browser job execution |

All three drivers in `dd-browser-test-server` reuse Playwright's bundled Chromium
via `playwright.chromium.executablePath()`; Selenium resolves a matching
`chromedriver` through Selenium Manager. Nightly e2e suites run as CronJobs
(`dd-athleto-e2e-browser-suite`, `dd-fiducia-e2e-browser-suite`).

### The `POST /run` contract

Authenticated with `x-server-auth: $SERVER_AUTH_SECRET`. Body:

```json
{
  "tool": "playwright | puppeteer | selenium",
  "captureFinalScreenshot": true,
  "steps": [
    { "action": "goto", "url": "https://example.com", "waitUntil": "domcontentloaded" },
    { "action": "waitForSelector", "selector": "h1" },
    { "action": "extractText", "selector": "title", "name": "pageTitle" },
    { "action": "screenshot", "name": "shot" }
  ]
}
```

Supported actions: `goto`, `click`, `fill`, `select`, `press`, `waitForSelector`,
`waitForUrl`, `waitForTimeout`, `extractText`, `extractAttribute`, `screenshot`,
`evaluate` (disabled unless `BROWSER_TEST_ALLOW_EVALUATE=true`). Public
diagnostics: `GET /healthz`, `/metrics`, `/status`, `/tools` (the gateway also
mirrors them under `/browser-test/*`, but behind the operator header).

`/run` is **not** gateway-exposed — it is an in-cluster endpoint. To drive it you
must be inside the cluster (kubectl exec / in-cluster curl), which is why the
runbook below execs into the pod.

## Reaching the two runtimes

| Runtime | Node(s) | Access path |
| --- | --- | --- |
| **AWS EC2** | `dd-remote-k8s-1` (`i-0cc2461a55d491af6`, r7i.4xlarge, `98.90.186.114`) | Direct SSH is key-denied; the node is **SSM-managed** → `aws ssm send-command` (profile `default`/`my-cli-user`, account `710156900967`). kubeconfig on the node: `/home/ec2-user/.kube/config`. |
| **Hetzner** | `dd-k8s-{fsn1,nbg1,hel1}` (control-plane) + `wrk1,wrk2` (workers), k8s v1.31.14 | SSH `hetzner-k8s-bastion` (`167.233.100.88`, root), kubectl available on the control-plane node. |

The gateway (`https://98.90.186.114`) gates `/browser-test/*` behind the operator
`dd` header, so it is not a shortcut to `/run`.

## Runbook — drive `/run` without leaking the secret

The clean pattern: `kubectl exec` **into the test-server pod** and curl
`localhost`, so `$SERVER_AUTH_SECRET` is read from the pod's own env and never
leaves the pod or appears in a command line.

**AWS (via SSM):**

```bash
aws ssm send-command --profile default --instance-ids i-0cc2461a55d491af6 \
  --document-name AWS-RunShellScript \
  --parameters 'commands=["echo <base64-of-script> | base64 -d | bash"]'
# where the script is:
export KUBECONFIG=/home/ec2-user/.kube/config
POD=$(kubectl get pod -n default -l app=dd-browser-test-server -o jsonpath='{.items[0].metadata.name}')
kubectl exec -n default "$POD" -- sh -c \
  'curl -s -X POST http://localhost:8104/run \
     -H "content-type: application/json" \
     -H "x-server-auth: $SERVER_AUTH_SECRET" \
     -d "{\"tool\":\"playwright\",\"steps\":[{\"action\":\"goto\",\"url\":\"https://example.com\"}]}"'
```

Base64-wrapping the script avoids SSM's JSON-parameter quoting entirely. A ready
helper is kept out of tree at scratch (`ssm.sh`): it send-commands, polls
`get-command-invocation` until terminal, and prints stdout/stderr.

**Hetzner (via SSH):** same `kubectl exec` one-liner over
`ssh hetzner-k8s-bastion '…'`.

The dedicated Selenium server is identical but targets the `selenium-api`
container on `:8105`:
`kubectl exec -n default <selenium-pod> -c selenium-api -- sh -c 'curl … localhost:8105/run …'`.

## Run results — 2026-07-25

### AWS EC2 — healthy, all drivers pass

| Driver | Target | Result | Duration |
| --- | --- | --- | --- |
| Playwright | example.com | ✅ `ok`, title "Example Domain", `headline` extracted | 612 ms |
| Puppeteer | example.com | ✅ `ok`, title "Example Domain" | 517 ms |
| Selenium (`:8104`) | example.com | ✅ `ok`, title "Example Domain" | 1009 ms |
| Selenium (dedicated `:8105`) | example.com | ✅ `ok`, headline extracted | 723 ms |
| **Playwright** | **https://fiducia.cloud** (real prod) | ✅ `ok`, title "Fiducia — Consensus & Coordination as a Service", 13.7 KB screenshot | 250 ms |
| Playwright | https://the1mills.com | ❌ `ERR_NAME_NOT_RESOLVED` (DNS — domain not live) | — |
| Selenium | https://canonical.cloud | ❌ `ERR_NAME_NOT_RESOLVED` (DNS — domain not live) | — |
| Playwright | in-cluster `dd-remote-web-home:8080` | ⚠️ blocked (see NetworkPolicy finding) | — |

**Web-server UI matrix — each driver against a real product UI:**

| Driver | Web UI | Result |
| --- | --- | --- |
| Playwright | https://app.fiducia.cloud | ✅ "Fiducia Customer Portal" (38 KB shot), 1358 ms |
| Puppeteer | https://app.athleto.store | ✅ "AthletO \| performance gelatin protein" (48 KB shot), 1171 ms |
| Selenium | https://admin.fiducia.cloud | ✅ "Sign in · Fiducia Admin" (→ /login, 30 KB shot), 1459 ms |

All driver paths are healthy on AWS, including all three against real product web
UIs. The two DNS failures are real signals about those properties (they don't
resolve; `fiducia.cloud` does), and the error-handling path is correct (distinct
`ERR_NAME_NOT_RESOLVED` vs timeout vs `ERR_ACCESS_DENIED`).

### Hetzner — was down; `dd-selenium-server` now FIXED and deployed

Both browser services were crash-looping on Hetzner (`dd-selenium-server` and
`dd-browser-test-server` at **0/2**, restart counts **6000+** over 23 days).
Three root causes were found and fixed — the first two masked on AWS because its
pods/jars were built ~27 days ago against then-valid host state and deps:

1. **Source via hostPath.** Both deployments `cd /opt/dd-next-1/…`, mounted from
   `hostPath: /home/ec2-user/codes/dd/dd-next-1`, which exists **only on the AWS
   EC2 node**. On Hetzner every pod exited 1 (`cd: … No such file or directory`).
   This is the **runtime-source anti-pattern** from dd-build-server audit **F13**
   (see [`build-server-hardening.md`](build-server-hardening.md)). **Fix:** a
   per-pod `initContainer` shallow-clones the **public** superproject into an
   `emptyDir` at `/opt/dd-next-1` — self-contained on any node/cluster.
   browser-test additionally needs the **private** `remote/libs` submodule (for
   `@dd/telemetry`) and so needs a GH token (see below); selenium's Maven build is
   standalone (public clone only).
2. **semconv `NoClassDefFoundError` (selenium).** `selenium-server/pom.xml`
   force-pinned `opentelemetry-semconv:1.30.0`, which dropped the monolithic
   `io.opentelemetry.semconv.SemanticAttributes` that selenium-java 4.27 uses at
   its first WebDriver session — so a **fresh** build threw
   `NoClassDefFoundError`. Verified with `jar tf`; pinned to `1.25.0-alpha`
   (selenium 4.27's own declared version), which ships **both**
   `SemanticAttributes` (selenium) and `ServiceAttributes` (Telemetry.java).
3. **initContainer git hardening.** Getting the clone to work as a non-root
   container surfaced three more: git `safe.directory` (dubious-ownership on the
   fsGroup emptyDir), an SSH→HTTPS submodule URL rewrite, and idempotency across
   init restarts (the `emptyDir` persists). All handled in the initContainer.

**Result — validated in production (2026-07-25):** `dd-selenium-server` on
Hetzner went from `0/2` to **`2/2` available** after the fix deployed via ArgoCD
(PR #34 → `dev`); a `selenium /run` against the real service returns `ok:true`
driving example.com and `app.fiducia.cloud` ("Fiducia Customer Portal").

**`dd-browser-test-server` — also FIXED (PR #36).** It additionally needs the
**private** `remote/libs` submodule (for the `@dd/telemetry` file: dep). `GH_PAT`
turned out to be **empty in the backing store on both clusters** (base64 length 0
on AWS and Hetzner — AWS only worked because it used the pre-populated hostPath and
never cloned). The correct, already-provisioned credential is **`GH_DEPLOY_KEY`**
(present on both, 560 bytes), and the submodule URL is natively SSH. So an
`alpine/git` initContainer (which bundles openssh — the Playwright image doesn't)
writes the deploy key, `ssh-keyscan` + `StrictHostKeyChecking=yes`, clones the
public superproject over HTTPS and the private submodule over SSH, then `chown`s
the tree to the uid-1000 runtime so pnpm can build. **Validated in production
(2026-07-25):** both clusters went to **2/2** and `/run` returns `ok:true` for
**all three drivers** (Playwright/Puppeteer/Selenium) against app.fiducia.cloud.

Historical `DiskPressure` also left ~92 `Evicted` tombstones per service; nodes
now report all pressures `False`. Evicted-pod cleanup is safe hygiene.

## Findings & recommendations

1. **[DONE] Self-contained source via initContainer clone.**
   Shipped for both `dd-selenium-server` (PR #34) and `dd-browser-test-server`
   (PR #36) — live and 2/2 on **both** clusters. selenium clones the public
   superproject only; browser-test additionally clones the private `remote/libs`
   submodule over SSH via `GH_DEPLOY_KEY`. A future hardening is to bake proper CI
   images (build-server patch 01 style) so no runtime clone/build is needed at all.
2. **[LOW] `GH_PAT` is empty in the backing store** (`dd/remote-dev/agent-secrets`)
   on both clusters. Nothing currently depends on it (the browser services use
   `GH_DEPLOY_KEY`), but dd-build-server's config references it — worth populating
   or removing the reference to avoid a future silent gap.
2. **[MED] Evicted-pod hygiene.** Add a reaper (or
   `kubectl delete pod --field-selector status.phase=Failed`) so historical
   `DiskPressure` doesn't leave hundreds of `Failed` tombstones; investigate the
   disk-fill source (browser caches / `CARGO`/image layers) and add
   `ephemeral-storage` limits + `emptyDir.sizeLimit`, mirroring the build-server
   guardrails.
3. **[INFO — working as intended] Browser-test-server egress is NetworkPolicy-locked.**
   In-cluster targets (`dd-remote-web-home`, `dd-dev-server-home`) return
   `ERR_ACCESS_DENIED`/timeout: the pod may reach the public internet but not
   arbitrary internal services. This is correct SSRF hardening. To e2e an
   internal UI, add that Service to the browser-test-server egress allowlist
   deliberately.
4. **[INFO] Public-site smokes are a good CI signal.** The `fiducia.cloud` smoke
   (title + screenshot) is a real production canary; `the1mills.com` and
   `canonical.cloud` do not resolve yet and would make good "is DNS live?"
   checks once launched.

## Reproducing

The four AWS driver smokes + the `fiducia.cloud` production smoke are the
canonical green run. Re-run them with the SSM/SSH runbook above; a passing set is
all four drivers returning `"ok":true` with `finalTitle:"Example Domain"` and the
`fiducia.cloud` smoke returning its title + a non-zero screenshot byte count.
