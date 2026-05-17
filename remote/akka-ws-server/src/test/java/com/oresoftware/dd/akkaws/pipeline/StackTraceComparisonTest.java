package com.oresoftware.dd.akkaws.pipeline;

import akka.actor.testkit.typed.javadsl.ActorTestKit;
import org.junit.jupiter.api.AfterAll;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.Test;

import java.io.PrintWriter;
import java.io.StringWriter;
import java.util.concurrent.ExecutionException;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.TimeUnit;

import static org.junit.jupiter.api.Assertions.assertTrue;
import static org.junit.jupiter.api.Assertions.fail;

/**
 * Triggers the same in-pipeline failure ({@code PipelineStages.poison}) through both
 * implementations and prints the captured stack traces side by side. Purpose: give a
 * concrete, auditable view of the debuggability difference between async.java callback
 * orchestration and Akka Streams' stream-graph orchestration, which the comparison readme
 * references.
 *
 * <p>This is a print-and-assert-shape test, not a string-match test against a frozen trace —
 * stack-trace layout depends on JDK version, JIT inlining, and Akka version, and pinning the
 * exact frames would make this brittle.
 */
class StackTraceComparisonTest {

  private static ActorTestKit testKit;
  private static ExecutorService asyncExec;

  @BeforeAll
  static void setUp() {
    testKit = ActorTestKit.create();
    asyncExec = Executors.newFixedThreadPool(2);
  }

  @AfterAll
  static void tearDown() {
    asyncExec.shutdownNow();
    testKit.shutdownTestKit();
  }

  @Test
  void compareStackTraces() throws Exception {
    final String poison = "{\"id\":\"poison\",\"payload\":\"x\"}";

    final Throwable asyncJavaTrace = captureAsyncJavaError(poison);
    final Throwable akkaStreamsTrace = captureAkkaStreamsError(poison);

    System.out.println();
    System.out.println("=========== ASYNC.JAVA STACK TRACE ===========");
    asyncJavaTrace.printStackTrace(System.out);
    System.out.println("==============================================");
    System.out.println();
    System.out.println("=========== AKKA STREAMS STACK TRACE =========");
    akkaStreamsTrace.printStackTrace(System.out);
    System.out.println("==============================================");
    System.out.println();

    final String asyncJavaTraceStr = stackToString(asyncJavaTrace);
    final String akkaStreamsTraceStr = stackToString(akkaStreamsTrace);

    // Both implementations must surface the root cause (PipelineStages.poison) in the chain.
    // We don't pin a frame index; we just want to know `score` and `poison` are reachable.
    assertTrue(asyncJavaTraceStr.contains("PipelineStages"),
        "async.java trace should mention PipelineStages");
    assertTrue(asyncJavaTraceStr.contains("poison-pill") || asyncJavaTraceStr.contains("poison"),
        "async.java trace should surface the poison-pill message");

    assertTrue(akkaStreamsTraceStr.contains("PipelineStages"),
        "akka-streams trace should mention PipelineStages");
    assertTrue(akkaStreamsTraceStr.contains("poison-pill") || akkaStreamsTraceStr.contains("poison"),
        "akka-streams trace should surface the poison-pill message");

    // Print a frame-count comparison so the difference is visible at a glance.
    final int asyncJavaFrames = asyncJavaTrace.getStackTrace().length;
    final int akkaStreamsFrames = akkaStreamsTrace.getStackTrace().length;
    System.out.printf("frame counts -- asyncjava: %d  |  akkastreams: %d%n",
        asyncJavaFrames, akkaStreamsFrames);
  }

  // --- helpers ---

  private Throwable captureAsyncJavaError(final String payload) {
    final var pipeline = new AsyncJavaPipeline(asyncExec);
    try {
      pipeline.process(payload).get(5, TimeUnit.SECONDS);
      fail("expected pipeline to fail");
      throw new AssertionError("unreachable");
    } catch (ExecutionException ee) {
      return ee.getCause() != null ? ee.getCause() : ee;
    } catch (Exception e) {
      return e;
    }
  }

  private Throwable captureAkkaStreamsError(final String payload) {
    final var pipeline = new AkkaStreamsPipeline(testKit.system());
    try {
      pipeline.process(payload).toCompletableFuture().get(5, TimeUnit.SECONDS);
      fail("expected pipeline to fail");
      throw new AssertionError("unreachable");
    } catch (ExecutionException ee) {
      return ee.getCause() != null ? ee.getCause() : ee;
    } catch (Exception e) {
      return e;
    }
  }

  private static String stackToString(final Throwable t) {
    final StringWriter sw = new StringWriter();
    t.printStackTrace(new PrintWriter(sw));
    return sw.toString();
  }
}
