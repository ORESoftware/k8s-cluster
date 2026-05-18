package com.oresoftware.dd.akkaws.comparison;

import org.junit.jupiter.api.AfterAll;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.Test;

import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.TimeUnit;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * Pins that the two async.java translations of the F# {@code rxFiveStages} pipeline produce
 * byte-identical happy-path output and structurally identical error frames.
 *
 * <p>The reference shape:
 *
 * <ul>
 *   <li>Happy path: both versions emit
 *       {@code {"ok":true,"result":{"id":"...","score":N,"lookupA":"...","lookupB":"..."}}}.</li>
 *   <li>Parse failure: both versions emit
 *       {@code {"ok":false,"error":"&lt;ExceptionType&gt;: &lt;message&gt;"}} and the future
 *       completes successfully (never exceptionally) — mirrors Rx's
 *       {@code .Catch(perMessageErrorFrame)} semantics.</li>
 * </ul>
 */
class RxFiveStagesComparisonTest {

  private static ExecutorService exec;

  @BeforeAll
  static void setUp() {
    exec = Executors.newFixedThreadPool(4);
  }

  @AfterAll
  static void tearDown() {
    exec.shutdownNow();
  }

  @Test
  void happyPath_bothVersionsEmitIdenticalFrames() throws Exception {
    final String input = "{\"id\":\"abc\",\"payload\":\"hi\"}";

    final String fromWaterfall = RxFiveStagesComparison
        .runWaterfall(input, exec).get(5, TimeUnit.SECONDS);
    final String fromAsyncFut = RxFiveStagesComparison
        .runWithAsyncFut(input, exec).get(5, TimeUnit.SECONDS);

    // Byte-for-byte equal: same stages, same inputs, same JSON serializer.
    assertEquals(fromWaterfall, fromAsyncFut,
        "Waterfall and AsyncFut versions of the same pipeline must produce identical output");
    assertTrue(fromWaterfall.startsWith("{\"ok\":true,\"result\":"));
    assertTrue(fromWaterfall.contains("\"score\""));
    assertTrue(fromWaterfall.contains("\"lookupA\""));
    assertTrue(fromWaterfall.contains("\"lookupB\""));
  }

  @Test
  void parseFailure_bothVersionsEmitErrorFrameInsteadOfThrowing() throws Exception {
    // "not json" trips PipelineStages.parse — both versions must trap the throw and produce a
    // perMessageErrorFrame on the SUCCESS channel of the returned future.
    final String fromWaterfall = RxFiveStagesComparison
        .runWaterfall("not json", exec).get(5, TimeUnit.SECONDS);
    final String fromAsyncFut = RxFiveStagesComparison
        .runWithAsyncFut("not json", exec).get(5, TimeUnit.SECONDS);

    assertTrue(fromWaterfall.startsWith("{\"ok\":false,\"error\":"),
        () -> "expected error frame, got: " + fromWaterfall);
    assertTrue(fromAsyncFut.startsWith("{\"ok\":false,\"error\":"),
        () -> "expected error frame, got: " + fromAsyncFut);
    assertFalse(fromWaterfall.contains("\"ok\":true"));
    assertFalse(fromAsyncFut.contains("\"ok\":true"));

    // Both must surface the same unwrapped exception class name (JsonParseException). The
    // promise-style version's .thenApply rethrows wrap the original in RuntimeException; the
    // perMessageErrorFrame helper unwraps that so the two versions report the same type.
    assertEquals(
        extractErrorType(fromWaterfall),
        extractErrorType(fromAsyncFut),
        "Both translations must report the same original exception type on parse failure");
  }

  @Test
  void validationFailure_bothVersionsEmitErrorFrame() throws Exception {
    // Missing "id" field -> PipelineStages.validate throws IllegalArgumentException.
    final String input = "{\"payload\":\"hi\"}";

    final String fromWaterfall = RxFiveStagesComparison
        .runWaterfall(input, exec).get(5, TimeUnit.SECONDS);
    final String fromAsyncFut = RxFiveStagesComparison
        .runWithAsyncFut(input, exec).get(5, TimeUnit.SECONDS);

    assertTrue(fromWaterfall.contains("IllegalArgumentException"),
        () -> "expected IllegalArgumentException in: " + fromWaterfall);
    assertTrue(fromAsyncFut.contains("IllegalArgumentException"),
        () -> "expected IllegalArgumentException in: " + fromAsyncFut);
    assertEquals(extractErrorType(fromWaterfall), extractErrorType(fromAsyncFut));
  }

  @Test
  void scoreFailure_bothVersionsEmitErrorFrame() throws Exception {
    // id="poison" trips PipelineStages.score's deliberate poison-pill (IllegalStateException).
    // This proves the error funnel works even when the failure originates DOWNSTREAM of the
    // Parallel/ParallelF fan-out, not just in the early stages.
    final String input = "{\"id\":\"poison\",\"payload\":\"hi\"}";

    final String fromWaterfall = RxFiveStagesComparison
        .runWaterfall(input, exec).get(5, TimeUnit.SECONDS);
    final String fromAsyncFut = RxFiveStagesComparison
        .runWithAsyncFut(input, exec).get(5, TimeUnit.SECONDS);

    assertTrue(fromWaterfall.contains("IllegalStateException"),
        () -> "expected IllegalStateException in: " + fromWaterfall);
    assertTrue(fromAsyncFut.contains("IllegalStateException"),
        () -> "expected IllegalStateException in: " + fromAsyncFut);
    assertTrue(fromWaterfall.contains("poison"));
    assertTrue(fromAsyncFut.contains("poison"));
  }

  // Extracts the bare exception type from the perMessageErrorFrame's
  // `"error":"TypeName: message"` field, so the two versions can be compared.
  private static String extractErrorType(final String frame) {
    final int errorIdx = frame.indexOf("\"error\":\"");
    if (errorIdx < 0) return "<no-error-field>";
    final int start = errorIdx + "\"error\":\"".length();
    final int colon = frame.indexOf(":", start);
    return colon < 0 ? frame.substring(start) : frame.substring(start, colon);
  }
}
