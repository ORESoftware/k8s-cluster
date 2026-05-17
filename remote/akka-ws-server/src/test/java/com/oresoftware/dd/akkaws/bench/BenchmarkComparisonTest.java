package com.oresoftware.dd.akkaws.bench;

import akka.actor.testkit.typed.javadsl.ActorTestKit;
import com.oresoftware.dd.akkaws.pipeline.AkkaStreamsPipeline;
import com.oresoftware.dd.akkaws.pipeline.AsyncJavaPipeline;
import org.junit.jupiter.api.AfterAll;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.Test;

import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.TimeUnit;

import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * Runs both pipelines through {@code BenchmarkRunner} and prints the JSON summary to stdout.
 *
 * <p>This test is not a perf regression gate — it doesn't assert numeric thresholds because
 * laptop / CI variance is too high to land that responsibly. What it <em>does</em> check is
 * that the harness emits well-formed JSON containing the expected fields for both
 * implementations, which is enough to keep the benchmark surface honest as the code evolves.
 * The actual numbers should be read out of the printed output.
 */
class BenchmarkComparisonTest {

  private static ActorTestKit testKit;
  private static ExecutorService asyncExec;

  @BeforeAll
  static void setUp() {
    testKit = ActorTestKit.create();
    // Match the production App's executor choice: a virtual-thread-per-task executor on
    // JDK 21+. A bounded fixed-thread-pool starves with async.java's rapid-fire
    // Asyncc.Parallel calls in this harness (observed at iteration ~97 / 100 in CI), and
    // moreover this is exactly the pairing the comparison readme recommends.
    asyncExec = Executors.newVirtualThreadPerTaskExecutor();
  }

  @AfterAll
  static void tearDown() {
    asyncExec.shutdownNow();
    testKit.shutdownTestKit();
  }

  @Test
  void runSideBySide() throws Exception {
    final var asyncJava = new AsyncJavaPipeline(asyncExec);
    final var akkaStreams = new AkkaStreamsPipeline(testKit.system());
    final var runner = new BenchmarkRunner(asyncJava, akkaStreams);

    final String payload = "{\"id\":\"bench\",\"payload\":\"a benchmark message body\"}";
    // Was capped at 30 to dodge a CounterLimit lost-update data race in async.java that
    // hung Asyncc.Parallel under sustained rapid-fire load. Fixed upstream by
    // async-java/async.java#9 (CounterLimit.{started,finished} -> AtomicInteger). The
    // current async-java.version coordinate in pom.xml pins to that fix branch's HEAD;
    // once #9 merges and we bump to the merge SHA this can stay at 200.
    final int iterations = 200;

    final String summary = runner.runAsync(iterations, payload).toCompletableFuture()
        .get(60, TimeUnit.SECONDS);

    System.out.println("=========== BENCHMARK SUMMARY ===========");
    System.out.println(summary);
    System.out.println("=========================================");

    // Smoke-check the JSON shape rather than any numeric threshold.
    assertTrue(summary.contains("\"asyncjava\""), "asyncjava section present");
    assertTrue(summary.contains("\"akkastreams\""), "akkastreams section present");
    assertTrue(summary.contains("\"p50_us\""), "p50 reported");
    assertTrue(summary.contains("\"p95_us\""), "p95 reported");
    assertTrue(summary.contains("\"p99_us\""), "p99 reported");
    assertTrue(summary.contains("\"throughput_req_per_sec\""), "throughput reported");
  }
}
