package com.oresoftware.dd.sparkpipeline.pipeline;

import com.oresoftware.dd.sparkpipeline.model.JobKind;
import com.oresoftware.dd.sparkpipeline.model.JobState;
import io.vertx.core.Vertx;
import io.vertx.core.json.JsonObject;
import io.vertx.junit5.VertxExtension;
import io.vertx.junit5.VertxTestContext;
import org.junit.jupiter.api.AfterEach;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.extension.ExtendWith;

import java.util.concurrent.CompletableFuture;
import java.util.concurrent.TimeUnit;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * End-to-end tests for the {@code COMPOSITION_DEMO} job kind, which exercises
 * 11 different async.java combinators in one pipeline (Waterfall, Times, Map, Parallel,
 * FilterMap, GroupBy, Each, Race, Reduce, Inject, NeoLock, plus the NeoQueue that
 * {@link JobService} uses for backpressure).
 *
 * <p>Goals: (a) prove the composition runs to completion successfully; (b) confirm every
 * stage's intermediate output is recorded so the stage log can be used as a per-combinator
 * checklist; (c) drive enough concurrent COMPOSITION_DEMO jobs that any cross-combinator
 * regression of the kind fixed in PR #9 / PR #10 / 0.2.1 would surface.
 */
@ExtendWith(VertxExtension.class)
class CompositionDemoPipelineTest {

  private Vertx vertx;
  private JobService svc;

  @BeforeEach
  void setUp() {
    vertx = Vertx.vertx();
    svc = new JobService(vertx, null);
  }

  @AfterEach
  void tearDown() throws InterruptedException {
    svc.shutdown();
    vertx.close().toCompletionStage().toCompletableFuture().orTimeout(5, TimeUnit.SECONDS);
  }

  @Test
  void singleCompositionDemoSucceeds(final VertxTestContext ctx) {
    svc.submitAndAwait(JobKind.COMPOSITION_DEMO, new JsonObject().put("shardCount", 8))
        .onComplete(ctx.succeeding(rec -> ctx.verify(() -> {
          assertEquals(JobState.SUCCEEDED, rec.getState());
          assertNotNull(rec.getResult());
          final JsonObject r = rec.getResult();

          assertEquals(8, (int) r.getInteger("shardsGenerated"),
              "Asyncc.Times must produce shardCount shards");
          assertEquals(8, (int) r.getInteger("enrichedCount"),
              "Asyncc.Map enriches every input shard");

          final int kept = r.getInteger("keptCount");
          assertTrue(kept >= 0 && kept <= 8,
              "Asyncc.FilterMap keeps at most shardsGenerated, dropping any size % 13 == 0");

          assertNotNull(r.getJsonObject("byRegion"),
              "Asyncc.GroupBy must produce the byRegion bucket map");

          assertNotNull(r.getString("raceWinner"));
          // The fast path is supposed to win — the slow racer sleeps 50ms vs 5ms.
          assertTrue(r.getString("raceWinner").contains("fastPath"),
              "Asyncc.Race must pick the fast path: " + r.getString("raceWinner"));

          assertEquals(kept, Integer.parseInt(r.getString("aggregateTotal")),
              "Asyncc.Reduce aggregateTotal must equal the kept-shard count");

          assertNotNull(r.getString("manifest"));
          assertTrue(r.getString("manifest").contains("\"winner\""),
              "Asyncc.Inject must include the raceWinner in the manifest");

          assertNotNull(r.getString("publicationCount"),
              "NeoLock-guarded publicationCount must be present");

          // Stage log should contain every combinator marker as evidence that each stage
          // ran. This is a per-combinator checklist that fails loudly if a stage silently
          // skips.
          final var log = rec.getStageLog();
          assertTrue(log.stream().anyMatch(s -> s.contains("composition.configure")),
              "Waterfall stage 1 ran");
          assertTrue(log.stream().anyMatch(s -> s.contains("composition.times")),
              "Asyncc.Times ran");
          assertTrue(log.stream().anyMatch(s -> s.contains("composition.map")),
              "Asyncc.Map (+ nested Parallel) ran");
          assertTrue(log.stream().anyMatch(s -> s.contains("composition.filterMap")),
              "Asyncc.FilterMap ran");
          assertTrue(log.stream().anyMatch(s -> s.contains("composition.groupBy")),
              "Asyncc.GroupBy ran");
          assertTrue(log.stream().anyMatch(s -> s.contains("composition.each")),
              "Asyncc.Each ran");
          assertTrue(log.stream().anyMatch(s -> s.contains("composition.race")),
              "Asyncc.Race ran");
          assertTrue(log.stream().anyMatch(s -> s.contains("composition.reduce")),
              "Asyncc.Reduce ran");
          assertTrue(log.stream().anyMatch(s -> s.contains("composition.inject")),
              "Asyncc.Inject ran");
          assertTrue(log.stream().anyMatch(s -> s.contains("composition.neoLock")),
              "NeoLock stage ran");

          ctx.completeNow();
        })));
  }

