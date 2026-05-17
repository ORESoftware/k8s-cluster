package com.oresoftware.dd.akkaws.pipeline;

import com.fasterxml.jackson.databind.JsonNode;
import org.ores.async.Asyncc;
import org.slf4j.Logger;
import org.slf4j.LoggerFactory;

import java.util.List;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.ExecutorService;

/**
 * Pipeline orchestration using async.java's combinators.
 *
 * <p>The shape:
 *
 * <pre>
 *   parse        (sync, on caller thread)
 *     |
 *     v
 *   validate     (sync, on caller thread)
 *     |
 *     v
 *   ParallelLookup A and B  (Asyncc.Parallel, fan-out on the supplied executor)
 *     |
 *     v
 *   score        (sync, on whatever thread finished the Parallel)
 *     |
 *     v
 *   serialize    (sync, on the same thread)
 * </pre>
 *
 * <p>The whole thing exposes a {@link CompletableFuture} so the HTTP layer can hand the result
 * back to Akka HTTP / the WS sink without caring about the orchestrator's threading model.
 *
 * <p>Why not chain plain {@code CompletableFuture}s? Because the point of this module is to
 * compare async.java against Akka Streams; using a third library to glue the test together
 * would muddy the result. The {@code CompletableFuture} at the boundary is purely for shipping
 * the final value out, not for orchestrating internal stages.
 */
public final class AsyncJavaPipeline {

  private static final Logger log = LoggerFactory.getLogger(AsyncJavaPipeline.class);

  private final ExecutorService executor;

  public AsyncJavaPipeline(final ExecutorService executor) {
    this.executor = executor;
  }

  public CompletableFuture<String> process(final String inputFrame) {

    final CompletableFuture<String> result = new CompletableFuture<>();

    try {
      final JsonNode parsed = PipelineStages.parse(inputFrame);
      final JsonNode validated = PipelineStages.validate(parsed);

      // Fan out the two enrichment lookups in parallel on the supplied executor. We use
      // Asyncc.Parallel rather than ParallelLimit because there are only two tasks; for larger
      // fan-outs ParallelLimit / NeoQueue would put a cap on in-flight work.
      final List<Asyncc.AsyncTask<String, Throwable>> lookups = List.of(
          cb -> executor.submit(() -> {
            try {
              cb.done(null, PipelineStages.enrichLookupA(validated));
            } catch (Throwable t) {
              cb.done(t, null);
            }
          }),
          cb -> executor.submit(() -> {
            try {
              cb.done(null, PipelineStages.enrichLookupB(validated));
            } catch (Throwable t) {
              cb.done(t, null);
            }
          }));

      Asyncc.Parallel(lookups, (err, lookupResults) -> {
        if (err != null) {
          result.completeExceptionally(unwrap(err));
          return;
        }
        try {
          final JsonNode scored = PipelineStages.score(validated, lookupResults.get(0), lookupResults.get(1));
          result.complete(PipelineStages.serialize(scored));
        } catch (Throwable t) {
          result.completeExceptionally(t);
        }
      });

    } catch (Throwable t) {
      // parse / validate / pre-Parallel throw on the calling thread. Funnel them through the
      // same future so the caller has one place to attach `.whenComplete`.
      result.completeExceptionally(t);
    }

    return result;
  }

  private static Throwable unwrap(final Object err) {
    if (err instanceof Throwable t) {
      return t;
    }
    return new RuntimeException("async.java pipeline failure: " + err);
  }
}
