# dd-selenium-server

A **dedicated, long-lived, Selenium-only** browser-automation server for the EC2 Kubernetes runtime.

This is the Selenium-only sibling of [`dd-browser-test-server`](../browser-test-server/readme.md). Where
that service multiplexes Playwright, Puppeteer, **and** Selenium behind one Node/Fastify API, this one
runs a single dedicated **Selenium Grid** and exposes the same bounded `POST /run` scenario contract
through a Java service — for callers that want first-class Selenium fidelity and an isolated browser
process.

## Why Java + a Grid sidecar

Selenium is a Java-first project: `selenium-java` is the reference WebDriver binding, so a
Selenium-dedicated service maximizes parity by using it directly. The deployment is **one pod with two
containers**:

| Container | Image | Role |
| --- | --- | --- |
| `selenium` | `selenium/standalone-chromium` | The actual long-lived **Selenium server** (Grid) on `:4444`. Bundles Chromium + chromedriver. |
| `selenium-api` | `maven:3.9.9-eclipse-temurin-17` | Vert.x HTTP API (this project). Self-builds the shaded jar with Maven, then drives the Grid over `RemoteWebDriver` at `http://localhost:4444`. |

The Grid port (`:4444`) is **never** exposed through the Kubernetes Service — only the authenticated
Java API on `:8105` is — so the Grid can't be reached as an unauthenticated remote-control endpoint.

Driving the browser in a separate container (over `RemoteWebDriver`) rather than in-process is the
canonical Selenium production shape for a long-lived server: a browser crash can't take down the API,
and the Grid owns session lifecycle and queueing. Each `POST /run` opens a fresh remote session and
quits it afterward, so cookies and storage never leak between scenarios.

## API

All endpoints are served at the root and mirrored under `/selenium/...` (the gateway proxies the
prefixed path through unchanged).

| Method | Path | Description |
| --- | --- | --- |
| `POST` | `/run` | Run one bounded scenario. **Requires auth.** |
| `GET` | `/` , `/selenium` | Service descriptor. |
| `GET` | `/tools` , `/selenium/tools` | Driver/version descriptor. |
| `GET` | `/status` , `/selenium/status` | Runtime status (in-flight, limits, grid URL). |
| `GET` | `/healthz` , `/selenium/healthz` | Liveness/health. |
| `GET` | `/readyz` | Readiness probe. |
| `GET` | `/metrics` , `/selenium/metrics` | Prometheus scrape (`selenium_runs_total`, `selenium_in_flight`, Vert.x metrics). |

### `POST /run`

```jsonc
{
  "requestId": "optional-correlation-id",
  "url": "https://example.com",          // optional opening goto
  "steps": [
    { "action": "goto", "url": "https://example.com" },
    { "action": "waitForSelector", "selector": "h1", "state": "visible" },
    { "action": "extractText", "selector": "h1", "name": "heading" },
    { "action": "click", "selector": "a.more", "nth": 0 },
    { "action": "screenshot", "name": "after-click" }
  ],
  "viewport": { "width": 1280, "height": 800 },  // optional
  "userAgent": "…",                              // optional
  "timeoutMs": 30000,                            // optional overall
  "captureFinalScreenshot": true,                // default true
  "failOnConsoleError": false                    // default false
}
```

Supported step actions (identical to `dd-browser-test-server`): `goto`, `click`, `fill`, `select`,
`press`, `waitForSelector`, `waitForUrl`, `waitForTimeout`, `extractText`, `extractAttribute`,
`screenshot`, `evaluate`.

The response carries `ok`, per-step logs, `extracted` values, `screenshots` (base64 PNG, byte-capped),
`consoleEntries`, `finalUrl`/`finalTitle`, and `error` on failure. Status codes: `200` ok, `422` a step
failed, `400` invalid request, `429` over the concurrency cap, `401` unauthenticated.

> `screenshot` produces a viewport PNG; `fullPage` is accepted but not honored (Selenium has no
> portable full-page capture). `evaluate` is disabled unless `SELENIUM_ALLOW_EVALUATE=true`.

## Auth

`POST /run` requires the shared `SERVER_AUTH_SECRET` (synced from `dd-agent-secrets`), supplied as
`X-Server-Auth: <secret>` or `Authorization: Bearer <secret>` and compared in constant time. The
gateway injects this header for `/selenium*` traffic that has already passed the operator cookie/header
gate. The service **fails closed**: with no secret configured, every `POST /run` is rejected unless
`SELENIUM_ALLOW_UNAUTHENTICATED=true`.

## Configuration

| Env var | Default | Meaning |
| --- | --- | --- |
| `HTTP_HOST` / `HTTP_PORT` | `0.0.0.0` / `8105` | Bind address for the Java API. |
| `SELENIUM_REMOTE_URL` | `http://localhost:4444` | In-pod Selenium Grid endpoint. |
| `SELENIUM_MAX_CONCURRENT` | `2` | Max concurrent browser sessions per pod (429 over the cap). |
| `SELENIUM_DEFAULT_TIMEOUT_MS` | `30000` | Default overall scenario timeout. |
| `SELENIUM_MAX_TIMEOUT_MS` | `180000` | Upper bound for a requested overall timeout. |
| `SELENIUM_STEP_TIMEOUT_MS` | `15000` | Default per-step / page-load / script timeout. |
| `SELENIUM_MAX_STEPS` | `64` | Max steps per scenario. |
| `SELENIUM_MAX_SCREENSHOT_BYTES` | `1500000` | Screenshot byte cap (PNG truncated beyond it). |
| `SELENIUM_BROWSER_HEADLESS` | `true` | Pass `--headless=new` to Chrome. |
| `SELENIUM_ALLOW_EVALUATE` | `false` | Allow arbitrary in-page `evaluate` steps. |
| `SELENIUM_ALLOW_UNAUTHENTICATED` | `false` | Bypass the auth gate (non-production only). |
| `SERVER_AUTH_SECRET` | _(secret)_ | Shared dd-agent secret for `POST /run`. |

## Build / run

The cluster deployment self-builds from the mounted repo (the `selenium-api` container runs
`mvn -B -e -DskipTests package` on first start, exactly like `dd-spark-pipeline-server`). To build or
run the API container standalone — pointing at any external Selenium Grid — use the multi-stage
[`Dockerfile`](./Dockerfile):

```bash
docker build -f remote/deployments/selenium-server/Dockerfile \
  -t dd-selenium-server:dev remote/deployments/selenium-server
docker run --rm -p 8105:8105 \
  -e SELENIUM_REMOTE_URL=http://host.docker.internal:4444 \
  -e SELENIUM_ALLOW_UNAUTHENTICATED=true \
  dd-selenium-server:dev
```

Local compile / unit check:

```bash
cd remote/deployments/selenium-server
mvn -B -e -DskipTests package
```

## Deployment

- Manifests: `remote/argocd/dd-next-runtime/dd-selenium-server.{deployment,service}.yaml`
  (registered in `kustomization.yaml`).
- Gateway: `/selenium` and `/selenium/` in `dd-remote-gateway.configmap.yaml` (operator-auth gated;
  injects `X-Server-Auth`).
- Logs: `dd-selenium-server` is included in the promtail prod log selector.
