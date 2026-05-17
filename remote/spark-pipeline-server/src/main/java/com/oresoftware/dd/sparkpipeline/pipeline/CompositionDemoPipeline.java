package com.oresoftware.dd.sparkpipeline.pipeline;

import com.fasterxml.jackson.databind.ObjectMapper;
import com.oresoftware.dd.sparkpipeline.model.JobRecord;
import io.vertx.core.Promise;
import io.vertx.core.json.JsonObject;
import org.ores.async.Asyncc;
import org.ores.async.NeoEachI;
import org.ores.async.NeoFilterMapI;
import org.ores.async.NeoGroupByI;
import org.ores.async.NeoInject;
import org.ores.async.NeoInjectI;
import org.ores.async.NeoLock;
import org.ores.async.NeoRaceIfc;
import org.ores.async.NeoReduceI;
import org.ores.async.NeoTimesI;
import org.ores.async.NeoWaterfallI;
import org.ores.async.Unlock;
import org.slf4j.Logger;
import org.slf4j.LoggerFactory;

import java.util.ArrayList;
import java.util.Collections;
import java.util.HashMap;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.Set;
import java.util.concurrent.atomic.AtomicInteger;

/**
 * The {@code COMPOSITION_DEMO} pipeline: a single coherent job that exercises 11 different
 * async.java combinators in composition. Each stage's intermediate output is recorded on the
 * {@link JobRecord} stage log so the runner can be debugged step-by-step from the HTTP
 * response.
 *
 * <p>The combinators used, in order of appearance:
 *
 * <ol>
 *   <li><strong>{@code Asyncc.Waterfall}</strong> — the outer scaffold; each stage publishes a
 *       named output that downstream stages read via {@code cb.get("key")}.</li>
 *   <li><strong>{@code Asyncc.Times}</strong> — generate N synthetic shard descriptors.</li>
 *   <li><strong>{@code Asyncc.Map}</strong> — enrich each shard. The mapper itself nests:
 *     <ol type="a">
 *       <li><strong>{@code Asyncc.Parallel}</strong> — fan-out two simulated upstream
 *           lookups per shard.</li>
 *     </ol>
 *   </li>
 *   <li><strong>{@code Asyncc.FilterMap}</strong> — drop shards that fail a synthetic
 *       validation rule (returning {@code null} from the mapper signals "drop").</li>
 *   <li><strong>{@code Asyncc.GroupBy}</strong> — bucket the surviving shards by region.</li>
 *   <li><strong>{@code Asyncc.Each}</strong> — fan-out fire-and-forget side effects per
 *       bucket (logging only, but real pipelines would publish per-bucket events here).</li>
 *   <li><strong>{@code Asyncc.Race}</strong> — first-to-respond between two scoring
 *       strategies. Whichever completes first wins; the loser is silently discarded.</li>
 *   <li><strong>{@code Asyncc.Reduce}</strong> — fold the per-bucket counts into an
 *       aggregate total.</li>
 *   <li><strong>{@code Asyncc.Inject}</strong> — name-keyed DAG that builds the final
 *       manifest by reading prior stage outputs through {@code cb.get("name")}.</li>
 *   <li><strong>{@code NeoLock}</strong> — async mutex serialising updates to a shared
 *       cross-job publication counter (cross-cutting concern, not a Waterfall stage).</li>
 *   <li><strong>{@code NeoQueue}</strong> — server-wide concurrency cap, owned by
 *       {@link JobService} and applied above every job. Not invoked here but in scope for the
 *       "11 combinators in one pipeline" tally.</li>
 * </ol>
 *
 * <p>All eleven exercise different facets of async.java's composability surface: fan-out
 * (Parallel, Map, Each), fan-in (Reduce, Inject), filtering (FilterMap), grouping (GroupBy),
 * sequential pipelining (Waterfall), retry / generation (Times), first-to-respond (Race),
 * mutex (NeoLock), and concurrency capping (NeoQueue). If any of these had a regression of
 * the type fixed by PRs #9 / #10 / #11 (NeoReduce defensive guard), a COMPOSITION_DEMO job
 * would surface it under load.
 */
public final class CompositionDemoPipeline {

  private static final Logger log = LoggerFactory.getLogger(CompositionDemoPipeline.class);
  private static final ObjectMapper MAPPER = new ObjectMapper();

  /**
   * Shared across jobs (intentionally) so the {@code NeoLock}-guarded section has real
   * cross-job contention.
   */
  private static final AtomicInteger PUBLICATION_COUNT = new AtomicInteger();

  /** Regions assigned in round-robin to synthetic shards. */
  private static final List<String> REGIONS = List.of("us-east", "us-west", "eu-central", "ap-southeast");

