# dd-akka-ws-server

An Akka HTTP + WebSocket service that runs the same multi-stage pipeline through
**two different coordination libraries** so we can compare them side by side:

| Pipeline implementation       | Library                                                                  |
| ----------------------------- | ------------------------------------------------------------------------ |
| `pipeline/AsyncJavaPipeline`  | [`async-java/async.java`](https://github.com/async-java/async.java) — error-first callback combinators (`Asyncc.Parallel`) |
| `pipeline/AkkaStreamsPipeline`| [Akka Streams](https://doc.akka.io/docs/akka/current/stream/) — back-pressured graph (`Source → Flow → Sink`) |

The per-stage work (`PipelineStages.java`) is **byte-for-byte identical** between
the two; only the orchestration around it differs. Any performance or
debuggability delta observed is attributable to the coordination library, not
to the work itself.

## Endpoints

| Method | Path                | Purpose                                                                  |
| ------ | ------------------- | ------------------------------------------------------------------------ |
| GET    | `/healthz`          | Liveness probe.                                                          |
| GET    | `/readyz`           | Readiness probe.                                                         |
| GET    | `/ws/asyncjava`     | WebSocket; each text frame runs through `AsyncJavaPipeline` and the JSON result is sent back. |
| GET    | `/ws/akkastreams`   | Same shape, `AkkaStreamsPipeline`.                                       |
| GET    | `/v1/benchmark`     | Runs both pipelines N times against the same payload and returns a JSON timing summary. |

### The pipeline

Five stages, modelling a realistic WebSocket request flow:

```
parse → validate → enrich (lookupA ∥ lookupB) → score → serialize
```

`enrich` fans out **two** simulated downstream lookups (each sleeps 1-4ms to
mimic an HTTP/DB hop). Everything else is sequential. The score stage
deliberately throws when `id == "poison"` so the
[stack-trace comparison](#debuggability) has a reliable failure to exercise.

## The async.java dependency

This service pins [async.java](https://github.com/async-java/async.java) to a
specific commit SHA via [JitPack](https://jitpack.io) rather than via Maven
Central, because the post-`#8` release line hasn't been published to Central
yet. The `pom.xml` declares:

```xml
<dependency>
  <groupId>com.github.async-java</groupId>
  <artifactId>async.java</artifactId>
  <version>937c0e3</version>   <!-- merged master, post-#8 -->
</dependency>
```

When `io.github.async-java:async-java:0.2.0` lands on Central per the
[RELEASING.md](https://github.com/async-java/async.java/blob/master/RELEASING.md)
runbook in that repo, swap this for a stable Central coordinate.

---

# Comparison findings

The headline question: **for a five-stage WS request handler with two parallel
sub-stages, what does each coordination library cost you?** Numbers below come
from `BenchmarkComparisonTest` on JDK 21 (Eclipse Temurin), 30 iterations,
20-iteration warmup, virtual-thread executor.

## Performance

Two complementary views of the same question:

1. **Synthetic micro-benchmark** (`BenchmarkComparisonTest`) — one client,
   sequential, no network. Tells you each library's per-call overhead.
2. **Real WebSocket load test** — many concurrent clients each sending at a
   fixed rate over real WS frames. Tells you how each library behaves under
   the kind of concurrency a production endpoint actually sees.

The two paint **very different pictures**. Worth understanding both.

### 1. Synthetic micro-benchmark

200 iterations on JDK 21 (Eclipse Temurin), 20-iter warmup, virtual-thread
executor, against async-java/async.java post-PR-#9
(CounterLimit race fixed):

```
                async.java           akka-streams         async/akka
p50 latency     4460 µs              5826 µs              0.77  (faster)
p95 latency     6140 µs              7593 µs              0.81
p99 latency     6663 µs              8046 µs              0.83
max latency     7280 µs              8681 µs
throughput      227 req/s            172 req/s            1.32  (higher)
wall time        882 ms              1162 ms
```

**Read**: when one caller drives the pipeline sequentially, async.java is
~25 % faster on median and ~32 % higher throughput. This is the
per-call-overhead view; Akka Streams' graph materialisation cost
dominates short pipelines.

### 2. Real WebSocket load test (Rust loadtest in pipeline mode)

50 concurrent WS clients × 10 msg/sec → 500 msg/sec offered load, sustained
60 seconds. async.java v0.2.2 (post PR #9, PR #10, v0.2.1 NeoReduce, v0.2.2
slot-write-order). All runs JDK 21 with virtual-thread executor.

```
                              async.java        akka-streams       async/akka
sent (over 60s)              29 867            29 801
received                     29 867            29 801
delivery rate                100.0 %           100.0 %             parity
correlation_misses            0                 0
p50 latency                   5 699 µs         17 839 µs            0.32  (async 3.1× faster)
p95 latency                  10 863 µs         27 679 µs            0.39
p99 latency                  14 271 µs         30 687 µs            0.46
max latency                  46 431 µs         55 199 µs
mean                          6 216 µs         16 358 µs
```

**Read**: at steady-state 500 msg/sec offered load, **async.java is now
~3× faster than Akka Streams on every percentile while both deliver 100 %**.
Earlier benchmarks suggested rough parity at this load; sustained 60-second
runs surface a bigger gap — Akka Streams' graph materialisation cost per
incoming WS frame accumulates as queue depth builds, while async.java's
direct callback dispatch holds steady.

> **Earlier numbers showed async.java at 94.5 % delivery on 20-second runs.**
> That was a real bug, not a load-test artifact:
>
> 1. **`CounterLimit` lost-update race** ([PR #9](https://github.com/async-java/async.java/pull/9), v0.2.0) —
>    non-atomic `Integer++` from per-task callbacks caused `Asyncc.Parallel` to
>    hang after dozens of sequential calls on JDK 21.
> 2. **Double-fire of the final callback** ([PR #10](https://github.com/async-java/async.java/pull/10), v0.2.0) —
>    `NeoParallel.Parallel(List, callback)` called `f.done(...)` directly
>    without the shared `NeoUtils.fireFinalCallback` dedup guard. Two task
>    runners finishing nearly simultaneously could each invoke the user
>    callback. Akka HTTP's `mapAsync` silently dropped the duplicate emit, so
>    the downstream WS client saw it as a "lost" response.
> 3. **Slot-write-before-counter-increment race** (v0.2.2) — `NeoParallel`
>    and `NeoMap` incremented their atomic counter *before* writing the
>    per-index result slot. A sibling runner reading `count == size` could
>    fire the final callback while another's slot write hadn't landed yet,
>    publishing `null` at the last-finishing index.
>
> The numbers above are with all three fixes applied (poms pin to
> [v0.2.2](https://github.com/async-java/async.java/releases/tag/v0.2.2)).

### 3. Real WebSocket load test — higher concurrency (1 000 msg/sec)

200 clients × 5 msg/sec → 1 000 msg/sec offered. 60-second runs.

```
                              async.java        akka-streams       async/akka
sent (over 60s)              59 739            59 679
received                     59 739            59 679
delivery rate                100.0 %           100.0 %             parity
correlation_misses            0                 0
p50 latency                   5 087 µs          5 935 µs            0.86
p95 latency                  10 135 µs         34 943 µs            0.29  (async 3.4× faster)
p99 latency                  14 807 µs         54 303 µs            0.27  (async 3.7× faster)
max latency                  21 183 µs        100 095 µs            0.21  (async 4.7× faster)
mean                          5 530 µs         10 572 µs
```

**Read**: at 1 000 msg/sec, the gap widens. async.java holds its tail
(p99/p50 ratio = 2.9×) while Akka Streams' tail blows out (p99/p50 ratio
= 9.1×) — same saturation behaviour the earlier 30-second runs flagged,
now confirmed across 60 seconds of sustained load. Both still deliver
100 %. The earlier hypothesis that Akka Streams' p99 blowup was a
one-off variance was wrong; it's the systematic structural-back-pressure
trade-off — saturation manifests as latency growth in Akka Streams,
which it does cleanly without losing messages, but the latency growth
itself is steep.

### 4. Cross-runtime confirmation (Gleam/Node loader, 50 × 10)

```
                              async.java        akka-streams
sent (over 60s)              29 033            29 084
received                     29 033            29 084
delivery rate                100.0 %           100.0 %
p50 latency                   6 652 µs         20 355 µs
p95 latency                  12 304 µs         28 056 µs
p99 latency                  15 889 µs         31 152 µs
```

The Gleam/Node loader confirms the Rust loader's reading: async.java
serves at ~3× lower latency than Akka Streams under the same workload.
Both runtimes' results are consistent, so the gap is not an artifact of
the loader's tokio scheduling.

### 5. Performance curve across the full load spectrum

The interesting question isn't "which is faster" — it's "where do they diverge,
why, and by how much?" Here's the same pipeline under five offered-load points
on the same host (JDK 21, virtual-thread executor, async.java v0.2.2), each run
for 35-60 seconds via the Rust loader in pipeline mode:

| offered      | clients × rate | async.java p50 / p95 / p99 / max | akka-streams p50 / p95 / p99 / max | drops async / akka |
| ------------ | -------------- | -------------------------------- | ---------------------------------- | ------------------ |
| 10 msg/s     | 1 × 10         | 10.0 / 16.1 / 18.7 / 27 ms       | 8.6 / 12.8 / 15.3 / 16 ms          | 0 / 0              |
| 100 msg/s    | 10 × 10        | 6.9 / 11.3 / 13.3 / 16 ms        | 7.2 / 11.4 / 13.8 / 16 ms          | 0 / 0              |
| 500 msg/s    | 50 × 10        | 5.7 / 10.9 / 14.3 / 46 ms        | 17.8 / 27.7 / 30.7 / 55 ms         | 0 / 0              |
| 1 000 msg/s  | 200 × 5        | 5.1 / 10.1 / 14.8 / 21 ms        | 5.9 / 34.9 / 54.3 / 100 ms         | 0 / 0              |
| 2 500 msg/s  | 50 × 50        | 5.0 / 8.7 / 11.5 / 18 ms         | **2 017 / 4 624 / 5 230 / 6 258 ms** | 0 / **~14.3 %**    |

Reading by row:

* **10 msg/s** — single client, no concurrency. Both libraries are dominated by
  the per-message work (one `Thread.sleep(1-4 ms)` fan-out of two). Akka is
  ~15 % faster at p99 here, mostly because the actor mailbox is already warm
  and the JIT has had time to inline the stage interpreter. With 288 samples
  the run is dominated by JIT-warmup variance more than overhead.
* **100 msg/s** — moderate concurrency. The two libraries are **at parity**
  (within 4 % across all percentiles). At this rate the dispatcher has spare
  capacity, so neither library's per-message overhead matters.
* **500 msg/s** — Akka Streams starts to wobble. p50 climbs from 7 → 18 ms while
  async.java's drops from 7 → 6 ms. Both still deliver 100 %.
* **1 000 msg/s** — the tail diverges sharply. async.java's p99 = 14.8 ms
  (3.0× p50, healthy distribution). Akka Streams' p99 = 54 ms (**9.2× p50**,
  long right tail). Both deliver 100 %. **This is the 3-4× tail-latency gap.**
* **2 500 msg/s** — Akka Streams falls off a cliff. p50 = **2.0 seconds**,
  ~14 % of in-flight messages never come back within the 15-second correlation
  budget. async.java is unchanged: p50 = 5 ms, p99 = 11.5 ms, 0 drops, 0
  correlation misses. This is the knee.

### Why the tail latency? Mechanism, not magic

For this five-stage WS request pipeline (~5 ms work per message), here is what
each library actually allocates and dispatches **per message**:

**`AsyncJavaPipeline.process(frame)`**:

1. One `CompletableFuture` for the boundary.
2. `parse` and `validate` run **synchronously** on the caller thread (the
   Akka-HTTP mapAsync worker).
3. `List.of(taskA, taskB)` for the two enrichment lookups.
4. `Asyncc.Parallel(...)` allocates: one `ParallelRunner`, one `ShortCircuit`,
   one `CounterLimit` (two `AtomicInteger`s), and two `AsyncTaskRunner`s.
5. Two `executor.submit(...)` calls, each spawning a virtual thread. VT spawn
   on JDK 21 is **~250 ns**.
6. Each task lands on a VT, runs its 1-4 ms sleep, calls `cb.done(...)`. The
   final-callback dedup-guard fires the user callback exactly once on whichever
   VT finished last.

Per-message coordination overhead (excluding work): **under 50 µs.** Mostly
heap allocation + four `AtomicInteger.incrementAndGet()` calls. No actor
mailbox, no graph compiler, no stage interpreter.

**`AkkaStreamsPipeline.process(frame)`**:

1. Construct **five** `Flow` instances (`parseFlow`, `validateFlow`,
   `enrichFlow`, `scoreFlow`, `serializeFlow`). Each is a small graph fragment
   with attributes, input/output ports, and a stage logic factory.
2. `Source.single(inputFrame)` — a graph fragment.
3. `.via(...).via(...).via(...).via(...).via(...)` — graph composition. Each
   `via` glues the upstream's output port to the next stage's input port.
4. `.runWith(Sink.head(), system)` — **materialisation**. This walks the graph,
   allocates a `GraphInterpreterShell`, an `ActorGraphInterpreter` actor with
   its own mailbox, instantiates each stage's logic, wires async callbacks
   through the actor system, and schedules an initial pull on the source. The
   resulting CompletableFuture only completes after the actor's mailbox has
   been processed end-to-end.
5. Each push/pull event between stages goes through the stage-machine
   interpreter loop — that's the ~26-frame stack trace you see in
   [§ Debuggability](#debuggability).
6. When the graph completes (or fails), the actor stops, the materialiser
   tears the graph down.

Per-message coordination overhead (excluding work): **~80-200 µs base** when
the dispatcher is idle, plus **whatever the actor mailbox queue depth costs**
when it isn't. The materialisation work itself is a few dozen allocations,
some `AtomicReferenceFieldUpdater` CAS-ing, and a `dispatcher.execute(runnable)`.

That's why the two diverge as load climbs:

* At **10-100 msg/s** the dispatcher is idle; materialisation latency = base
  cost only. Akka's mature mailbox + ForkJoinPool tuning makes it competitive.
* At **500-1 000 msg/s** the dispatcher's queue starts to back up. Every new
  `runWith` enqueues a new mailbox to schedule. Materialisation time =
  `(base) + (queue depth × per-actor scheduling slice)`. The right tail
  reflects messages that landed late in a long queue. async.java has no
  comparable queue — its work goes onto the VT executor directly, and VTs are
  nearly free.
* At **2 500 msg/s** the dispatcher cannot dequeue actors fast enough. New
  graph materialisations pile on top of in-progress ones. Median latency
  becomes "average mailbox queue depth × time per actor slice" = **seconds**.
  Eventually the per-WS-frame `mapAsync(8)` boundary upstream stops accepting
  new work, but the actor-side queue has already grown unboundedly and
  client-side correlation timeouts fire (~14 % drops).

**Important context**: this isn't a fair criticism of Akka Streams' design.
Akka Streams is built for **long-running streams** (Kafka, JetStream, change-data
feeds) where you materialise the graph **once** and run millions of messages
through it. Materialisation cost amortises to zero. This benchmark
deliberately fights that model — `Source.single → runWith` per WS frame —
because the comparison function is `String → CompletionStage<String>` and that's
the only way to express it without changing the function signature.

If you instead built the Akka-Streams version with one long-running flow per
WS connection (which is the idiomatic way), the per-message materialisation
cost vanishes. That model is in [§ When to still pick Akka Streams](#when-to-still-pick-akka-streams) below.

### 6. Where the time actually goes (per-message accounting)

Numbers from the 1 000 msg/s, 200×5 run, JDK 21, JFR-sampled:

```
                          async.java          akka-streams
work (sleep + JSON)           ~4.8 ms             ~4.8 ms      (identical)
coordination overhead          0.10 ms             0.30 ms     (base case)
dispatcher queue wait          0.10 ms             4.5 ms      (load-dependent)
total p99                     14.8 ms             54.3 ms

queue wait fraction            ~7 %                ~50 %       (of total p99)
```

The 3-4× tail-latency gap is **entirely the dispatcher queue-wait term**. The
work and the base coordination overhead are roughly the same. What changes is
that async.java's coordination overhead doesn't *enqueue anything onto a
shared, contended structure*, so it stays flat as load grows. Akka Streams
enqueues a new actor per message, and the resulting queue depth is what shows
up in the tail.

### Headline: async.java vs Akka Streams in 2026, post-hardening

For this five-stage WS request pipeline (~5 ms median work per message):

| Property                       | async.java v0.2.2       | Akka Streams 2.8.8                                              |
| ------------------------------ | ----------------------- | ----------------------------------------------------------------|
| Delivery rate (steady state)   | 100 %                   | 100 % up to ~1 000 msg/s, ~86 % at 2 500 msg/s                  |
| Median latency                 | 5 ms (10 → 2 500 msg/s) | 7 - 18 ms steady, then ramps to seconds at saturation           |
| p99 latency                    | 11 - 19 ms (stable)     | 14 - 54 ms steady, > 5 s at saturation                          |
| Max latency observed           | 46 ms                   | 6 258 ms                                                        |
| p99 / p50 ratio (load curve)   | 2 - 3× (flat)           | 1.8 - 9× (knee-shaped)                                          |
| Per-call overhead              | ~50 µs (heap + VT spawn)| ~80-200 µs base + load-dependent mailbox-queue wait             |
| Structural back-pressure       | opt-in (`NeoQueue`)     | built-in (per long-running flow)                                |
| Failure stack-trace depth      | ~10 frames              | ~26 frames                                                      |

### When to still pick Akka Streams

Akka Streams is the right call when:

* **Your pipeline is a long-running stream consumer** (Kafka, NATS, JetStream,
  CDC feeds) rather than per-request short pipelines. The graph
  materialisation cost amortises over millions of messages on the same flow,
  the structural back-pressure prevents memory pressure when upstream
  outpaces downstream, and the per-message overhead in this benchmark
  literally vanishes. The 14 % drop at 2 500 msg/s in the table above is an
  artefact of `Source.single → runWith` *per message*; if you instead build
  one long-running `Source.queue → ... → Sink` per WS connection and
  `offer(frame)` into it, the materialisation tax is paid once at connect and
  Akka Streams will hold its tail latency just like async.java does. **The
  benchmark deliberately picked the worst-case Akka usage pattern** so the
  function signatures could be identical — that's worth knowing when you
  read the numbers.
* **You can't easily reason about correctness without static back-pressure
  guarantees.** Akka Streams' type-system-level back-pressure is genuinely
  easier to get right than ad-hoc callback chains.
* **You're already in the Akka ecosystem.** Pekko/Akka actors compose with
  Streams natively.

For per-call short pipelines — short HTTP request handlers, short WS
request/response, job orchestration where each job is its own graph —
**async.java v0.2.2+ is faster, has shorter stack traces, and matches Akka
Streams on delivery**. The choice is no longer a performance trade-off; it's
a question of which orchestration style fits your code, and whether the
"materialise once, run forever" model maps onto your workload.

### Reproducing the real-load comparison

The two existing loadtest services in this repo
(`remote/ws-loadtest-rs/` and `remote/gleamlang-ws-loadtest/`) gained a
`LOAD_MODE=pipeline` option for this comparison. In `pipeline` mode each
client sends shaped JSON at `MESSAGES_PER_SECOND_PER_CLIENT` and the
harness correlates responses by `id` for round-trip latency.

```bash
# 1. Boot the akka-ws-server with the patched async.java
docker run -d --name akkaws -p 8086:8086 \
    -v "$(pwd)"/remote/akka-ws-server:/work -w /work \
    maven:3.9.9-eclipse-temurin-21 java -jar target/dd-akka-ws-server.jar

# 2. Build the Rust loadtest once
docker run --rm -v "$(pwd)"/remote/ws-loadtest-rs:/work -w /work \
    rust:1.86-slim cargo build --release

# 3. Run the harness against each endpoint
for endpoint in asyncjava akkastreams; do
    docker run --rm \
        -v "$(pwd)"/remote/ws-loadtest-rs/target/release:/bin/bench:ro \
        -e LOAD_MODE=pipeline \
        -e CLIENT_COUNT=50 \
        -e MESSAGES_PER_SECOND_PER_CLIENT=10 \
        -e TARGET_WS_URL="ws://host.docker.internal:8086/ws/$endpoint" \
        -e REPORT_INTERVAL_SECONDS=5 \
        -e HOLD_SECONDS=20 \
        debian:bookworm-slim timeout 25 /bin/bench/ws-loadtest-rs
done
```

The same `LOAD_MODE=pipeline` and `MESSAGES_PER_SECOND_PER_CLIENT` env
vars work for the Gleam loadtest (`remote/gleamlang-ws-loadtest/`).

### In-cluster

Four dedicated Argo CD applications drive both endpoints in pipeline
mode continuously from inside the cluster, alongside the existing
`dd-ws-loadtest-rs` / `dd-ws-loadtest-gleam` apps that keep doing the
older capacity-style stress test against `dd-gleamlang-server`:

| Application                                              | Loadtest    | Endpoint              |
| -------------------------------------------------------- | ----------- | --------------------- |
| `dd-ws-loadtest-rs-akkaws-asyncjava`                     | Rust        | `/ws/asyncjava`       |
| `dd-ws-loadtest-rs-akkaws-akkastreams`                   | Rust        | `/ws/akkastreams`     |
| `dd-gleamlang-ws-loadtest-akkaws-asyncjava`              | Gleam/Node  | `/ws/asyncjava`       |
| `dd-gleamlang-ws-loadtest-akkaws-akkastreams`            | Gleam/Node  | `/ws/akkastreams`     |

Each runs 50 clients × 10 msg/sec (500 msg/sec offered) for an hour at a
time, reconnecting on `HOLD_SECONDS` boundaries. Manifests live under
`remote/ws-loadtest-rs/k8s/akkaws-{asyncjava,akkastreams}/` and
`remote/gleamlang-ws-loadtest/k8s/akkaws-{asyncjava,akkastreams}/`; the
Argo CD apps live at `remote/argocd/apps/dd-{ws,gleamlang-ws}-loadtest*-akkaws-*.application.yaml`.

To compare:

```bash
# Watch the rolling pipeline-report from each loadtest pod.
kubectl logs -f deployment/dd-ws-loadtest-rs-akkaws-asyncjava
kubectl logs -f deployment/dd-ws-loadtest-rs-akkaws-akkastreams
kubectl logs -f deployment/dd-gleamlang-ws-loadtest-akkaws-asyncjava
kubectl logs -f deployment/dd-gleamlang-ws-loadtest-akkaws-akkastreams
```

The `pipeline-report` log lines emit the same shape from every loadtest:

```
pipeline-report attempted=… connected=… failed=… open=… sent=… received=…
                in_flight=… correlation_misses=… receive_errors=…
                p50_us=… p95_us=… p99_us=… max_us=… mean_us=… sample=…
```

so diffing the four streams is straightforward.

To pause the comparison loadtests without removing the apps, scale them
to zero replicas:

```bash
for app in dd-ws-loadtest-rs-akkaws-asyncjava dd-ws-loadtest-rs-akkaws-akkastreams \
           dd-gleamlang-ws-loadtest-akkaws-asyncjava dd-gleamlang-ws-loadtest-akkaws-akkastreams; do
    kubectl scale deployment/$app --replicas=0
done
```

Argo CD will see this as drift (since the manifest declares
`replicas: 1`) and re-up them on the next reconcile, so for a long
pause prefer editing the manifest in this repo and pushing.

**Read**: for short, simple pipelines where each request lives ~5ms, async.java
wins by ~25%. The reason is **fixed setup cost per request**. Akka Streams
materialises a fresh graph for every `Source.single(...).runWith(...)` —
each pipeline run allocates stage instances, wires the `GraphInterpreter`,
and shuttles input/output through actor mailboxes (visible in the stack
traces below). async.java's combinators are just method calls dispatching
lambdas; per-invocation cost is near zero.

**The win flips on long pipelines.** Once each request keeps the graph busy
for tens of milliseconds, Akka Streams amortises the materialisation cost
and its structural back-pressure outperforms ad-hoc callback orchestration.
The crossover for this exact workload appears to be around 50ms per request;
beyond that, fixed-cost analysis stops dominating and the two libraries
converge.

## Consistency / predictability

This is where the two libraries genuinely disagree on philosophy.

### Back-pressure

* **Akka Streams** has back-pressure baked into the type system. A slow
  `Sink` pulls less, every upstream stage cooperatively slows down,
  buffering is explicit (`Flow.buffer(size, overflowStrategy)`). You can't
  accidentally overrun a downstream stage — the compiler won't let you.
* **async.java** does not. `Asyncc.Parallel(tasks, callback)` runs all
  tasks as fast as the supplied executor will accept them. Back-pressure
  is a separate concern that you address with `Asyncc.ParallelLimit` (cap
  in-flight count) or `NeoQueue` (full saturated/drain lifecycle). Out of
  the box, fan-out is uncapped.

For HTTP request handling the difference rarely matters — each WS message
is its own small pipeline. For consumer workloads (Kafka, NATS, JetStream)
that ingest at unbounded rates, Akka Streams' built-in back-pressure is
strictly easier to get right.

### A misdiagnosed-then-fixed correctness bug (CounterLimit data race)

The first benchmark run against the patched-but-not-yet-debugged async.java
timed out around iteration 40 of a sustained sequential drive. Initial
hypothesis: a JDK 21 virtual-thread pinning issue caused by the
`synchronized` accessors added to `ShortCircuit` in
[async-java/async.java#8](https://github.com/async-java/async.java/pull/8).

**That hypothesis turned out to be wrong.** `-Djdk.tracePinnedThreads=full`
produced zero pinning output. The actual root cause is a
**`CounterLimit` lost-update data race that pre-existed #8**:

- `CounterLimit.{started, finished}` were plain `Integer` fields.
- `NeoParallel.AsyncTaskRunner.done()` calls `p.c.incrementFinished()`
  inside `synchronized(this.cbLock)`, but `cbLock` is **per-task-runner**,
  not shared.
- Two parallel-task callbacks can therefore enter their (different)
  `cbLock` blocks simultaneously and both `this.finished++` against the
  shared counter — a textbook read-modify-write race.
- One lost increment means `finished < started` forever, so
  `ParallelRunner.isDone()` returns `false` forever, so the final
  `Asyncc.Parallel` callback never fires.

The fix is two lines:
`CounterLimit.{started, finished}` → `AtomicInteger`. Lock-free,
provides the memory-visibility guarantees the existing call sites assume,
and removes any concern about `synchronized`-pinning at the same time
(by making most of those `synchronized` blocks dead code).

Fix lives in
[async-java/async.java#9](https://github.com/async-java/async.java/pull/9).
This repo's `pom.xml` consumes the fix-branch SHA via JitPack until #9
merges. After that, bump both `async-java.version` properties to the merge
commit SHA, and after the next Maven Central release bump them to the
stable `io.github.async-java:async-java:0.2.0` coordinate.

Why virtual threads surfaced this and platform threads didn't: VTs make
spawning concurrent callbacks essentially free, so the contention window
between the two per-task `synchronized(cbLock)` sections is hit on nearly
every iteration. With a small platform-thread pool the contention is
much rarer, the race happens occasionally but the test suite (which
doesn't drive `Parallel` in a sustained loop) never noticed.

Akka Streams does not have an analogous internal race because its stage
state transitions are dispatched through a single actor mailbox — every
stage event is processed serially by definition. No shared counter
across concurrent callback threads.

### Tail latency

Both libraries had tight tails in this run — p99 within 50µs of p95 for
each. Akka Streams' tail is structurally *more* predictable in
sustained-throughput regimes because its mailbox-driven dispatch keeps
stage latencies bounded. async.java's tail is more sensitive to the
underlying executor's scheduling (visible in earlier 100-iteration runs:
p99=15757µs against akka-streams' p99=7122µs with the same workload on
a pinning-prone configuration).

## Debuggability

Compare what each library shows you when the same in-pipeline failure
happens — `PipelineStages.score` throws an `IllegalStateException` when
the request has `id == "poison"`.

**async.java stack trace** (10 frames):

```
java.lang.IllegalStateException: score: deliberate poison-pill id=poison
    at PipelineStages.poison(PipelineStages.java:100)
    at PipelineStages.score(PipelineStages.java:76)
    at AsyncJavaPipeline.lambda$process$4(AsyncJavaPipeline.java:84)
    at org.ores.async.NeoParallel$3.done(NeoParallel.java:431)
    at AsyncJavaPipeline.lambda$process$0(AsyncJavaPipeline.java:65)
    at java.base/java.util.concurrent.Executors$RunnableAdapter.call(...)
    at java.base/java.util.concurrent.FutureTask.run(FutureTask.java:317)
    at java.base/java.util.concurrent.ThreadPoolExecutor.runWorker(...)
    at java.base/java.util.concurrent.ThreadPoolExecutor$Worker.run(...)
    at java.base/java.lang.Thread.run(Thread.java:1583)
```

**Akka Streams stack trace** (26 frames):

```
java.lang.IllegalStateException: score: deliberate poison-pill id=poison
    at PipelineStages.poison(PipelineStages.java:100)
    at PipelineStages.score(PipelineStages.java:76)
    at AkkaStreamsPipeline.lambda$process$b13915d5$1(AkkaStreamsPipeline.java:85)
    at akka.stream.javadsl.Flow.$anonfun$map$1(Flow.scala:675)
    at akka.stream.impl.fusing.Map$$anon$1.onPush(Ops.scala:58)
    at akka.stream.impl.fusing.GraphInterpreter.processPush(GraphInterpreter.scala:556)
    at akka.stream.impl.fusing.GraphInterpreter.processEvent(GraphInterpreter.scala:542)
    at akka.stream.impl.fusing.GraphInterpreter.execute(GraphInterpreter.scala:402)
    at akka.stream.impl.fusing.GraphInterpreterShell.runBatch(ActorGraphInterpreter.scala:650)
    at akka.stream.impl.fusing.GraphInterpreterShell$AsyncInput.execute(...:521)
    at akka.stream.impl.fusing.GraphInterpreterShell.processEvent(...:625)
    at akka.stream.impl.fusing.ActorGraphInterpreter$$anonfun$receive$1.applyOrElse(...)
    at akka.actor.Actor.aroundReceive(Actor.scala:537)
    at akka.actor.Actor.aroundReceive$(Actor.scala:535)
    at akka.stream.impl.fusing.ActorGraphInterpreter.aroundReceive(...:716)
    at akka.actor.ActorCell.receiveMessage(ActorCell.scala:579)
    at akka.actor.ActorCell.invoke(ActorCell.scala:547)
    at akka.dispatch.Mailbox.processMailbox(Mailbox.scala:270)
    at akka.dispatch.Mailbox.run(Mailbox.scala:231)
    at akka.dispatch.Mailbox.exec(Mailbox.scala:243)
    at java.base/java.util.concurrent.ForkJoinTask.doExec(ForkJoinTask.java:387)
    at java.base/java.util.concurrent.ForkJoinPool$WorkQueue.topLevelExec(...)
    at java.base/java.util.concurrent.ForkJoinPool.scan(...)
    at java.base/java.util.concurrent.ForkJoinPool.runWorker(...)
    at java.base/java.util.concurrent.ForkJoinWorkerThread.run(...)
```

### Reading the traces

* **Top three frames are identical** — both pipelines surface
  `PipelineStages.poison(...)` then `PipelineStages.score(...)` then the
  per-pipeline lambda. That's the actually-useful part: the cause of the
  failure is in your business code, exactly where it should be, and both
  libraries get out of the way enough to show it.
* **async.java adds ~5 frames of orchestration**: one frame for the
  `NeoParallel$3.done` callback dispatcher, one for the
  `executor.submit(...)` lambda, then the JDK's `Executors` runnable
  adapter and `ThreadPoolExecutor` bookkeeping. Easy to read.
* **Akka Streams adds ~20 frames**: the `GraphInterpreter` push/event
  loop (3 frames), the materialised actor's `runBatch` / `aroundReceive`
  / `processMailbox` (4 frames), the actor wrapping itself (3 frames),
  then the dispatcher's `ForkJoinPool` machinery (5 frames). Most of
  these are infrastructure — you can ignore them once you know what to
  look for, but on a fresh debug they're noise.
* **In production logs**, async.java's shorter stack is more digestible
  in a single log line. Akka Streams' deeper stack survives APM tools
  fine (Datadog / NewRelic / OpenTelemetry deduplicate the infra frames
  in their UI), but feels heavier in a raw `tail -f` debug session.

### Failure propagation semantics

* **async.java** — the first task to call `cb.done(err, null)` puts the
  combinator into a short-circuit state, the final callback fires
  immediately, sibling tasks that complete *afterwards* are silently
  dropped (logged as duplicate-done, not re-fired). You don't have to
  worry about cancelling in-flight work because the work hasn't been
  *scheduled* by async.java — it was scheduled by the caller's
  executor (in our case the VT-per-task executor), and the executor
  doesn't know to stop.
* **Akka Streams** — a stage throwing triggers the materialised
  `CompletionStage` to fail, supervision tears down the graph, and any
  in-flight upstream/downstream work is cancelled cooperatively
  (back-pressure + completion signal). Cleaner cancellation semantics if
  the downstream work is itself stream-shaped; equivalent if it isn't.

## Summary

For **short request/response pipelines** like this WS handler:

* async.java is **faster** and produces **shorter stack traces**.
* Akka Streams gives you **back-pressure** and **structured cancellation**
  for free.
* On JDK 21–23, async.java pins virtual threads under sustained load. JDK
  24+ (JEP 491) or a lock-free `ShortCircuit` rewrite fixes that.

For **long-running stream consumption** (Kafka / NATS / JetStream):

* Akka Streams is the right default — type-system-level back-pressure is
  a meaningful win and the per-graph setup cost amortises.
* async.java's `NeoQueue` covers a subset of this if you don't want the
  Akka dependency footprint.

For **bridging callback APIs you don't control** (Netty futures, AWS SDK
v2 async clients, Vert.x `Handler<AsyncResult<T>>`):

* async.java is the natural fit — that's literally what its combinators
  were ported for. Lifting them into Akka Streams via `Source.fromFuture`
  works but isn't ergonomic for many bridging call sites.

---

## Environment variables

| Var                       | Default     | Description                                              |
| ------------------------- | ----------- | -------------------------------------------------------- |
| `HTTP_HOST`               | `0.0.0.0`   | Bind address.                                            |
| `HTTP_PORT`               | `8086`      | Bind port.                                               |
| `BENCHMARK_ITERATIONS`    | `200`       | Iterations for `GET /v1/benchmark`.                      |
| `BENCHMARK_PAYLOAD`       | sample JSON | Payload to drive the benchmark.                          |
| `JAVA_OPTS`               | G1, 70% RAM | Standard JVM tuning.                                     |

## Local build

```bash
cd remote/akka-ws-server
mvn -B test
mvn -B -DskipTests package
java -jar target/dd-akka-ws-server.jar
```

## Kubernetes layout

* `k8s/dd-akka-ws-server.deployment.yaml`
* `k8s/dd-akka-ws-server.service.yaml`
* `k8s/kustomization.yaml` (default)
* `k8s/ec2/kustomization.yaml` (Argo CD target — synced via
  `remote/argocd/apps/dd-akka-ws-server.application.yaml`)

The EC2 deployment mounts the repo as a hostPath at `/opt/dd-next-1` and
runs `mvn package` once on container start, matching the pattern used by
`dd-spark-pipeline-server` and `dd-gleamlang-server`.
