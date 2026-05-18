# dd-fsharp-ws-server

An ASP.NET Core + WebSocket service written in F# that runs the same five-stage
request pipeline through **two different coordination libraries** so we can
compare them side by side on the .NET runtime:

| Pipeline implementation              | Library                                                   |
| ------------------------------------ | --------------------------------------------------------- |
| `RxPipeline.processFrame`            | [Rx.NET (`System.Reactive`)](https://github.com/dotnet/reactive) — Observable graph (`Observable.Return → Select → SelectMany.Zip → Select`) |
| `AsyncPipeline.processFrame`         | F# `task { }` — direct `Task.Run` fan-out and `Task.WhenAll` join |

The per-stage work (`PipelineStages.fs`) is **byte-for-byte identical** between
the two; only the orchestration around it differs. Any performance or
debuggability delta observed is attributable to the coordination library, not
to the work itself.

This is the .NET sibling of [`dd-akka-ws-server`](../akka-ws-server/) — same
shape (`parse → validate → enrich (lookupA ∥ lookupB) → score → serialize`),
same env var layout, same WS frame protocol, so the existing
[`ws-loadtest-rs`](../ws-loadtest-rs/) and
[`gleamlang-ws-loadtest`](../gleamlang-ws-loadtest/) harnesses drive it without
modification.

## Endpoints

### Probes & metadata

| Method | Path        | Purpose                                            |
| ------ | ----------- | -------------------------------------------------- |
| GET    | `/`         | Self-describing HTML landing page.                 |
| GET    | `/healthz`  | Liveness probe (text).                             |
| GET    | `/readyz`   | Readiness probe (text).                            |
| GET    | `/livez`    | Liveness JSON blob (runtime, machine, uptime).     |

### Per-message comparison (apples-to-apples benchmark targets)

| Method | Path             | Purpose                                                                                |
| ------ | ---------------- | -------------------------------------------------------------------------------------- |
| WS     | `/ws/rx`         | Each text frame builds a fresh `Observable.Return → … → ToTask`. **Worst-case Rx**, deliberately picked for apples-to-apples comparison with Akka Streams / `async.java`. |
| WS     | `/ws/async`      | Same work, native F# `task { }` + `Task.WhenAll`.                                      |
| GET    | `/v1/benchmark`  | Runs both pipelines `BENCHMARK_ITERATIONS` times against the same payload, returns a JSON timing summary. |

### Rx-native long-running pipelines

These endpoints showcase what Rx.NET actually buys you when you commit to its
model. The Subject + IObservable graph is **materialised once at connect** and
lives for the lifetime of the socket, so there's no per-message graph
allocation tax. They're not directly comparable to the per-message benchmark
targets above — they exist to demonstrate Rx-native shapes that don't compose
cleanly into a `string -> Task<string>` boundary.

| Method | Path                | Purpose                                                                              |
| ------ | ------------------- | ------------------------------------------------------------------------------------ |
| WS     | `/ws/rx-stream`     | Long-running `Subject<string>`-fed pipeline. One reply per input, but the operator chain is built once. Replies can arrive out-of-order because `Observable.Start` on `TaskPoolScheduler` is unordered — that's the fan-out doing its job. |
| WS     | `/ws/rx-window`     | Same input pipeline, but output goes through `Buffer(200ms, 16)` — one batched frame per window. Try this with `wscat`: send 5 frames quickly, get a single batch reply with `"count":5`. |
| WS     | `/ws/rx-throttle`   | Same input pipeline, output `Throttle(50ms)` — flood the socket and you only get the latest reply once you pause for 50 ms. Classic keystroke-debounce shape. |
| WS     | `/ws/rx-sample`     | Same input pipeline, output `Sample(100ms)` — a dashboard-friendly "latest value every tick" stream under heavy input. |
| WS     | `/ws/rx-burst`      | Same input pipeline, output goes through `Timestamp -> Buffer(250ms, 64) -> Scan` — stateful per-connection load windows with cumulative counts. |

### Live process telemetry (Rx `BehaviorSubject` + `ReplaySubject` + SSE)

A 1 Hz ticker drives a `BehaviorSubject<StatsSnapshot>` (latest-cached) and a
120-element `ReplaySubject<StatsSnapshot>` (rolling history). Counters are
`Interlocked`-incremented from every WS connection.

| Method | Path                       | Purpose                                                                |
| ------ | -------------------------- | ---------------------------------------------------------------------- |
| GET    | `/v1/rx-stats`             | Current snapshot — open connections, msgs/bytes in & out, uptime.      |
| GET    | `/v1/rx-stats/history`     | Last ~120 snapshots, replayed synchronously off the `ReplaySubject`.   |
| SSE    | `/sse/rx-stats`            | Server-Sent Events feed at 1 Hz. `curl -N` shows `data: {…}` per second. |

### The pipeline

Five stages, modelling a realistic WebSocket request flow:

```
parse → validate → enrich (lookupA ∥ lookupB) → score → serialize
```

`enrich` fans out **two** simulated downstream lookups (each sleeps 1-4 ms to
mimic an HTTP/DB hop). Everything else is sequential. The score stage
deliberately throws when `id == "poison"` so a stack-trace comparison has a
reliable failure to exercise.

### WS frame protocol

Each text frame in is a JSON object with at least `id` and `payload`. The reply
text frame is one of:

```json
{"ok":true,"result":{"id":"...","score":...,"lookupA":"...","lookupB":"..."}}
```

```json
{"ok":false,"pipeline":"rx","error":"InvalidOperationException: ..."}
```

so the same loadtest correlator works against this service and `dd-akka-ws-server`.

## Environment variables

| Var                       | Default     | Description                                                  |
| ------------------------- | ----------- | ------------------------------------------------------------ |
| `HTTP_HOST`               | `0.0.0.0`   | Bind address.                                                |
| `HTTP_PORT`               | `8087`      | Bind port.                                                   |
| `BENCHMARK_ITERATIONS`    | `200`       | Iterations for `GET /v1/benchmark`.                          |
| `MAX_BENCHMARK_ITERATIONS` | `1000`     | Upper bound applied to `BENCHMARK_ITERATIONS` at runtime.    |
| `BENCHMARK_PAYLOAD`       | sample JSON | Payload to drive the benchmark.                              |
| `MAX_WS_TEXT_FRAME_BYTES` | `65536`     | Maximum assembled inbound WebSocket text frame size.         |
| `RX_STREAM_OUTBOUND_QUEUE_CAPACITY` | `1024` | Per-connection bounded outbound queue for long-running Rx streams. If the client cannot drain replies, the socket is closed instead of letting memory grow without bound. |
| `DOTNET_gcServer`         | `1`         | Server GC (multi-core, throughput-tuned).                    |
| `DOTNET_TieredPGO`        | `1`         | Tiered JIT + dynamic PGO.                                    |

## Local build

```bash
cd remote/fsharp-ws-server
dotnet restore
dotnet publish -c Release -o ./out
HTTP_PORT=8087 dotnet ./out/dd-fsharp-ws-server.dll
```

Smoke test:

```bash
curl -fsS http://localhost:8087/healthz
curl -fsS http://localhost:8087/v1/benchmark | jq .
```

WebSocket smoke test (`wscat` / `websocat`):

```bash
wscat -c ws://localhost:8087/ws/rx
> {"id":"abc","payload":"hello"}
< {"ok":true,"result":{"id":"abc","score":...,"lookupA":"...","lookupB":"..."}}
```

## Container build

```bash
# Build from the repo root so the Dockerfile context can reach sibling repos.
docker build -f remote/fsharp-ws-server/Dockerfile -t dd-fsharp-ws-server:dev .
docker run --rm -p 8087:8087 dd-fsharp-ws-server:dev
```

## Reproducing the real-load comparison

The two existing loadtest services in this repo
(`remote/ws-loadtest-rs/` and `remote/gleamlang-ws-loadtest/`) speak the same
`LOAD_MODE=pipeline` protocol that `dd-akka-ws-server` uses. To point them at
this service:

```bash
docker run --rm \
    -v "$(pwd)"/remote/ws-loadtest-rs/target/release:/bin/bench:ro \
    -e LOAD_MODE=pipeline \
    -e CLIENT_COUNT=50 \
    -e MESSAGES_PER_SECOND_PER_CLIENT=10 \
    -e TARGET_WS_URL="ws://host.docker.internal:8087/ws/rx" \
    -e REPORT_INTERVAL_SECONDS=5 \
    -e HOLD_SECONDS=20 \
    debian:bookworm-slim timeout 25 /bin/bench/ws-loadtest-rs
```

Run the same command against `/ws/async` to diff the two pipelines.

## Kubernetes layout

* `k8s/ec2/dd-fsharp-ws-server.deployment.yaml`
* `k8s/ec2/dd-fsharp-ws-server.service.yaml`
* `k8s/ec2/kustomization.yaml` (Argo CD target — synced via
  `remote/argocd/apps/dd-fsharp-ws-server.application.yaml`)

The layout is flat (no `../` resource references) because ArgoCD's bundled
kustomize runs with `LoadRestrictionsRootOnly` and rejects path traversal
out of the kustomization root. Same posture as `dd-formal-methods-service`
which hit and fixed the same constraint in commit
[`73b78d6`](https://github.com/ORESoftware/k8s-cluster/commit/73b78d6).

The EC2 deployment mounts the repo as a hostPath at `/opt/dd-next-1` and runs
`dotnet publish` once on container start, matching the on-pod-build pattern
used by `dd-akka-ws-server`, `dd-spark-pipeline-server`, and
`dd-gleamlang-server`. NuGet packages are cached at `/tmp/nuget-packages` so
the read-only repo mount stays clean.

To roll the deployment after a code change, push the change to the `dev`
branch — Argo CD picks it up and the next pod start runs the publish step
against the new sources.

## Why two pipelines?

For the long-form comparison (Rx vs callback-style coordination, materialisation
cost, tail-latency under load, stack-trace depth, when to pick which) see the
[`dd-akka-ws-server` readme](../akka-ws-server/readme.md). The mechanism on .NET
is the same as on the JVM:

* **Rx.NET** materialises an Observable graph per call (`Observable.Return →
  Select → SelectMany.Zip → Select → ToTask`). The graph is cheap to allocate
  on a hot path but is still a graph — every push/pull between stages goes
  through the operator's scheduler.
* **F# `task { }`** is a thin wrapper around `ValueTask` continuations.
  `Task.Run` fans out onto the ThreadPool directly, `Task.WhenAll` joins, and
  there is no per-message graph allocation.

For short, per-WS-frame pipelines (~5 ms median work, two parallel sub-stages),
the `task { }` path is expected to win on every percentile and the gap widens
under sustained load — exactly the same trade-off the akka-ws-server readme
documents for `async.java` vs Akka Streams.

For long-running Rx usage (one Observable graph per WS *connection*,
`Subject.OnNext` per frame, `Buffer` / `Throttle` / `Window` operators across
the lifetime of the socket), the materialisation tax is paid once at connect
and Rx.NET holds its tail latency just like the callback path. That pattern
doesn't compose into a `string -> Task<string>` boundary, so it isn't what
the `/v1/benchmark` endpoint measures — but it's the right call when the WS
stream itself is the shape your business logic wants to react over. The
`/ws/rx-stream`, `/ws/rx-window`, `/ws/rx-throttle`, `/ws/rx-sample`, and `/ws/rx-burst`
endpoints exist specifically to demonstrate those shapes; see `RxAdvanced.fs`.

## Quick demo of the Rx-native endpoints

```bash
# 1. Long-running pipeline. Note the reply ordering — `hog-3` may arrive
#    before `hog-0` because the enrichment fan-out runs on the thread pool.
wscat -c ws://localhost:8087/ws/rx-stream
> {"id":"hog-0","payload":"a"}
> {"id":"hog-1","payload":"b"}
> {"id":"hog-2","payload":"c"}
< {"ok":true,"result":{"id":"hog-2",...}}
< {"ok":true,"result":{"id":"hog-0",...}}
< {"ok":true,"result":{"id":"hog-1",...}}

# 2. Time-windowed output. Send 5 frames fast, get one batched reply.
wscat -c ws://localhost:8087/ws/rx-window
> {"id":"a","payload":"1"}
> {"id":"b","payload":"2"}
> {"id":"c","payload":"3"}
> {"id":"d","payload":"4"}
> {"id":"e","payload":"5"}
< {"window":"200ms|16","count":5,"items":[...]}

# 3. Sampled output. Flood it; it emits at most one latest result per 100 ms.
wscat -c ws://localhost:8087/ws/rx-sample
> {"id":"sample-1","payload":"a"}
> {"id":"sample-2","payload":"b"}
< {"sample":"100ms","item":{"ok":true,"result":{...}}}

# 4. Stateful burst windows. Send several frames fast, get one compact summary.
wscat -c ws://localhost:8087/ws/rx-burst
> {"id":"burst-1","payload":"a"}
> {"id":"burst-2","payload":"b"}
< {"burst":"250ms|64","window":1,"count":2,"total":2,"items":[...]}

# 5. Live SSE feed of the process counters.
curl -N http://localhost:8087/sse/rx-stats
event: hello
data: connected

data: {"openConnections":0,"messagesIn":15,...}

data: {"openConnections":0,"messagesIn":15,...}
```