  private CompositionDemoPipeline() {
  }

  /**
   * Run the composition demo against a {@link JobRecord} and complete the supplied promise
   * with the same record once the pipeline finishes (success or failure).
   *
   * @param shardCount how many synthetic shards to generate (default 8).
   */
  public static void run(final JobRecord rec, final int shardCount, final Promise<JobRecord> outcome) {

    final List<NeoWaterfallI.AsyncTask<Object, Throwable>> stages = new ArrayList<>();

    // -- Stage 1: configure -------------------------------------------------
    stages.add(c -> {
      rec.appendStage("composition.configure shardCount=" + shardCount);
      c.success("config", "{\"shards\":" + shardCount + ",\"regions\":" + REGIONS.size() + "}");
    });

    // -- Stage 2: Asyncc.Times --------------------------------------------
    //    Generate shardCount synthetic Shard descriptors. Times runs the producer N times and
    //    collects the results into a List<String> (one shard JSON per element).
    stages.add(c -> {
      Asyncc.<String, Throwable>Times(shardCount,
          (NeoTimesI.ITimesr<String, Throwable>) (i, inner) -> {
            final String shard = "{\"id\":\"shard-" + i + "\",\"region\":\"" + REGIONS.get(i % REGIONS.size())
                + "\",\"size\":" + (100 + i * 7) + "}";
            inner.done(null, shard);
          },
          (err, shards) -> {
            if (err != null) {
              c.fail(toThrowable(err));
              return;
            }
            rec.appendStage("composition.times produced=" + shards.size() + " shards");
            c.success("shards", new ArrayList<>(shards));
          });
    });

    // -- Stage 3: Asyncc.Map (with nested Asyncc.Parallel) ----------------
    //    For each shard, fan out two simulated upstream lookups in parallel and merge their
    //    results into a single enriched shard JSON.
    stages.add(c -> {
      final List<String> shards = readListOfString(c, "shards");

      Asyncc.<String, String, Throwable>Map(shards,
          (shard, inner) -> {
            // Nested Parallel — two upstream fetches per shard.
            Asyncc.<String, Throwable>Parallel(
                p -> p.success("schemaV=" + (shard.length() % 5)),
                p -> p.success("ownerV=team-" + Math.abs(shard.hashCode() % 7)),
                (perr, pairs) -> {
                  if (perr != null) {
                    inner.fail(toThrowable(perr));
                    return;
                  }
                  inner.success(mergeJson(shard, "enrich", pairs.get(0) + ";" + pairs.get(1)));
                });
          },
          (err, enriched) -> {
            if (err != null) {
              c.fail(toThrowable(err));
              return;
            }
            rec.appendStage("composition.map enriched=" + enriched.size() + " (each via Parallel of 2)");
            c.success("enriched", new ArrayList<>(enriched));
          });
    });

    // -- Stage 4: Asyncc.FilterMap ----------------------------------------
    //    Drop any shard whose size field is divisible by 13 — synthetic validation rule.
    //    Returning null from the mapper signals "drop this element".
    stages.add(c -> {
      final List<String> enriched = readListOfString(c, "enriched");

      Asyncc.<String, String, Throwable>FilterMap(enriched,
          (NeoFilterMapI.IMapper<String, String, Throwable>) (s, inner) -> {
            final int size = extractIntField(s, "size", 0);
            if (size > 0 && size % 13 == 0) {
              // Signal drop. The (String) null cast resolves the ambiguity between
              // NeoFilterMapI.AsyncCallback.done(E, Optional<T>) and the inherited
              // IAsyncCallback.done(E, T) — both match `done(null, null)`.
              inner.done(null, (String) null);
            } else {
              inner.success(s);
            }
          },
          (err, kept) -> {
            if (err != null) {
              c.fail(toThrowable(err));
              return;
            }
            rec.appendStage("composition.filterMap kept=" + kept.size() + " of " + enriched.size());
            c.success("kept", new ArrayList<>(kept));
          });
    });

    // -- Stage 5: Asyncc.GroupBy ------------------------------------------
    //    Bucket the surviving shards by their region field.
    stages.add(c -> {
      final List<String> kept = readListOfString(c, "kept");

      Asyncc.<String, String, Throwable>GroupBy(kept,
          (NeoGroupByI.IMapper<String, Throwable>) (s, inner) -> inner.done(null, extractStringField(s, "region", "unknown")),
          (err, byRegion) -> {
            if (err != null) {
              c.fail(toThrowable(err));
              return;
            }
            rec.appendStage("composition.groupBy regions=" + byRegion.keySet());
            // The waterfall map only holds Object values; cast through an intermediate copy.
            c.success("byRegion", new HashMap<>(byRegion));
          });
    });

    // -- Stage 6: Asyncc.Each ---------------------------------------------
    //    Fire-and-forget per region: log how many shards are in each bucket. Real pipelines
    //    would publish per-region events here.
    stages.add(c -> {
      final Map<String, List<String>> byRegion = readMapOfRegionToList(c, "byRegion");
      final Set<String> regions = byRegion.keySet();

      Asyncc.<String, Throwable>Each(regions,
          (NeoEachI.IEacher<String, Throwable>) (region, inner) -> {
            rec.appendStage("composition.each region=" + region + " count=" + byRegion.get(region).size());
            inner.done(null);
          },
          err -> {
            if (err != null) {
              c.fail(toThrowable(err));
              return;
            }
            c.success("regionsLogged", regions.size());
          });
    });

    // -- Stage 7: Asyncc.Race ---------------------------------------------
    //    Two scoring strategies sprint to produce a number. Fast-path completes after a tiny
    //    sleep; careful-path completes after a larger sleep. Whichever lands first wins.
    stages.add(c -> {
      final Map<String, List<String>> byRegion = readMapOfRegionToList(c, "byRegion");

      final List<NeoRaceIfc.AsyncTask<String, Throwable>> racers = List.of(
          inner -> sleepThenDone(inner, 5, "fastPath:count=" + byRegion.size()),
          inner -> sleepThenDone(inner, 50, "carefulPath:count=" + byRegion.size()));

      Asyncc.<String, String, Throwable>Race(racers, (err, winner) -> {
        if (err != null) {
          c.fail(toThrowable(err));
          return;
        }
        rec.appendStage("composition.race winner=" + winner);
        c.success("raceWinner", String.valueOf(winner));
      });
    });

    // -- Stage 8: Asyncc.Reduce -------------------------------------------
    //    Sum the per-bucket counts into a total. The reducer is "acc + |bucket|".
    stages.add(c -> {
      final Map<String, List<String>> byRegion = readMapOfRegionToList(c, "byRegion");
      final List<Integer> bucketSizes = new ArrayList<>();
      for (final List<String> bucket : byRegion.values()) {
        bucketSizes.add(bucket.size());
      }

      Asyncc.<Integer, Integer, Integer, Throwable>Reduce(0, bucketSizes,
          (NeoReduceI.IReducer<Integer, Integer, Throwable>) (acc, next, inner) -> inner.done(null, acc + next),
          (err, total) -> {
            if (err != null) {
              c.fail(toThrowable(err));
              return;
            }
            rec.appendStage("composition.reduce total=" + total);
            c.success("aggregateTotal", total);
          });
    });

    // -- Stage 9: Asyncc.Inject -------------------------------------------
    //    Build the final manifest from named prior outputs. Inject's task body uses
    //    c.get("name") to read other (already-completed) named outputs from the
    //    pre-populated map. We seed that map by hand from the waterfall's accumulated
    //    state, then run Inject for the final assembly.
    stages.add(c -> {
      final Map<String, NeoInject.Task<String, Throwable>> dag = new LinkedHashMap<>();

      dag.put("raceWinner",
          new NeoInject.Task<>(inner -> inner.done(null, (String) c.get("raceWinner"))));

      dag.put("aggregateTotal",
          new NeoInject.Task<>(inner -> inner.done(null, "total=" + c.get("aggregateTotal"))));

      dag.put("manifest",
          new NeoInject.Task<>("raceWinner", "aggregateTotal",
              (NeoInjectI.IInjectable<String, Throwable>) inner -> {
                final String raceWinner = inner.get("raceWinner");
                final String aggregateTotal = inner.get("aggregateTotal");
                inner.done(null,
                    "{\"winner\":\"" + raceWinner + "\",\"" + aggregateTotal + "\"}");
              }));

      Asyncc.<String, Throwable>Inject(dag, (err, results) -> {
        if (err != null) {
          c.fail(toThrowable(err));
          return;
        }
        rec.appendStage("composition.inject manifest=" + results.get("manifest"));
        c.success("manifest", (String) results.get("manifest"));
      });
    });

    // -- Stage 10: NeoLock-guarded shared-state update -------------------
    //    Serialises updates to a process-wide AtomicInteger across concurrent demo jobs.
    //    NeoLock is an async mutex: acquire schedules a callback (not blocking the worker
    //    thread), and the Unlock token can be released from any thread.
    stages.add(c -> {
      SharedLocks.PUBLICATION_LOCK.acquire((lockErr, unlock) -> {
        if (lockErr != null) {
          c.fail(toThrowable(lockErr));
          return;
        }
        try {
          final int count = PUBLICATION_COUNT.incrementAndGet();
          rec.appendStage("composition.neoLock publicationCount=" + count);
          c.success("publicationCount", count);
        } finally {
          unlock.releaseLock();
        }
      });
    });

    // ---- Run the waterfall ----------------------------------------------
    Asyncc.Waterfall(stages, (err, all) -> {
      if (err != null) {
        outcome.fail(toThrowable(err));
        return;
      }
      rec.setResult(buildResultJson(all));
      outcome.complete(rec);
    });
  }

