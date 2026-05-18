package com.oresoftware.dd.akkaws.comparison;

import com.fasterxml.jackson.databind.JsonNode;
import com.oresoftware.dd.akkaws.pipeline.PipelineStages;
import org.ores.async.AsyncFut;
import org.ores.async.Asyncc;
import org.ores.async.NeoWaterfallI;

import java.util.ArrayList;
import java.util.List;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.CompletionStage;
import java.util.concurrent.Executor;

/**
 * Documentation-grade side-by-side of the F# Rx <em>rxFiveStages</em> pipeline rewritten in
 * async.java, in two flavors: callback-style with {@link Asyncc#Waterfall} and promise-style
 * with {@link AsyncFut#ParallelF}.
 *
 * <p>This file is a <strong>reference</strong> — it is not what
 * {@link com.oresoftware.dd.akkaws.pipeline.AsyncJavaPipeline} uses on the hot WS path. The
 * production class deliberately keeps parse/validate inline (one fewer Waterfall scaffold) for
 * the lowest possible per-message overhead in the benchmark comparison. This class instead
 * shows what async.java code looks like when you want a line-for-line mirror of an Rx chain —
 * one continuation per Rx {@code .Select} call, one {@code Parallel}/{@code ParallelF} per
 * {@code Observable.Zip}, one terminal error funnel per {@code .Catch}.
 *
 * <h2>The F# original</h2>
 *
 * <p>Lives in {@code remote/fsharp-ws-server/RxAdvanced.fs} as {@code rxFiveStages}. The
 * relevant body, condensed:
 *
 * <pre>
 * let private rxFiveStages (inbound: IObservable&lt;string&gt;) : IObservable&lt;string&gt; =
 *   inbound.SelectMany(fun input -&gt;
 *     let body =
 *       Observable
 *         .Return(input)
 *         .Select(fun s -&gt; parse s)
 *         .Select(fun n -&gt; validate n)
 *         .SelectMany(fun validated -&gt;
 *           let a = Observable.Start((fun () -&gt; enrichLookupA validated), TaskPoolScheduler.Default)
 *           let b = Observable.Start((fun () -&gt; enrichLookupB validated), TaskPoolScheduler.Default)
 *           Observable.Zip(a, b, fun lookupA lookupB -&gt;
 *             struct (validated, lookupA, lookupB)))
 *         .Select(fun (struct (v, a, b)) -&gt; score v a b)
 *         .Select(fun scored -&gt; serialize scored)
 *         .Select(fun out -&gt; sprintf "{\"ok\":true,\"result\":%s}" out)
 *     body.Catch(fun (ex: exn) -&gt; Observable.Return(perMessageErrorFrame ex)))
 * </pre>
 *
 * <h2>Side-by-side mapping</h2>
 *
 * <table border="1" summary="Rx to async.java operator map">
 *   <tr><th>Rx (F#)</th><th>async.java callback (Waterfall)</th><th>async.java promise (AsyncFut)</th></tr>
 *   <tr>
 *     <td>{@code inbound.SelectMany(fun input -> ...)}</td>
 *     <td>one {@link #runWaterfall} call per inbound frame</td>
 *     <td>one {@link #runWithAsyncFut} call per inbound frame</td>
 *   </tr>
 *   <tr>
 *     <td>{@code Observable.Return(input).Select(f)}</td>
 *     <td>Waterfall stage publishing {@code c.success(k, f(x))}</td>
 *     <td>{@code .thenApply(f)}</td>
 *   </tr>
 *   <tr>
 *     <td>{@code Observable.Start(fn, TaskPool)}</td>
 *     <td>{@code exec.execute(() -> try inner.success(fn()) catch inner.fail(t))}</td>
 *     <td>{@code CompletableFuture.supplyAsync(() -> fn(), exec)}</td>
 *   </tr>
 *   <tr>
 *     <td>{@code Observable.Zip(a, b, combiner)}</td>
 *     <td>{@code Asyncc.Parallel(List.of(a, b), (err, results) -> combiner(results.get(0), results.get(1)))}</td>
 *     <td>{@code AsyncFut.ParallelF(List.of(a, b)).thenApply(combiner)}</td>
 *   </tr>
 *   <tr>
 *     <td>{@code body.Catch(fun ex -> errorFrame ex)}</td>
 *     <td>Waterfall's terminal {@code if (err != null) emitErrorFrame(err)} branch</td>
 *     <td>{@code .exceptionally(errorFrame)}</td>
 *   </tr>
 * </table>
 *
 * <h2>When async.java earns its keep over plain {@link CompletableFuture}</h2>
 *
 * <p>For <em>this exact pipeline</em> (linear flow, 2-way fan-out, one error sink), plain
 * {@link CompletableFuture#thenApply}/{@link CompletableFuture#thenCombine} is arguably the
 * most idiomatic Java. The F# Rx version is doing the same thing with extra ceremony.
 *
 * <p>async.java's value-add shows when one or more of these enters the picture:
 *
 * <ol>
 *   <li><strong>N parallel tasks, not 2.</strong> {@code Asyncc.Parallel(List.of(t1, ..., tN))}
 *       or {@code ParallelLimit(k, List.of(...))} is a one-liner for arbitrary fan-out width.
 *       With {@code thenCombine} you'd be writing a {@code combine}-cascade.</li>
 *   <li><strong>Bounded concurrency.</strong> {@code ParallelLimit(4, tasks, ...)} caps in-flight
 *       work. Rx has {@code Merge(maxConcurrent: 4)}; plain {@code CompletableFuture} has no
 *       equivalent without writing your own semaphore.</li>
 *   <li><strong>Backpressure.</strong> {@code NeoQueue} (bounded async work queue with
 *       saturated / unsaturated / drain hooks) is the equivalent of Rx's Buffer / Throttle /
 *       Window for "accept submissions but cap concurrent execution".</li>
 *   <li><strong>Shared-state coordination across pipelines.</strong> {@code NeoLock} and v0.2.6
 *       {@code NeoRwLock} for cross-frame mutual exclusion. Rx has no native equivalent.</li>
 *   <li><strong>Composability of more exotic flow.</strong> {@code Whilst}, {@code FilterMap},
 *       {@code GroupBy}, {@code Reduce}, {@code Race}, {@code Inject} — Rx has these too, but
 *       async.java's callback contract is uniform across all of them, so they nest without
 *       conversion adapters.</li>
 * </ol>
 *
 * <p>Both methods below produce byte-identical happy-path output and structurally identical
 * error frames; this is asserted by
 * {@code RxFiveStagesComparisonTest}.
 */
