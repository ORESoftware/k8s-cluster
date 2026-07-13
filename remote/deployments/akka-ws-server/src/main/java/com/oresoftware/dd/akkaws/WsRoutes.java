package com.oresoftware.dd.akkaws;

import akka.NotUsed;
import akka.actor.typed.ActorSystem;
import akka.http.javadsl.model.ContentTypes;
import akka.http.javadsl.model.HttpEntities;
import akka.http.javadsl.model.HttpRequest;
import akka.http.javadsl.model.ws.Message;
import akka.http.javadsl.model.ws.TextMessage;
import akka.http.javadsl.server.AllDirectives;
import akka.http.javadsl.server.Route;
import akka.stream.javadsl.Flow;
import com.oresoftware.dd.akkaws.bench.BenchmarkRunner;
import com.oresoftware.dd.akkaws.pipeline.AkkaStreamsPipeline;
import com.oresoftware.dd.akkaws.pipeline.AsyncJavaPipeline;
import io.opentelemetry.api.OpenTelemetry;
import io.opentelemetry.api.trace.Span;
import io.opentelemetry.api.trace.SpanKind;
import io.opentelemetry.api.trace.StatusCode;
import io.opentelemetry.api.trace.Tracer;
import io.opentelemetry.context.Context;
import io.opentelemetry.context.propagation.TextMapGetter;
import org.slf4j.Logger;
import org.slf4j.LoggerFactory;

import java.nio.charset.StandardCharsets;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.CompletionStage;
import java.util.concurrent.atomic.AtomicLong;

