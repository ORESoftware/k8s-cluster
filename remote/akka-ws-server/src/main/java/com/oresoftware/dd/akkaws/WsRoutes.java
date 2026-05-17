package com.oresoftware.dd.akkaws;

import akka.NotUsed;
import akka.actor.typed.ActorSystem;
import akka.http.javadsl.model.ContentTypes;
import akka.http.javadsl.model.HttpEntities;
import akka.http.javadsl.model.ws.Message;
import akka.http.javadsl.model.ws.TextMessage;
import akka.http.javadsl.server.AllDirectives;
import akka.http.javadsl.server.Route;
import akka.stream.javadsl.Flow;
import com.oresoftware.dd.akkaws.bench.BenchmarkRunner;
import com.oresoftware.dd.akkaws.pipeline.AkkaStreamsPipeline;
import com.oresoftware.dd.akkaws.pipeline.AsyncJavaPipeline;
import org.slf4j.Logger;
import org.slf4j.LoggerFactory;

import java.util.concurrent.CompletableFuture;
import java.util.concurrent.CompletionStage;

/**
 * HTTP route definitions.
 *
 * <ul>
 *   <li>{@code GET /healthz}              — liveness probe.</li>
 *   <li>{@code GET /readyz}               — readiness probe.</li>
 *   <li>{@code GET /ws/asyncjava}         — WebSocket; each text frame runs through the
 *       async.java pipeline.</li>
 *   <li>{@code GET /ws/akkastreams}       — WebSocket; same pipeline, Akka Streams.</li>
 *   <li>{@code GET /v1/benchmark}         — runs both pipelines a fixed number of iterations and
 *       returns a JSON timing summary. Iteration count comes from the
 *       {@code BENCHMARK_ITERATIONS} env var (default 200).</li>
 * </ul>
 */
public final class WsRoutes extends AllDirectives {

  private static final Logger log = LoggerFactory.getLogger(WsRoutes.class);

  private final ActorSystem<?> system;
  private final AsyncJavaPipeline asyncJavaPipeline;
  private final AkkaStreamsPipeline akkaStreamsPipeline;
  private final BenchmarkRunner benchmark;

  public WsRoutes(final ActorSystem<?> system,
                  final AsyncJavaPipeline asyncJavaPipeline,
                  final AkkaStreamsPipeline akkaStreamsPipeline) {
    this.system = system;
    this.asyncJavaPipeline = asyncJavaPipeline;
    this.akkaStreamsPipeline = akkaStreamsPipeline;
    this.benchmark = new BenchmarkRunner(asyncJavaPipeline, akkaStreamsPipeline);
  }

  public Route all() {
    return concat(
        path("healthz", () -> get(() -> complete("ok\n"))),
        path("readyz",  () -> get(() -> complete("ready\n"))),
        pathPrefix("ws", () -> concat(
            path("asyncjava",   () -> handleWebSocketMessages(asyncJavaWsFlow())),
            path("akkastreams", () -> handleWebSocketMessages(akkaStreamsWsFlow())))),
        pathPrefix("v1", () -> path("benchmark",
            () -> get(() -> {
              final int iterations = parsePositiveIntEnv("BENCHMARK_ITERATIONS", 200);
              final String payload = System.getenv()
                  .getOrDefault("BENCHMARK_PAYLOAD",
                      "{\"id\":\"bench\",\"payload\":\"a benchmark message body\"}");
              final CompletionStage<String> result = benchmark.runAsync(iterations, payload);
              return onSuccess(result,
                  json -> complete(HttpEntities.create(ContentTypes.APPLICATION_JSON, json)));
            })))
    );
  }

  /**
   * WS flow that runs every inbound text frame through the async.java pipeline and emits the
   * result back as a text frame. Errors are converted to a JSON-shaped error frame so the
   * client connection isn't torn down on a single bad input.
   */
  private Flow<Message, Message, NotUsed> asyncJavaWsFlow() {
    return Flow.<Message>create()
        .filter(Message::isText)
        .map(m -> m.asTextMessage().getStrictText())
        .mapAsync(/* parallelism */ 8, inputFrame -> {
          final CompletableFuture<String> done = asyncJavaPipeline.process(inputFrame);
          return done
              .thenApply(WsRoutes::okFrame)
              .exceptionally(e -> errFrame("asyncjava", e));
        })
        .map(TextMessage::create);
  }

  /** Same shape, driven by the Akka Streams pipeline. */
  private Flow<Message, Message, NotUsed> akkaStreamsWsFlow() {
    return Flow.<Message>create()
        .filter(Message::isText)
        .map(m -> m.asTextMessage().getStrictText())
        .mapAsync(/* parallelism */ 8, inputFrame ->
            akkaStreamsPipeline.process(inputFrame)
                .toCompletableFuture()
                .thenApply(WsRoutes::okFrame)
                .exceptionally(e -> errFrame("akkastreams", e)))
        .map(TextMessage::create);
  }

  private static String okFrame(final String body) {
    return "{\"ok\":true,\"result\":" + body + "}";
  }

  private static String errFrame(final String pipeline, final Throwable e) {
    final Throwable cause = (e.getCause() != null) ? e.getCause() : e;
    log.warn("pipeline={} failed: {}", pipeline, cause.toString());
    return "{\"ok\":false,\"pipeline\":\"" + pipeline + "\",\"error\":\""
        + cause.getClass().getSimpleName() + ": " + escape(cause.getMessage()) + "\"}";
  }

  private static String escape(final String s) {
    if (s == null) return "";
    return s.replace("\\", "\\\\").replace("\"", "\\\"");
  }

  private static int parsePositiveIntEnv(final String name, final int fallback) {
    final String raw = System.getenv(name);
    if (raw == null || raw.isBlank()) return fallback;
    try {
      final int v = Integer.parseInt(raw.trim());
      return v > 0 ? v : fallback;
    } catch (NumberFormatException nfe) {
      return fallback;
    }
  }
}
