# `remote/tests`

Smoke tests for the live remote dev-server on EC2.

## Env

- `REMOTE_DEV_BASE_URL` (optional): defaults to `http://54.91.17.58`
- `REMOTE_DEV_SERVER_SECRET` (optional): enables authenticated endpoint checks
- `REMOTE_DEV_EC2_HOST` (optional): defaults to `54.91.17.58`
- `REMOTE_DEV_EC2_USER` (optional): defaults to `ec2-user`
- `REMOTE_DEV_EC2_KEY_PATH` (optional): defaults to `/Users/maca5/Downloads/main-key-pair.pem`
- `DD_EC2_GLEAM_LAMBDA_INTEGRATION=1`: enables the destructive-on-temp-resources EC2 Gleam lambda runner integration test
- `REMOTE_DEV_K8S_NAMESPACE` (optional): defaults to `default`
- `REMOTE_DEV_K8S_DEPLOYMENT` (optional): defaults to `dd-dev-server-api`

## Run

```bash
pnpm --dir remote/tests run test:all
```

Run the full UUID reuse + sleep/wake lifecycle test:

```bash
pnpm --dir remote/tests run test:cli:general
```

Run the deeper lifecycle test (routing + SSE + duplicate task IDs + UUID reuse + sleep/wake +
post-wake dispatch):

```bash
pnpm --dir remote/tests run test:cli:deep
```

Run websocket loadtest manifest checks (verifies 5k rust + 5k gleam and Argo paths):

```bash
pnpm --dir remote/tests run test:cli:ws-loadtest-config
```

Run runtime split checks (Rust `/home` web + Node API + gateway path map):

```bash
pnpm --dir remote/tests run test:cli:runtime-split-config
```

Run fabrication CAD/source intake checks (native CAD packages, neutral STEP/STL/3MF/DXF exports,
and machine-ready conversion evidence):

```bash
pnpm --dir remote/tests run test:cli:fabrication-cad-source-intake
```

Run observability stack checks (collector + Prometheus + Grafana + Loki + Tempo + Jaeger):

```bash
pnpm --dir remote/tests run test:cli:observability-config
```

Run the standalone observability coverage guardrail (workload watchlist, Grafana drilldown route,
and no auto-instrumentation/monkey-patching packages):

```bash
pnpm --dir remote/tests run check:observability-coverage
```

Run NATS messaging checks (NATS deployment + exporter scrape + Grafana panels):

```bash
pnpm --dir remote/tests run test:cli:nats-config
```

Run the true EC2 Gleam lambda runner integration. This copies the local runner/schema files to
`/tmp` on EC2, starts temporary Postgres and Gleam runner pods, builds runtime images with real
`nerdctl`, runs lambda containers through `ctr`/containerd, invokes host and containerized
Node/Python/Ruby/Bash functions, checks the runner survives a failed invocation, and cleans up its
temporary containers/images:

```bash
pnpm --dir remote/tests run test:cli:gleam-lambda-runner-ec2
```

## Athlet-O

The Athlet-O storefront ships as two vendored submodules under `remote/deployments/`:
`athleto-backend-rs` (the standalone `/jello` backend) and `athleto-app-rs` (the MASH shop app,
service alias `jello-ws`, public on `app.athleto.store` / `biz.athleto.store`).

- `ATHLETO_BASE_URL` (optional): base URL for the UI smokes. Falls back to `REMOTE_DEV_BASE_URL`,
  then the documented default `https://app.athleto.store`. Point it at a local `cargo run`
  (`http://127.0.0.1:8080`) or `biz.athleto.store` to exercise other chrome.

The config/contract tests read files only (no network) and pass offline. They validate the
`.gitmodules` pins plus the superproject argocd/gateway wiring, and skip the vendored-manifest
assertions with a clear message when a submodule is not checked out:

```bash
pnpm --dir remote/tests run test:cli:athleto-backend-config
pnpm --dir remote/tests run test:cli:athleto-app-config
```

The UI smokes drive the deployed storefront/backend (GET `/` + `/healthz` 200, Athlet-O brand copy,
CSP / X-Frame-Options / nosniff headers, the `/static` htmx asset served as javascript, and the
backend `/readyz` JSON when present). If `ATHLETO_BASE_URL` is unreachable they print a `SKIP`
notice and exit 0, so they are CI/cluster-ready without a live target:

```bash
pnpm --dir remote/tests run test:ui:athleto:playwright
pnpm --dir remote/tests run test:ui:athleto:puppeteer
```

Run all four Athlet-O suites together (also included in `test:all`):

```bash
pnpm --dir remote/tests run test:athleto
```