public final class RxFiveStagesComparison {

  private RxFiveStagesComparison() {
  }

  // ===========================================================================================
  // Version 1: callback-style with Asyncc.Waterfall + nested Asyncc.Parallel
  // ===========================================================================================

  /**
   * Closest line-for-line equivalent of the F# Rx chain: every {@code .Select} becomes a
   * Waterfall stage, the {@code Observable.Zip(a, b, combiner)} becomes an
   * {@code Asyncc.Parallel(List.of(a, b), ...)}, and the {@code .Catch} becomes the Waterfall's
   * terminal {@code err} branch.
   *
   * <p>Each stage closes over {@code c}, the Waterfall continuation. {@code c.get("key")} reads
   * outputs published by earlier stages; {@code c.success("key", v)} publishes this stage's
   * output for downstream stages.
   *
   * @param inputFrame the WebSocket text frame as received
   * @param exec       the executor on which the two enrichment lookups should run (TaskPool
   *                   equivalent — anything with {@code execute(Runnable)} works)
   * @return a {@code CompletableFuture} that always completes successfully (never
   *         exceptionally) — the per-message error path lands in the success channel as a
   *         JSON error frame, mirroring Rx's {@code .Catch(fun ex -> errorFrame ex)} semantics.
   */
  public static CompletableFuture<String> runWaterfall(final String inputFrame, final Executor exec) {

    final CompletableFuture<String> outcome = new CompletableFuture<>();

    final List<NeoWaterfallI.AsyncTask<Object, Throwable>> stages = new ArrayList<>();

    // .Select(fun s -> parse s)
    stages.add(c -> {
      try {
        c.success("parsed", PipelineStages.parse(inputFrame));
      } catch (Throwable t) {
        c.fail(t);
      }
    });

    // .Select(fun n -> validate n)
    stages.add(c -> {
      try {
        c.success("validated", PipelineStages.validate((JsonNode) c.get("parsed")));
      } catch (Throwable t) {
        c.fail(t);
      }
    });

    // .SelectMany(fun validated -> Zip(Start(enrichA), Start(enrichB)))
    stages.add(c -> {
      final JsonNode validated = (JsonNode) c.get("validated");
      // List<Asyncc.Task<String>> flows directly into Parallel thanks to v0.2.8-rc2's
      // `List<? extends AsyncTask<T, E>>` widening. No cast, no defensive copy.
      final List<Asyncc.Task<String>> lookups = List.of(
          inner -> exec.execute(() -> {
            try {
              inner.success(PipelineStages.enrichLookupA(validated));
            } catch (Throwable t) {
              inner.fail(t);
            }
          }),
          inner -> exec.execute(() -> {
            try {
              inner.success(PipelineStages.enrichLookupB(validated));
            } catch (Throwable t) {
              inner.fail(t);
            }
          }));

      Asyncc.<String, Throwable>Parallel(lookups, (err, results) -> {
        if (err != null) {
          c.fail(err);
          return;
        }
        c.success("lookups", new ArrayList<>(results));
      });
    });

    // .Select(fun (validated, lookupA, lookupB) -> score validated lookupA lookupB)
    stages.add(c -> {
      try {
        final JsonNode validated = (JsonNode) c.get("validated");
        @SuppressWarnings("unchecked")
        final List<String> lookups = (List<String>) c.get("lookups");
        c.success("scored", PipelineStages.score(validated, lookups.get(0), lookups.get(1)));
      } catch (Throwable t) {
        c.fail(t);
      }
    });

    // .Select(fun scored -> serialize scored)
    stages.add(c -> {
      try {
        c.success("serialized", PipelineStages.serialize((JsonNode) c.get("scored")));
      } catch (Throwable t) {
        c.fail(t);
      }
    });

    // .Select(fun out -> sprintf "{\"ok\":true,\"result\":%s}" out)
    stages.add(c -> {
      c.success("envelope", "{\"ok\":true,\"result\":" + c.get("serialized") + "}");
    });

    // The terminal callback IS the Rx .Catch — funnel any stage failure into the same
    // perMessageErrorFrame as a successful-completion value, never propagate.
    Asyncc.Waterfall(stages, (err, all) -> {
      if (err != null) {
        outcome.complete(perMessageErrorFrame(err));
        return;
      }
      outcome.complete((String) all.get("envelope"));
    });

    return outcome;
  }

