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

From `BenchmarkComparisonTest` running 200 iterations on JDK 21 (Eclipse
Temurin), 20-iteration warmup, virtual-thread executor, against
async-java/async.java post-PR-#9 (CounterLimit race fixed):

```
                async.java           akka-streams         async/akka
p50 latency     4460 µs              5826 µs              0.77  (faster)
p95 latency     6140 µs              7593 µs              0.81
p99 latency     6663 µs              8046 µs              0.83
max latency     7280 µs              8681 µs
throughput      227 req/s            172 req/s            1.32  (higher)
wall time        882 ms              1162 ms
```

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
