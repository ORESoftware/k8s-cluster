package com.oresoftware.dd.akkaws.bench;

import com.fasterxml.jackson.databind.ObjectMapper;
import com.fasterxml.jackson.databind.node.ObjectNode;
import com.oresoftware.dd.akkaws.pipeline.AkkaStreamsPipeline;
import com.oresoftware.dd.akkaws.pipeline.AsyncJavaPipeline;
import org.slf4j.Logger;
import org.slf4j.LoggerFactory;

import java.util.ArrayList;
import java.util.Arrays;
import java.util.List;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.CompletionStage;
import java.util.concurrent.TimeUnit;

/**
 * Drives the two pipeline implementations through an identical workload and produces a
 * side-by-side timing summary.
 *
 * <p>The harness is intentionally simple — sequential request injection with per-iteration
 * latency measurement — because the goal here is *comparability*, not absolute throughput.
 * Anything more elaborate (parallel injection, warmup phases, GC pause filtering) would also
 * benefit one implementation differently than the other and muddy the read on which library
 * is doing what.
 *
 * <p>The benchmark <strong>does</strong> run a small warmup pass before each measurement loop
 * to absorb JIT compilation and class-loading cost; both implementations get the same warmup
 * count.
 */
public final class BenchmarkRunner {

  private static final Logger log = LoggerFactory.getLogger(BenchmarkRunner.class);

  private static final ObjectMapper MAPPER = new ObjectMapper();
  private static final int WARMUP_ITERATIONS = 20;

  private final AsyncJavaPipeline asyncJavaPipeline;
  private final AkkaStreamsPipeline akkaStreamsPipeline;

  public BenchmarkRunner(final AsyncJavaPipeline asyncJavaPipeline,
                         final AkkaStreamsPipeline akkaStreamsPipeline) {
    this.asyncJavaPipeline = asyncJavaPipeline;
    this.akkaStreamsPipeline = akkaStreamsPipeline;
  }

  /**
   * Run {@code iterations} of each pipeline against {@code payload} and return a JSON summary.
   *
   * <p>The returned {@link CompletionStage} is non-blocking from the caller's perspective —
   * the actual benchmark work runs on the pipelines' own executors. Both pipelines see the
   * same payload; ordering between them is documented in the JSON output's {@code timestamp}
   * fields.
   */
  public CompletionStage<String> runAsync(final int iterations, final String payload) {
    return CompletableFuture.supplyAsync(() -> {
      try {
        final Stats async = measure("asyncjava", iterations, () -> asyncJavaPipeline.process(payload));
        final Stats streams = measure("akkastreams", iterations,
            () -> akkaStreamsPipeline.process(payload).toCompletableFuture());
        return MAPPER.writerWithDefaultPrettyPrinter().writeValueAsString(summary(payload, iterations, async, streams));
      } catch (Exception e) {
        log.error("benchmark failed", e);
        final ObjectNode err = MAPPER.createObjectNode();
        err.put("ok", false);
        err.put("error", e.toString());
        // Drill into the chain so the surfaced JSON tells the operator *why* the benchmark
        // failed, not just which stage logged the wrapper.
        final var chain = MAPPER.createArrayNode();
        for (Throwable cur = e; cur != null && cur.getCause() != cur; cur = cur.getCause()) {
          chain.add(cur.getClass().getName() + ": " + String.valueOf(cur.getMessage()));
          if (cur.getCause() == null) break;
        }
        err.set("causeChain", chain);
        return err.toString();
      }
    });
  }

  // --- core ---

  private Stats measure(final String label, final int iterations,
                        final java.util.function.Supplier<CompletableFuture<String>> driver) {

    // Warmup — discard timings. First-iteration latency can blow past several seconds on a
    // cold JIT (class loading, deoptimization recompiles); give it room so we don't
    // misattribute slow startup to one of the pipelines being broken.
    for (int i = 0; i < WARMUP_ITERATIONS; i++) {
      try {
        driver.get().get(30, TimeUnit.SECONDS);
      } catch (Exception e) {
        throw new RuntimeException("warmup failure for " + label + " at i=" + i, e);
      }
    }

    final long[] latenciesNs = new long[iterations];
    final long t0 = System.nanoTime();

    for (int i = 0; i < iterations; i++) {
      final long start = System.nanoTime();
      try {
        driver.get().get(10, TimeUnit.SECONDS);
      } catch (Exception e) {
        throw new RuntimeException("measure failure for " + label + " at i=" + i, e);
      }
      latenciesNs[i] = System.nanoTime() - start;
    }

    final long wallNs = System.nanoTime() - t0;
    return Stats.from(label, latenciesNs, wallNs);
  }

