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

All four driver paths are healthy on AWS. The two DNS failures are real signals
about those properties (they don't resolve; `fiducia.cloud` does), and the
error-handling path is correct (distinct `ERR_NAME_NOT_RESOLVED` vs timeout vs
`ERR_ACCESS_DENIED`).

### Hetzner — browser-test-server is **down** (crash loop)

`dd-browser-test-server`: **0/2 available**, ~92 `Evicted` tombstones plus live
pods in `CrashLoopBackOff` with restart counts up to **6373** over 23 days.

Two distinct causes:

1. **Application crash on startup (root cause).** Last-crash log:
   ```
   /bin/bash: line 5: cd: /opt/dd-next-1/remote/deployments/browser-test-server: No such file or directory
   ```
   The Hetzner deployment runs the service from a **host source path**
   (`/opt/dd-next-1/…`) that is not present on the Hetzner nodes, so it `cd`s
   into a missing directory and exits 1 — forever. It runs on AWS only because
   that path happens to exist there. This is the **same runtime-source
   anti-pattern** called out for dd-build-server (audit **F13**, see
   [`build-server-hardening.md`](build-server-hardening.md)): the workload
   depends on cluster-host state instead of a self-contained image.
2. **Historical `DiskPressure`.** Nodes currently report all pressures `False`,
   but past `DiskPressure` evicted pods en masse ("Pod was rejected: The node
   had condition: [DiskPressure]"), leaving ~92 `Failed` tombstones.

A `kubectl rollout restart` does **not** fix it (the new pod hits the same
missing path); the evicted-pod cleanup is safe hygiene but insufficient.

## Findings & recommendations

1. **[HIGH] Hetzner browser-test-server must ship as a self-contained image.**
   Build the `remote/deployments/browser-test-server` image in CI, push to a
   registry, and run it by digest — dropping the `/opt/dd-next-1` host-source
   `cd`. This is the browser-test analogue of build-server hardening patch 01.
   Until then, `dd-browser-test-server` and its nightly suites are inoperative on
   Hetzner, and the ~92 evicted pods will keep accumulating.
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