/**
 * HTTP route definitions.
 *
 * <ul>
 *   <li>{@code GET /healthz}              — liveness probe.</li>
 *   <li>{@code GET /readyz}               — readiness probe.</li>
 *   <li>{@code GET /metrics}              — Prometheus scrape endpoint.</li>
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
  private final OpenTelemetry openTelemetry;
  private final Tracer tracer;

  /**
   * Reads W3C {@code traceparent} / {@code tracestate} out of an inbound Akka HTTP request so an
   * upstream trace context is continued rather than orphaned. Akka header names are matched
   * case-insensitively, which is what {@code getFirstHeader} already does.
   */
  private static final TextMapGetter<HttpRequest> REQUEST_GETTER = new TextMapGetter<>() {
    @Override
    public Iterable<String> keys(final HttpRequest carrier) {
      final java.util.List<String> names = new java.util.ArrayList<>();
      carrier.getHeaders().forEach(h -> names.add(h.name()));
      return names;
    }

    @Override
    public String get(final HttpRequest carrier, final String key) {
      if (carrier == null) {
        return null;
      }
      return carrier.getHeader(key).map(akka.http.javadsl.model.HttpHeader::value).orElse(null);
    }
  };
  private final AtomicLong asyncJavaMessagesIn = new AtomicLong();
  private final AtomicLong asyncJavaMessagesOut = new AtomicLong();
  private final AtomicLong asyncJavaBytesIn = new AtomicLong();
  private final AtomicLong asyncJavaBytesOut = new AtomicLong();
  private final AtomicLong asyncJavaErrors = new AtomicLong();
  private final AtomicLong akkaStreamsMessagesIn = new AtomicLong();
  private final AtomicLong akkaStreamsMessagesOut = new AtomicLong();
  private final AtomicLong akkaStreamsBytesIn = new AtomicLong();
  private final AtomicLong akkaStreamsBytesOut = new AtomicLong();
  private final AtomicLong akkaStreamsErrors = new AtomicLong();

  public WsRoutes(final ActorSystem<?> system,
                  final AsyncJavaPipeline asyncJavaPipeline,
                  final AkkaStreamsPipeline akkaStreamsPipeline,
                  final Telemetry telemetry) {
    this.system = system;
    this.asyncJavaPipeline = asyncJavaPipeline;
    this.akkaStreamsPipeline = akkaStreamsPipeline;
    this.benchmark = new BenchmarkRunner(asyncJavaPipeline, akkaStreamsPipeline);
    this.openTelemetry = telemetry.openTelemetry();
    this.tracer = telemetry.tracer();
  }

  public Route all() {
    // One SERVER span per HTTP route. `traced` extracts any inbound W3C traceparent so we continue
    // the caller's trace, opens the span, and ends it when the response is produced.
    return traced(() -> concat(
        path("healthz", () -> get(() -> complete("ok\n"))),
        path("readyz",  () -> get(() -> complete("ready\n"))),
        path("metrics",
            () -> get(() -> complete(HttpEntities.create(ContentTypes.TEXT_PLAIN_UTF8, metricsText())))),
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
    ));
  }

  /**
   * Wraps {@code inner} in a SERVER span. The parent context is extracted from the request's W3C
   * {@code traceparent}/{@code tracestate} headers (so a caller-initiated trace continues), the
   * span is named {@code "HTTP <METHOD> <path>"}, and the response status code is recorded before
   * the span ends. The span is closed via {@code mapResponse}, which fires once the inner route
   * has produced its {@code HttpResponse} — including for upgraded WebSocket handshakes.
   */
  private Route traced(final java.util.function.Supplier<Route> inner) {
    return extractRequest(request -> {
      final Context parent = openTelemetry.getPropagators().getTextMapPropagator()
          .extract(Context.current(), request, REQUEST_GETTER);
      final String method = request.method().value();
      final String path = request.getUri().getPathString();
      final Span span = tracer.spanBuilder("HTTP " + method + " " + path)
          .setParent(parent)
          .setSpanKind(SpanKind.SERVER)
          .setAttribute("http.request.method", method)
          .setAttribute("url.path", path)
          .setAttribute("server.address", request.getUri().getHost().address())
          .startSpan();
      return mapResponse(response -> {
        try {
          final int status = response.status().intValue();
          span.setAttribute("http.response.status_code", (long) status);
          if (status >= 500) {
            span.setStatus(StatusCode.ERROR);
          }
        } finally {
          span.end();
        }
        return response;
      }, inner);
    });
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
        .mapAsync(/* parallelism */ 8, this::processAsyncJavaFrame)
        .map(TextMessage::create);
  }

  /** Same shape, driven by the Akka Streams pipeline. */
  private Flow<Message, Message, NotUsed> akkaStreamsWsFlow() {
    return Flow.<Message>create()
        .filter(Message::isText)
        .map(m -> m.asTextMessage().getStrictText())
        .mapAsync(/* parallelism */ 8, this::processAkkaStreamsFrame)
        .map(TextMessage::create);
  }

  private CompletionStage<String> processAsyncJavaFrame(final String inputFrame) {
    asyncJavaMessagesIn.incrementAndGet();
    asyncJavaBytesIn.addAndGet(utf8Bytes(inputFrame));

    final CompletableFuture<String> done = asyncJavaPipeline.process(inputFrame);
    return done
        .thenApply(body -> observeAsyncJavaOutput(okFrame(body)))
        .exceptionally(e -> {
          asyncJavaErrors.incrementAndGet();
          return observeAsyncJavaOutput(errFrame("asyncjava", e));
        });
  }

  private CompletionStage<String> processAkkaStreamsFrame(final String inputFrame) {
    akkaStreamsMessagesIn.incrementAndGet();
    akkaStreamsBytesIn.addAndGet(utf8Bytes(inputFrame));

    return akkaStreamsPipeline.process(inputFrame)
        .toCompletableFuture()
        .thenApply(body -> observeAkkaStreamsOutput(okFrame(body)))
        .exceptionally(e -> {
          akkaStreamsErrors.incrementAndGet();
          return observeAkkaStreamsOutput(errFrame("akkastreams", e));
        });
  }

  private String observeAsyncJavaOutput(final String frame) {
    asyncJavaMessagesOut.incrementAndGet();
    asyncJavaBytesOut.addAndGet(utf8Bytes(frame));
    return frame;
  }

  private String observeAkkaStreamsOutput(final String frame) {
    akkaStreamsMessagesOut.incrementAndGet();
    akkaStreamsBytesOut.addAndGet(utf8Bytes(frame));
    return frame;
  }

  private String metricsText() {
    final StringBuilder b = new StringBuilder(2048);
    appendMetric(b, "dd_akka_ws_async_java_messages_in_total", "counter",
        "Total text frames received by the async.java websocket pipeline.",
        asyncJavaMessagesIn.get());
    appendMetric(b, "dd_akka_ws_async_java_messages_out_total", "counter",
        "Total text frames sent by the async.java websocket pipeline.",
        asyncJavaMessagesOut.get());
    appendMetric(b, "dd_akka_ws_async_java_bytes_in_total", "counter",
        "Total text-frame bytes received by the async.java websocket pipeline.",
        asyncJavaBytesIn.get());
    appendMetric(b, "dd_akka_ws_async_java_bytes_out_total", "counter",
        "Total text-frame bytes sent by the async.java websocket pipeline.",
        asyncJavaBytesOut.get());
    appendMetric(b, "dd_akka_ws_async_java_errors_total", "counter",
        "Total async.java websocket pipeline errors converted to error frames.",
        asyncJavaErrors.get());
    appendMetric(b, "dd_akka_ws_akka_streams_messages_in_total", "counter",
        "Total text frames received by the Akka Streams websocket pipeline.",
        akkaStreamsMessagesIn.get());
    appendMetric(b, "dd_akka_ws_akka_streams_messages_out_total", "counter",
        "Total text frames sent by the Akka Streams websocket pipeline.",
        akkaStreamsMessagesOut.get());
    appendMetric(b, "dd_akka_ws_akka_streams_bytes_in_total", "counter",
        "Total text-frame bytes received by the Akka Streams websocket pipeline.",
        akkaStreamsBytesIn.get());
    appendMetric(b, "dd_akka_ws_akka_streams_bytes_out_total", "counter",
        "Total text-frame bytes sent by the Akka Streams websocket pipeline.",
        akkaStreamsBytesOut.get());
    appendMetric(b, "dd_akka_ws_akka_streams_errors_total", "counter",
        "Total Akka Streams websocket pipeline errors converted to error frames.",
        akkaStreamsErrors.get());
    return b.toString();
  }

  private static void appendMetric(final StringBuilder b,
                                   final String name,
                                   final String metricType,
                                   final String help,
                                   final long value) {
    b.append("# HELP ").append(name).append(' ').append(help).append('\n');
    b.append("# TYPE ").append(name).append(' ').append(metricType).append('\n');
    b.append(name).append(' ').append(value).append('\n');
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

  private static long utf8Bytes(final String s) {
    if (s == null) return 0L;
    return s.getBytes(StandardCharsets.UTF_8).length;
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
