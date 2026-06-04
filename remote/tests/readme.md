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