  private ObjectNode summary(final String payload, final int iterations, final Stats a, final Stats b) {
    final ObjectNode out = MAPPER.createObjectNode();
    out.put("ok", true);
    out.put("iterations", iterations);
    out.put("warmupIterations", WARMUP_ITERATIONS);
    out.put("payloadBytes", payload.getBytes().length);
    out.put("nowEpochMillis", System.currentTimeMillis());
    out.set("asyncjava", a.toJson(MAPPER));
    out.set("akkastreams", b.toJson(MAPPER));
    out.set("delta", deltaJson(a, b));
    return out;
  }

  private ObjectNode deltaJson(final Stats a, final Stats b) {
    final ObjectNode delta = MAPPER.createObjectNode();
    delta.put("p50_ratio_asyncjava_over_akkastreams", round(a.p50Us / (double) b.p50Us));
    delta.put("p95_ratio_asyncjava_over_akkastreams", round(a.p95Us / (double) b.p95Us));
    delta.put("p99_ratio_asyncjava_over_akkastreams", round(a.p99Us / (double) b.p99Us));
    delta.put("throughput_ratio_asyncjava_over_akkastreams",
        round((a.throughputReqPerSec) / b.throughputReqPerSec));
    return delta;
  }

  private static double round(final double v) {
    return Math.round(v * 1000.0) / 1000.0;
  }

  /** Per-pipeline timing summary. */
  static final class Stats {
    final String label;
    final long p50Us, p95Us, p99Us, maxUs, minUs;
    final double meanUs;
    final double throughputReqPerSec;
    final long wallNs;

    private Stats(String label, long p50Us, long p95Us, long p99Us, long maxUs, long minUs,
                  double meanUs, double throughputReqPerSec, long wallNs) {
      this.label = label;
      this.p50Us = p50Us;
      this.p95Us = p95Us;
      this.p99Us = p99Us;
      this.maxUs = maxUs;
      this.minUs = minUs;
      this.meanUs = meanUs;
      this.throughputReqPerSec = throughputReqPerSec;
      this.wallNs = wallNs;
    }

    static Stats from(final String label, final long[] latenciesNs, final long wallNs) {
      final long[] sorted = Arrays.copyOf(latenciesNs, latenciesNs.length);
      Arrays.sort(sorted);
      final List<Long> usList = new ArrayList<>(sorted.length);
      long sumUs = 0;
      for (long ns : sorted) {
        final long us = ns / 1000;
        usList.add(us);
        sumUs += us;
      }
      final long p50 = pct(usList, 0.50);
      final long p95 = pct(usList, 0.95);
      final long p99 = pct(usList, 0.99);
      final long max = usList.get(usList.size() - 1);
      final long min = usList.get(0);
      final double mean = sumUs / (double) usList.size();
      final double throughput = latenciesNs.length / (wallNs / 1_000_000_000.0);
      return new Stats(label, p50, p95, p99, max, min, mean, throughput, wallNs);
    }

    private static long pct(final List<Long> sortedUs, final double q) {
      final int idx = Math.min(sortedUs.size() - 1, (int) Math.ceil(q * sortedUs.size()) - 1);
      return sortedUs.get(Math.max(0, idx));
    }

    ObjectNode toJson(final ObjectMapper m) {
      final ObjectNode o = m.createObjectNode();
      o.put("label", label);
      o.put("p50_us", p50Us);
      o.put("p95_us", p95Us);
      o.put("p99_us", p99Us);
      o.put("min_us", minUs);
      o.put("max_us", maxUs);
      o.put("mean_us", Math.round(meanUs));
      o.put("throughput_req_per_sec", Math.round(throughputReqPerSec));
      o.put("wall_ms", wallNs / 1_000_000);
      return o;
    }
  }
}
