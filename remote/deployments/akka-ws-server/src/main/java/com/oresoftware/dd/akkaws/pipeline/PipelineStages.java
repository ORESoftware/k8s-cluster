package com.oresoftware.dd.akkaws.pipeline;

import com.fasterxml.jackson.databind.JsonNode;
import com.fasterxml.jackson.databind.ObjectMapper;
import com.fasterxml.jackson.databind.node.ObjectNode;

import java.util.Objects;
import java.util.concurrent.ThreadLocalRandom;

/**
 * The actual stage logic, shared verbatim between the async.java and Akka Streams pipeline
 * implementations. Each stage is a plain Java function that either returns a value or throws.
 *
 * <p>The point of the comparison this module exists for is that <em>only</em> the orchestration
 * differs between the two implementations — the per-stage work is identical, byte for byte, so
 * any performance / debuggability difference observed downstream is attributable to the
 * coordination library, not to the work itself.
 *
 * <p>The five stages model a realistic-ish WebSocket request pipeline:
 *
 * <ol>
 *   <li>{@link #parse(String)} — decode an incoming text frame as JSON.</li>
 *   <li>{@link #validate(JsonNode)} — schema-check; reject if {@code id} or {@code payload}
 *       are missing.</li>
 *   <li>{@link #enrichLookupA(JsonNode)} / {@link #enrichLookupB(JsonNode)} — two fan-out
 *       simulated downstream lookups (each sleeps a few ms to mimic an HTTP / DB hop).</li>
 *   <li>{@link #score(JsonNode, String, String)} — combine the parent record with both
 *       enrichments into a score.</li>
 *   <li>{@link #serialize(JsonNode)} — encode back to JSON text.</li>
 * </ol>
 *
 * <p>{@link #poison(JsonNode)} is included so the
 * {@code StackTraceComparisonTest} can deliberately throw from inside the score stage and the
 * two pipelines' stack-trace shapes can be diffed.
 */
public final class PipelineStages {

  private PipelineStages() {
  }

  private static final ObjectMapper MAPPER = new ObjectMapper();

  /** Simulated downstream-lookup latency, ms. Kept short so unit tests stay fast. */
  static final int LOOKUP_LATENCY_MIN_MS = 1;
  static final int LOOKUP_LATENCY_MAX_MS = 4;

  public static JsonNode parse(final String input) throws Exception {
    Objects.requireNonNull(input, "input");
    return MAPPER.readTree(input);
  }

  public static JsonNode validate(final JsonNode parsed) {
    if (parsed == null || !parsed.has("id")) {
      throw new IllegalArgumentException("validate: missing required field `id`");
    }
    if (!parsed.has("payload")) {
      throw new IllegalArgumentException("validate: missing required field `payload`");
    }
    return parsed;
  }

  public static String enrichLookupA(final JsonNode validated) {
    sleepMs(randomLatencyMs());
    return "lookupA[" + validated.get("id").asText() + "]=" + (validated.get("id").asText().hashCode() & 0xffff);
  }

  public static String enrichLookupB(final JsonNode validated) {
    sleepMs(randomLatencyMs());
    return "lookupB[" + validated.get("id").asText() + "]=" + (validated.get("payload").toString().length());
  }

  public static JsonNode score(final JsonNode validated, final String lookupA, final String lookupB) {
    if ("poison".equals(validated.path("id").asText())) {
      // Deliberate trigger for stack-trace comparison test. Mimics a real bug deep in the
      // last business stage.
      throw poison(validated);
    }
    final int composite = lookupA.length() * 31 + lookupB.length();
    final ObjectNode out = MAPPER.createObjectNode();
    out.put("id", validated.get("id").asText());
    out.put("score", composite);
    out.put("lookupA", lookupA);
    out.put("lookupB", lookupB);
    return out;
  }

  public static String serialize(final JsonNode scored) throws Exception {
    return MAPPER.writeValueAsString(scored);
  }

  // --- helpers ---------------------------------------------------------------------------

  /**
   * Deliberate exception thrown from inside {@link #score} for the
   * {@code StackTraceComparisonTest}. Kept as a separate method so the stack trace shows
   * `PipelineStages.poison(...)` at the bottom, demonstrating that whatever the coordination
   * library does on top, the *cause* of the failure is always discoverable in the trace.
   */
  public static RuntimeException poison(final JsonNode validated) {
    return new IllegalStateException("score: deliberate poison-pill id=" + validated.path("id").asText());
  }

  private static int randomLatencyMs() {
    return ThreadLocalRandom.current().nextInt(LOOKUP_LATENCY_MIN_MS, LOOKUP_LATENCY_MAX_MS + 1);
  }

  private static void sleepMs(final long ms) {
    if (ms <= 0) {
      return;
    }
    try {
      Thread.sleep(ms);
    } catch (InterruptedException ie) {
      Thread.currentThread().interrupt();
    }
  }
}
