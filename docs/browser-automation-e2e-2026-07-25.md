# Browser-automation E2E — deployed services, 2026-07-25

Live end-to-end tests of the deployed Puppeteer/Playwright/Selenium stack, driven
against the real service pods (not mocks). Two of four services pass cleanly;
the async orchestrator is broken on this deployment (one code bug fixed here, one
config gap flagged), and the NATS bridge is down on a missing secret.

## Where the stack lives

| Cluster | Access | Browser stack? |
|---|---|---|
| **AWS EC2** (`dd-ec2-runtime`, us-east-1) | `kubectl` (admin context) | **Yes** — all four services + container-pool + NATS |
| **Hetzner** (`kind-fiducia-hetzner{,-fsn1,-hel1}`, v1.36.1) | `kubectl` | **No** — runs the `fiducia` workload (brain/node/load-balance); no browser services |

So browser E2E is inherently AWS-only. Engine versions live on the cluster:
**Playwright 1.56.0, Puppeteer 24.43.1, Selenium 4.44.0** (from `dd-browser-test-server /tools`).

## How to run (reproducible)

All services are `ClusterIP` and gated by the `x-server-auth` header
(`dd-agent-secrets/SERVER_AUTH_SECRET`). From a machine with the `dd-ec2-runtime`
kube-context:

```sh
CTX=dd-ec2-runtime
SECRET=$(kubectl --context $CTX -n default get secret dd-agent-secrets \
          -o jsonpath='{.data.SERVER_AUTH_SECRET}' | base64 -d)

# web-scraper (:8097) — Playwright render
kubectl --context $CTX -n default port-forward svc/dd-web-scraper 18097:8097 &
curl -s -X POST localhost:18097/scrape -H "x-server-auth: $SECRET" \
  -H 'content-type: application/json' \
  -d '{"url":"https://example.com","strategy":"playwright"}'

# browser-test-server (:8104) — one scenario, per engine
kubectl --context $CTX -n default port-forward svc/dd-browser-test-server 18104:8104 &
for ENG in playwright puppeteer selenium; do
  curl -s -X POST localhost:18104/run -H "x-server-auth: $SECRET" \
    -H 'content-type: application/json' \
    -d "{\"engine\":\"$ENG\",\"steps\":[
         {\"action\":\"goto\",\"url\":\"https://example.com\"},
         {\"action\":\"waitForSelector\",\"selector\":\"h1\"},
         {\"action\":\"extractText\",\"selector\":\"h1\",\"name\":\"headline\"}]}"
done

# selenium-server (:8105)
kubectl --context $CTX -n default port-forward svc/dd-selenium-server 18105:8105 &
curl -s -X POST localhost:18105/selenium/run -H "x-server-auth: $SECRET" \
  -H 'content-type: application/json' \
  -d '{"steps":[{"action":"goto","url":"https://example.com"},
       {"action":"extractText","selector":"h1","name":"headline"}]}'

# browser-job-runner (:8106) — async; returns 202 + jobId, result to NATS
kubectl --context $CTX -n default port-forward svc/dd-browser-job-runner 18106:8106 &
curl -s -X POST localhost:18106/browser-jobs/run -H "x-server-auth: $SECRET" \
  -H 'content-type: application/json' \
  -d '{"engine":"playwright","steps":[{"action":"goto","url":"https://example.com"},
       {"action":"extractText","selector":"h1","name":"headline"}]}'
```

Benign target only (`example.com`, whose `<h1>` is "Example Domain"); a PASS is
that string appearing in the extracted `headline` / rendered text.

## Results

| Service | Port | Result |
|---|---|---|
| `dd-web-scraper` | 8097 | ✅ Playwright rendered example.com (status 200, title "Example Domain", ~1.7s) |
| `dd-browser-test-server` | 8104 | ✅ **Playwright**, ✅ **Puppeteer**, ✅ **Selenium** — all extracted "Example Domain" |
| `dd-selenium-server` | 8105 | ✅ `/selenium/run` and `/run` both extracted "Example Domain" |
| `dd-browser-job-runner` | 8106 | ❌ submit OK (202 + jobId) but the job **never completes** — see F1/F2 |
| `dd-nats-bridge` | 3004 | ❌ `0/1`, pod `CreateContainerConfigError` — see F3 |

Auth posture confirmed correct: every service returns `401 {"ok":false,"error":"unauthorized"}` without the `x-server-auth` header.

## Findings

### F1 — browser-job-runner nerdctl fallback passed `-d --rm` (code bug — FIXED)

The fallback spawn built `nerdctl … run -d --rm …`. **nerdctl rejects `-d` and
`--rm` together** (unlike Docker): `fatal msg="flags -d and --rm cannot be
specified together"`, so every fallback job failed at spawn. Fixed in
`browser-job-runner-rs/src/main.rs`: extracted a testable `nerdctl_run_args()` and
dropped `--rm` (detached is required for the 202 contract; the container is
cleaned up by the tracker's `force_remove` on overrun/failure and by
`dd-idle-reaper` as a backstop). Regression test:
`nerdctl_run_args_are_detached_without_rm`.

### F2 — container-pool has no `browser-jobs` pool registered (config gap — flag)

Before falling back, the runner tries the warm pool and gets
`unknown container pool: browser-jobs; falling back to nerdctl`. The
`dd-container-pool` only has the `nodejs-chat-claude-live-mutex-dev` pool warm;
the `browser-jobs` pool from
`remote/databases/pg/seeds/container-pool-app-config.sql` is not loaded/registered
on this deployment. Until it is (or F1 is redeployed), the runner has neither a
pool nor a working fallback. Action: seed/register the `browser-jobs` pool, then
redeploy the runner with F1.

### F3 — dd-nats-bridge down on a missing secret (deploy gap — flag)

`dd-nats-bridge` is `0/1` with `CreateContainerConfigError`: its deployment
requires `BRIDGE_TOKEN` from secret `dd-nats-bridge-secrets`
(`Optional: false`), which is not provisioned — fallout from the recent auth
hardening (the bridge now refuses to run without a token). Action: create
`dd-nats-bridge-secrets` with `BRIDGE_TOKEN` (via the cluster's ExternalSecret /
secret manager, same pattern as other `*-secrets`), then the deployment will
schedule. Not fixed here — provisioning a production auth credential is an
operator action.

## Notes

- All calls used a per-service `kubectl port-forward` and a benign public target;
  no state was mutated on the cluster.
- The auth secret was read via the admin kube-context, held only in-process for
  the curl calls, and never written to logs or committed.