  // ------------------------------------------------------------------ //
  //  helpers                                                           //
  // ------------------------------------------------------------------ //

  private static JsonObject buildResultJson(final Map<String, Object> all) {
    final JsonObject out = new JsonObject();
    out.put("config", String.valueOf(all.get("config")));
    out.put("shardsGenerated", ((List<?>) all.getOrDefault("shards", List.of())).size());
    out.put("enrichedCount", ((List<?>) all.getOrDefault("enriched", List.of())).size());
    out.put("keptCount", ((List<?>) all.getOrDefault("kept", List.of())).size());
    out.put("byRegion", asJsonObject(asMapOfStringToList(all.get("byRegion"))));
    out.put("regionsLogged", String.valueOf(all.get("regionsLogged")));
    out.put("raceWinner", String.valueOf(all.get("raceWinner")));
    out.put("aggregateTotal", String.valueOf(all.get("aggregateTotal")));
    out.put("manifest", String.valueOf(all.get("manifest")));
    out.put("publicationCount", String.valueOf(all.get("publicationCount")));
    return out;
  }

  private static JsonObject asJsonObject(final Map<String, List<String>> byRegion) {
    final JsonObject obj = new JsonObject();
    if (byRegion == null) return obj;
    for (Map.Entry<String, List<String>> e : byRegion.entrySet()) {
      obj.put(e.getKey(), e.getValue().size());
    }
    return obj;
  }