  // ===========================================================================================
  // Version 2: promise-style with AsyncFut.ParallelF
  // ===========================================================================================

  /**
   * Promise-style mirror of the same pipeline. {@link AsyncFut#ParallelF} (added in v0.2.8-rc3)
   * accepts already-started {@link CompletionStage}s, so the
   * {@code Observable.Start(fn, TaskPool)} -> {@code Observable.Zip(a, b)} pair maps to two
   * {@link CompletableFuture#supplyAsync(java.util.function.Supplier, Executor)} calls handed
   * straight to {@code ParallelF}.
   *
   * <p>Each Rx {@code .Select} maps to a single {@code .thenApply}; the
   * {@code .Catch(perMessageErrorFrame)} maps to {@code .exceptionally(...)}.
   */
  public static CompletableFuture<String> runWithAsyncFut(final String inputFrame, final Executor exec) {

    return CompletableFuture
        .completedFuture(inputFrame)
        // .Select(fun s -> parse s)
        .thenApply(s -> {
          try {
            return PipelineStages.parse(s);
          } catch (Exception e) {
            throw new RuntimeException(e);
          }
        })
        // .Select(fun n -> validate n)
        .thenApply(PipelineStages::validate)
        // .SelectMany(fun validated -> Zip(Start(enrichA), Start(enrichB)))
        .thenCompose(validated ->
            AsyncFut.<String>ParallelF(List.<CompletionStage<String>>of(
                CompletableFuture.supplyAsync(() -> PipelineStages.enrichLookupA(validated), exec),
                CompletableFuture.supplyAsync(() -> PipelineStages.enrichLookupB(validated), exec)
            )).thenApply(lookups ->
                new Object[] { validated, lookups.get(0), lookups.get(1) }))
        // .Select(fun (v, a, b) -> score v a b)
        .thenApply(t ->
            PipelineStages.score((JsonNode) t[0], (String) t[1], (String) t[2]))
        // .Select(fun scored -> serialize scored)
        .thenApply(scored -> {
          try {
            return PipelineStages.serialize(scored);
          } catch (Exception e) {
            throw new RuntimeException(e);
          }
        })
        // .Select(fun out -> sprintf "{\"ok\":true,\"result\":%s}" out)
        .thenApply(serialized -> "{\"ok\":true,\"result\":" + serialized + "}")
        // body.Catch(fun ex -> Observable.Return(perMessageErrorFrame ex))
        .exceptionally(RxFiveStagesComparison::perMessageErrorFrame);
  }

  // ===========================================================================================
  // Shared error frame — matches the F# RxAdvanced.perMessageErrorFrame format.
  // ===========================================================================================

  /**
   * Mirrors {@code RxAdvanced.fs}'s {@code perMessageErrorFrame}. Unwraps
   * {@code RuntimeException}-wrappers and {@link CompletableFuture#exceptionally}'s
   * {@code CompletionException} so the JSON exposes the original exception type and message.
   */
  static String perMessageErrorFrame(final Throwable ex) {
    Throwable cause = ex;
    // Unwrap CompletionException / RuntimeException-wrappers introduced by the promise chain
    // so the JSON shows the original exception type, matching the F# semantics.
    while ((cause instanceof java.util.concurrent.CompletionException
        || cause.getClass() == RuntimeException.class)
        && cause.getCause() != null && cause.getCause() != cause) {
      cause = cause.getCause();
    }
    final String type = cause.getClass().getSimpleName();
    final String msg = cause.getMessage() == null ? "" : cause.getMessage();
    return "{\"ok\":false,\"error\":\"" + type + ": " + escape(msg) + "\"}";
  }

  private static String escape(final String s) {
    return s.replace("\\", "\\\\").replace("\"", "\\\"");
  }
}
