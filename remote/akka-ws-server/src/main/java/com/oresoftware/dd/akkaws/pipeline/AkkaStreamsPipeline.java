package com.oresoftware.dd.akkaws.pipeline;

import akka.NotUsed;
import akka.actor.typed.ActorSystem;
import akka.stream.javadsl.Flow;
import akka.stream.javadsl.Sink;
import akka.stream.javadsl.Source;
import com.fasterxml.jackson.databind.JsonNode;
import org.slf4j.Logger;
import org.slf4j.LoggerFactory;

import java.util.concurrent.CompletableFuture;
import java.util.concurrent.CompletionStage;

/**
 * Pipeline orchestration using Akka Streams.
 *
 * <p>The shape, expressed as a Source -&gt; Flow -&gt; Sink graph:
 *
 * <pre>
 *   Source.single(inputFrame)
 *     .via(parseFlow)
 *     .via(validateFlow)
 *     .via(enrichFlow)              // mapAsync(2) — runs both lookups concurrently
 *     .via(scoreFlow)
 *     .via(serializeFlow)
 *     .runWith(Sink.head(), system)
 * </pre>
 *
 * <p>{@code mapAsync(parallelism=2)} is the Akka Streams equivalent of running A and B in
 * parallel: the stage starts at most {@code parallelism} downstream-bound asynchronous calls
 * concurrently and emits their results in input order. Errors propagate as stream failures —
 * the materialized {@link CompletionStage} completes exceptionally and the supervision strategy
 * (default: stop) tears the rest of the graph down.
 *
 * <p>One subtle but real difference from the async.java implementation: Akka Streams' back-pressure
 * is structural — a downstream sink that can't keep up will pull less, and upstream stages
 * cooperatively slow down. In a pure callback library that property must be implemented
 * separately (which is exactly what async.java's {@code NeoQueue} provides for fan-out, but
 * not for stream-shaped pipelines).
 */
public final class AkkaStreamsPipeline {

  private static final Logger log = LoggerFactory.getLogger(AkkaStreamsPipeline.class);

  private final ActorSystem<?> system;

  public AkkaStreamsPipeline(final ActorSystem<?> system) {
    this.system = system;
  }

  public CompletionStage<String> process(final String inputFrame) {

    final Flow<String, JsonNode, NotUsed> parseFlow = Flow.<String>create()
        .map(PipelineStages::parse);

    final Flow<JsonNode, JsonNode, NotUsed> validateFlow = Flow.<JsonNode>create()
        .map(PipelineStages::validate);

    // mapAsync(2) — start both lookups concurrently and emit a merged tuple-like record.
    // Using a small helper record keeps both intermediate results carried together; an
    // alternative is `.zip` of two parallel sub-flows, but the record is cheaper and clearer.
    final Flow<JsonNode, EnrichedRecord, NotUsed> enrichFlow = Flow.<JsonNode>create()
        .mapAsync(2, validated -> {
          final CompletableFuture<EnrichedRecord> done = new CompletableFuture<>();
          // Two CompletableFutures, completed off the system's default dispatcher.
          final CompletableFuture<String> a = CompletableFuture.supplyAsync(
              () -> PipelineStages.enrichLookupA(validated),
              system.executionContext());
          final CompletableFuture<String> b = CompletableFuture.supplyAsync(
              () -> PipelineStages.enrichLookupB(validated),
              system.executionContext());
          a.thenCombine(b, (lookupA, lookupB) -> new EnrichedRecord(validated, lookupA, lookupB))
              .whenComplete((rec, err) -> {
                if (err != null) {
                  done.completeExceptionally(err);
                } else {
                  done.complete(rec);
                }
              });
          return done;
        });

    final Flow<EnrichedRecord, JsonNode, NotUsed> scoreFlow = Flow.<EnrichedRecord>create()
        .map(rec -> PipelineStages.score(rec.validated, rec.lookupA, rec.lookupB));

    final Flow<JsonNode, String, NotUsed> serializeFlow = Flow.<JsonNode>create()
        .map(PipelineStages::serialize);

    return Source.single(inputFrame)
        .via(parseFlow)
        .via(validateFlow)
        .via(enrichFlow)
        .via(scoreFlow)
        .via(serializeFlow)
        .runWith(Sink.head(), system);
  }

  /** Tiny tuple to carry the validated payload + both enrichment results into the score stage. */
  private record EnrichedRecord(JsonNode validated, String lookupA, String lookupB) {
  }
}