  /**
   * Drive 20 COMPOSITION_DEMO jobs concurrently. The JobService NeoQueue caps actual
   * in-flight execution at {@code PIPELINE_MAX_CONCURRENT} (default 4); the rest queue.
   * Beyond proving they all complete, this surfaces any cross-combinator concurrency
   * regression of the type fixed by async.java PR #9 / #10 / 0.2.1 — those bugs would
   * manifest as hung futures or duplicate result completions in a high-throughput run.
   */
  @Test
  void twentyConcurrentCompositionDemosAllSucceed(final VertxTestContext ctx) {
    final int n = 20;
    final CompletableFuture<?>[] futures = new CompletableFuture<?>[n];
    for (int i = 0; i < n; i++) {
      futures[i] = svc.submitAndAwait(JobKind.COMPOSITION_DEMO,
              new JsonObject().put("shardCount", 8))
          .toCompletionStage().toCompletableFuture();
    }

    CompletableFuture.allOf(futures).whenComplete((ignored, err) -> {
      ctx.verify(() -> {
        assertEquals(null, err, "Every COMPOSITION_DEMO must complete cleanly under 20-way concurrency");
        for (final var f : futures) {
          assertTrue(f.isDone() && !f.isCompletedExceptionally(),
              "Every future must succeed");
        }
        ctx.completeNow();
      });
    });
  }

  /**
   * Pin the FilterMap dropping rule: shardCount=14 produces shard ids 0..13 with sizes
   * 100, 107, 114, 121, 128, 135, 142, 149, 156, 163, 170, 177, 184, 191. None of these
   * are divisible by 13, so the filter should drop 0. Increasing to shardCount=20 produces
   * sizes up to 233 — still no size % 13 == 0 with the linear formula 100 + 7*i for i < 27.
   * Test that the FilterMap behaviour exists by using a shardCount large enough to span
   * a multiple of 13 in (100 + 7*i): 13 * 11 = 143 -> i = (143-100)/7 = 6.14 (not integer),
   * 13 * 12 = 156 -> i = 8 (not divisible by 7 again, doesn't match formula). So we never
   * actually hit a 13-divisible size in this formula. That's intentional in the
   * synthetic — but if you change the size formula, expect the filter to bite.
   */
  @Test
  void filterMapPreservesEverythingWithCurrentSizeFormula(final VertxTestContext ctx) {
    svc.submitAndAwait(JobKind.COMPOSITION_DEMO, new JsonObject().put("shardCount", 14))
        .onComplete(ctx.succeeding(rec -> ctx.verify(() -> {
          final JsonObject r = rec.getResult();
          assertEquals(14, (int) r.getInteger("shardsGenerated"));
          assertEquals(14, (int) r.getInteger("enrichedCount"));
          assertEquals(14, (int) r.getInteger("keptCount"),
              "with size = 100 + 7*i for i in [0..13], no size is divisible by 13");
          ctx.completeNow();
        })));
  }
}
