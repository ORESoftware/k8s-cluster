# Rx.NET vs async.java — line-for-line translation of `rxFiveStages`

This document is a side-by-side of the `rxFiveStages` Rx.NET pipeline in
[`RxAdvanced.fs`](../RxAdvanced.fs) translated into two flavors of [async.java](https://async-java.github.io):
**callback-style** with `Asyncc.Waterfall` + nested `Asyncc.Parallel`, and **promise-style**
with `AsyncFut.ParallelF` returning a `CompletableFuture`.

The runnable Java reference lives in the sibling akka-ws-server module:
[`RxFiveStagesComparison.java`](../../akka-ws-server/src/main/java/com/oresoftware/dd/akkaws/comparison/RxFiveStagesComparison.java).
It uses the same `PipelineStages` (parse / validate / enrichLookupA / enrichLookupB / score /
serialize) that the production benchmark pipelines use, so any structural difference is
attributable to the orchestration library, not to the per-stage work.

A live web version (with rendered Javadoc links and the full mapping table) is published at
<https://async-java.github.io/rx-comparison/>.

## The F# original

From `RxAdvanced.fs` (lines 149-186):

```fsharp
let private rxFiveStages (inbound: IObservable<string>) : IObservable<string> =
    inbound.SelectMany(fun input ->
        let body : IObservable<string> =
            Observable
                .Return(input)
                .Select(fun s -> parse s)
                .Select(fun n -> validate n)
                .SelectMany(fun validated ->
                    let a =
                        Observable.Start(
                            (fun () -> enrichLookupA validated),
                            TaskPoolScheduler.Default)
                    let b =
                        Observable.Start(
                            (fun () -> enrichLookupB validated),
                            TaskPoolScheduler.Default)
                    Observable.Zip(
                        a, b,
                        fun lookupA lookupB ->
                            struct (validated, lookupA, lookupB)))
                .Select(fun (struct (validated, lookupA, lookupB)) ->
                    score validated lookupA lookupB)
                .Select(fun scored -> serialize scored)
                .Select(fun out -> sprintf "{\"ok\":true,\"result\":%s}" out)
        body.Catch(fun (ex: exn) ->
            Observable.Return(perMessageErrorFrame ex)))
```

Six `.Select` calls, one `.SelectMany` for the fan-out, one `.Catch` for the error funnel,
and the inner subgraph is re-materialised per inbound emission via the outer `.SelectMany`.

## Version 1 — callback-style with `Asyncc.Waterfall`

Closest line-for-line equivalent. Each `.Select` becomes a Waterfall stage. The
`Observable.Zip(Observable.Start(...), Observable.Start(...))` pair becomes an
`Asyncc.Parallel(List.of(a, b), ...)`. The `.Catch` becomes the Waterfall's terminal `err`
branch.

```java
public static CompletableFuture<String> runWaterfall(final String inputFrame, final Executor exec) {
    final CompletableFuture<String> outcome = new CompletableFuture<>();
    final List<NeoWaterfallI.AsyncTask<Object, Throwable>> stages = new ArrayList<>();

    // .Select(fun s -> parse s)
    stages.add(c -> {
        try { c.success("parsed", PipelineStages.parse(inputFrame)); }
        catch (Throwable t) { c.fail(t); }
    });

    // .Select(fun n -> validate n)
    stages.add(c -> {
        try { c.success("validated", PipelineStages.validate((JsonNode) c.get("parsed"))); }
        catch (Throwable t) { c.fail(t); }
    });

    // .SelectMany(fun validated -> Zip(Start(enrichA), Start(enrichB)))
    stages.add(c -> {
        final JsonNode validated = (JsonNode) c.get("validated");
        // List<Asyncc.Task<String>> flows directly into Parallel thanks to v0.2.8-rc2's
        // `List<? extends AsyncTask<T, E>>` widening — no cast, no defensive copy.
        final List<Asyncc.Task<String>> lookups = List.of(
            inner -> exec.execute(() -> {
                try { inner.success(PipelineStages.enrichLookupA(validated)); }
                catch (Throwable t) { inner.fail(t); }
            }),
            inner -> exec.execute(() -> {
                try { inner.success(PipelineStages.enrichLookupB(validated)); }
                catch (Throwable t) { inner.fail(t); }
            }));
        Asyncc.<String, Throwable>Parallel(lookups, (err, results) -> {
            if (err != null) { c.fail(err); return; }
            c.success("lookups", new ArrayList<>(results));
        });
    });

    // .Select(fun (v, a, b) -> score v a b)
    stages.add(c -> {
        try {
            final JsonNode validated = (JsonNode) c.get("validated");
            @SuppressWarnings("unchecked")
            final List<String> lookups = (List<String>) c.get("lookups");
            c.success("scored", PipelineStages.score(validated, lookups.get(0), lookups.get(1)));
        } catch (Throwable t) { c.fail(t); }
    });

    // .Select(fun scored -> serialize scored)
    stages.add(c -> {
        try { c.success("serialized", PipelineStages.serialize((JsonNode) c.get("scored"))); }
        catch (Throwable t) { c.fail(t); }
    });

    // .Select(fun out -> sprintf "{\"ok\":true,\"result\":%s}" out)
    stages.add(c ->
        c.success("envelope", "{\"ok\":true,\"result\":" + c.get("serialized") + "}"));

    // body.Catch(fun ex -> Observable.Return(perMessageErrorFrame ex))
    Asyncc.Waterfall(stages, (err, all) -> {
        if (err != null) { outcome.complete(perMessageErrorFrame(err)); return; }
        outcome.complete((String) all.get("envelope"));
    });
    return outcome;
}
```

## Version 2 — promise-style with `AsyncFut.ParallelF`

`AsyncFut.ParallelF` (v0.2.8-rc3+) takes already-started `CompletionStage`s, so
`Observable.Start(fn, TaskPool)` maps directly to `CompletableFuture.supplyAsync(() -> fn(), exec)`
and `Observable.Zip(a, b)` maps to `AsyncFut.ParallelF(List.of(a, b))`.

```java
public static CompletableFuture<String> runWithAsyncFut(final String inputFrame, final Executor exec) {
    return CompletableFuture
        .completedFuture(inputFrame)
        .thenApply(s -> {                                                 // .Select(parse)
            try { return PipelineStages.parse(s); }
            catch (Exception e) { throw new RuntimeException(e); }
        })
        .thenApply(PipelineStages::validate)                              // .Select(validate)
        .thenCompose(validated ->                                         // .SelectMany + .Zip
            AsyncFut.<String>ParallelF(List.<CompletionStage<String>>of(
                CompletableFuture.supplyAsync(() -> PipelineStages.enrichLookupA(validated), exec),
                CompletableFuture.supplyAsync(() -> PipelineStages.enrichLookupB(validated), exec)
            )).thenApply(lookups -> new Object[] { validated, lookups.get(0), lookups.get(1) }))
        .thenApply(t ->                                                   // .Select(score)
            PipelineStages.score((JsonNode) t[0], (String) t[1], (String) t[2]))
        .thenApply(scored -> {                                            // .Select(serialize)
            try { return PipelineStages.serialize(scored); }
            catch (Exception e) { throw new RuntimeException(e); }
        })
        .thenApply(s -> "{\"ok\":true,\"result\":" + s + "}")              // .Select(envelope)
        .exceptionally(RxFiveStagesComparison::perMessageErrorFrame);     // .Catch
}
```

## Operator mapping table

The same five-stage pipeline expressed in four orchestrators: F# Rx, async.java callbacks,
async.java promises, and Akka Streams. The Akka Streams column tracks the production
[`AkkaStreamsPipeline.java`](../../akka-ws-server/src/main/java/com/oresoftware/dd/akkaws/pipeline/AkkaStreamsPipeline.java)
that lives next to the async.java reference in `akka-ws-server`.

| Rx (F#)                                | async.java callback (Waterfall)                                                                | async.java promise (AsyncFut)                            | Akka Streams                                                                                                |
| -------------------------------------- | ---------------------------------------------------------------------------------------------- | -------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------- |
| `inbound.SelectMany(fun input -> ...)` | one `runWaterfall` call per inbound frame                                                      | one `runWithAsyncFut` call per inbound frame             | `Source.single(input).via(flow).runWith(Sink.head(), system)` (per-message materialisation)                 |
| `Observable.Return(input).Select(f)`   | Waterfall stage publishing `c.success(k, f(x))`                                                | `.thenApply(f)`                                          | `Flow.<T>create().map(f)`                                                                                   |
| `Observable.Start(fn, TaskPool)`       | `exec.execute(() -> try inner.success(fn()) catch inner.fail(t))`                              | `CompletableFuture.supplyAsync(() -> fn(), exec)`        | `CompletableFuture.supplyAsync(() -> fn(), system.executionContext())` inside a `mapAsync` stage             |
| `Observable.Zip(a, b, combiner)`       | `Asyncc.Parallel(List.of(a, b), (err, results) -> combiner(results.get(0), results.get(1)))`   | `AsyncFut.ParallelF(List.of(a, b)).thenApply(combiner)`  | `.mapAsync(2, x -> aFut.thenCombine(bFut, combiner))` — or a `Broadcast` + `Zip` subgraph                    |
| `body.Catch(fun ex -> errorFrame ex)`  | Waterfall's terminal `if (err != null) emitErrorFrame(err)` branch                             | `.exceptionally(errorFrame)`                             | `.recover(ex -> errorFrame(ex))` Flow stage (or `Supervision.resumingDecider` on the materialiser)            |
| *Backpressure*                         | not native — pair with `NeoQueue` for submission-side cap                                      | not native — same: `NeoQueue` or `Semaphore`-style       | **structural** — demand signal propagates upstream from the sink; `buffer(n, overflowStrategy)` per stage    |
| *Per-pipeline overhead*                | ~50 µs (heap allocs + atomic counter increments + lambda dispatch)                             | ~50 µs + one `CompletableFuture` per stage               | ~200–400 µs per *materialisation* — see the [load-curve writeup](https://async-java.github.io/blog/2026/05/17/async-java-vs-akka-streams/) |

## When async.java earns its keep

For this **exact** pipeline (linear, 2-way fan-out, single error sink), the most idiomatic
Java answer is probably plain `CompletableFuture` with `.thenCombine(...)` instead of
`AsyncFut.ParallelF` — the F# Rx version is doing the same thing with extra ceremony, and
2-way fan-out doesn't need a List-based combinator. async.java's value-add appears when one
or more of these enters the picture:

1. **N parallel tasks, not 2.** `Asyncc.Parallel(List.of(t1, ..., tN))` or
   `ParallelLimit(k, List.of(...))` is a one-liner for arbitrary fan-out width.
2. **Bounded concurrency.** `ParallelLimit(4, tasks, ...)` caps in-flight work. Rx has
   `Merge(maxConcurrent: 4)`; plain `CompletableFuture` has no equivalent without writing
   your own semaphore.
3. **Backpressure.** `NeoQueue` (bounded async work queue with saturated / unsaturated /
   drain hooks) is the equivalent of Rx's Buffer / Throttle / Window for "accept submissions
   but cap concurrent execution".
4. **Shared-state coordination across pipelines.** `NeoLock` and v0.2.6 `NeoRwLock` for
   cross-frame mutual exclusion. Rx has no native equivalent.
5. **Composability of more exotic flow.** `Whilst`, `FilterMap`, `GroupBy`, `Reduce`, `Race`,
   `Inject` — Rx has these too, but async.java's callback contract is uniform across all of
   them, so they nest without conversion adapters.

## Tests

The runnable Java reference is pinned by [`RxFiveStagesComparisonTest.java`](../../akka-ws-server/src/test/java/com/oresoftware/dd/akkaws/comparison/RxFiveStagesComparisonTest.java):

* **happy path**: Waterfall and AsyncFut versions produce byte-identical output for the same input.
* **parse failure** (`"not json"`): both emit `{"ok":false,"error":"JsonParseException: ..."}` on the success channel of the returned future.
* **validation failure** (missing `id`): both emit an `IllegalArgumentException`-typed error frame.
* **score failure** (id=`"poison"`): both emit an `IllegalStateException`-typed error frame, proving the error funnel works downstream of the fan-out.

Run them with `mvn -pl remote/deployments/akka-ws-server test` from the repo root.