  @SuppressWarnings("unchecked")
  private static List<String> readListOfString(final NeoWaterfallI.AsyncCallback<?, ?> cb, final String key) {
    final Object raw = cb.get(key);
    if (raw == null) return Collections.emptyList();
    return (List<String>) raw;
  }

  @SuppressWarnings("unchecked")
  private static Map<String, List<String>> readMapOfRegionToList(final NeoWaterfallI.AsyncCallback<?, ?> cb, final String key) {
    final Object raw = cb.get(key);
    if (raw == null) return Collections.emptyMap();
    return (Map<String, List<String>>) raw;
  }

  @SuppressWarnings("unchecked")
  private static Map<String, List<String>> asMapOfStringToList(final Object raw) {
    if (raw == null) return null;
    return (Map<String, List<String>>) raw;
  }

  private static int extractIntField(final String json, final String field, final int fallback) {
    try {
      return MAPPER.readTree(json).path(field).asInt(fallback);
    } catch (Exception e) {
      return fallback;
    }
  }

  private static String extractStringField(final String json, final String field, final String fallback) {
    try {
      return MAPPER.readTree(json).path(field).asText(fallback);
    } catch (Exception e) {
      return fallback;
    }
  }

  /** Insert/overwrite a top-level key on the JSON object. */
  private static String mergeJson(final String json, final String key, final String value) {
    final int closingBrace = json.lastIndexOf('}');
    if (closingBrace < 0) {
      return "{\"" + key + "\":\"" + value + "\"}";
    }
    final String prefix = json.substring(0, closingBrace).trim();
    final String separator = prefix.endsWith("{") ? "" : ",";
    return prefix + separator + "\"" + key + "\":\"" + escape(value) + "\"}";
  }

  private static String escape(final String s) {
    return s.replace("\\", "\\\\").replace("\"", "\\\"");
  }

  private static void sleepThenDone(final org.ores.async.NeoRaceIfc.RaceCallback<String, Throwable> cb, final long ms, final String value) {
    // Use the JDK's delayedExecutor rather than Thread.sleep so the racer doesn't park a
    // platform thread. Both racers complete on the JDK's common scheduled-executor pool.
    java.util.concurrent.CompletableFuture
        .delayedExecutor(ms, java.util.concurrent.TimeUnit.MILLISECONDS)
        .execute(() -> cb.done(null, value));
  }

  private static Throwable toThrowable(final Object err) {
    if (err == null) return null;
    if (err instanceof Throwable) return (Throwable) err;
    return new RuntimeException(String.valueOf(err));
  }
}
