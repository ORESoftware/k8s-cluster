# dd-lock-loadtest-gleam

Mutex-broker load tester written in Gleam, compiled to Erlang/BEAM.
Deployed in-cluster as `dd-lock-loadtest-gleam`. Companion to
`dd-lock-loadtest-rs` (Rust) and `dd-lock-loadtest-trigger` (Node).

## Why a third runtime

Each runtime stresses the broker's wire format and queueing
guarantees in a slightly different way:

- **Rust** (`dd-lock-loadtest-rs`) — Tokio multi-thread runtime, raw
  `tokio::net::TcpStream` plus a hand-rolled NDJSON reader. Best for
  sustained high-RPS testing and percentile latency measurement.
- **Node** (`dd-lock-loadtest-trigger`) — uses the canonical
  `live-mutex` JS client, which is the *de facto* reference
  implementation of the wire protocol.
- **Gleam/BEAM** (this) — uses Erlang `gen_tcp` in `{packet, line}`
  mode for line-framed receives. Verifies the brokers round-trip
  cleanly when poked from a runtime that's neither Rust nor Node.

If all three brokers respond identically to all three load testers,
that's a strong signal that the wire format is genuinely runtime-
agnostic. A divergence is worth investigating.

## Operating model

Unlike the Rust load tester, this one has **no HTTP trigger API**. It
runs benchmarks on a fixed schedule:

1. On startup, read config from env (`BROKER_HOST`, `BENCH_*`).
2. Run one bench window of `BENCH_DURATION_MS` ms.
3. Print a JSON line summary to stdout.
4. Sleep `BENCH_INTERVAL_MS` ms.
5. Loop forever (or until SIGTERM).

A single bench window spawns `BENCH_WORKERS` BEAM processes (one
TCP connection each), each looping acquire/release across
`BENCH_KEYS` keys. Workers stop when the deadline (window start
+ duration) is reached.

## Stdout output

```
{"type":"startup","brokerHost":"…","brokerPort":6970,"workers":16,"keys":32,"durationMs":10000,"intervalMs":30000,"ttlMs":4000}
{"type":"bench-summary","runId":"…","brokerHost":"…","brokerPort":6970,"workers":16,"keys":32,"durationMs":10000,"startedAtMicro":…,"finishedAtMicro":…,"acquired":424500,"released":424500,"failedAcquires":0,"failedReleases":0,"actualRps":42450,"latencyUsP50":42,"latencyUsP95":65,"latencyUsP99":81,"latencyUsMax":4310,"uniqueKeysObserved":32}
```

## Comparing brokers

You target a different broker by overriding `BROKER_HOST` /
`BROKER_PORT`. The simplest way is to apply the deployment three
times under different names:

```bash
# Rust broker (default — leave the bundled deployment alone)
kubectl get pods -l app=dd-lock-loadtest-gleam

# Node submodule broker (improved fork)
kubectl create deployment dd-lock-loadtest-gleam-vs-submodule \
  --image=ghcr.io/gleam-lang/gleam:v1.16.0-erlang-alpine -- \
  /bin/sh -lc 'cd /opt/dd-next-1/remote/deployments/lock-loadtest-gleam && exec gleam run --target erlang'
kubectl set env deploy/dd-lock-loadtest-gleam-vs-submodule \
  BROKER_HOST=dd-live-mutex-submodule.default.svc.cluster.local

# npm baseline broker (live-mutex@0.2.25 from npm)
kubectl set env deploy/dd-lock-loadtest-gleam-vs-baseline \
  BROKER_HOST=dd-live-mutex.default.svc.cluster.local
```

A nicer GitOps approach is to bundle a Kustomize overlay per broker
target. See `docs/lock-broker-bench-procedure.md` for the doc'd
procedure.

## Running locally

Requires Gleam 1.16+ with the Erlang toolchain (`gleam` plus a working
`erlc` on PATH).

```bash
cd remote/deployments/lock-loadtest-gleam
BROKER_HOST=127.0.0.1 BROKER_PORT=16970 \
  BENCH_WORKERS=4 BENCH_KEYS=8 \
  BENCH_DURATION_MS=2000 BENCH_INTERVAL_MS=1000 \
  gleam run --target erlang
```

Tail the output with `jq` to make the summaries readable:

```bash
gleam run --target erlang | jq -c 'select(.type == "bench-summary")'
```

## Implementation notes

- Concurrency: Gleam's `task` module moved between minor versions of
  `gleam_otp`; we use plain BEAM processes via
  `gleam/erlang/process.spawn` and a typed `Subject` mailbox. This
  is dependency-cheap and works on every gleam_otp release.
- TCP transport: provided by the in-tree
  `dd_rust_network_mutex_client` library (path dep). It speaks the
  same wire format both brokers expose, so swapping `BROKER_HOST`
  is the only knob you need.
- FFI: `src/lock_loadtest_gleam_env_ffi.erl` adds tiny helpers for
  `os:getenv/1` (returning a Gleam `String`, never `false`),
  microsecond wall-clock, plain BEAM sleep, and a handcrafted v4
  UUID generator. Keeping these in Erlang avoids pulling extra Gleam
  dependencies.
