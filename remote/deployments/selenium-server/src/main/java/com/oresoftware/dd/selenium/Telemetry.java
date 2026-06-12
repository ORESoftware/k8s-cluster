package com.oresoftware.dd.selenium;

import io.opentelemetry.api.common.Attributes;
import io.opentelemetry.api.common.AttributesBuilder;
import io.opentelemetry.api.trace.propagation.W3CTraceContextPropagator;
import io.opentelemetry.context.propagation.ContextPropagators;
import io.opentelemetry.exporter.otlp.http.trace.OtlpHttpSpanExporter;
import io.opentelemetry.sdk.OpenTelemetrySdk;
import io.opentelemetry.sdk.resources.Resource;
import io.opentelemetry.sdk.trace.SdkTracerProvider;
import io.opentelemetry.sdk.trace.export.BatchSpanProcessor;
import io.opentelemetry.semconv.ServiceAttributes;
import org.slf4j.Logger;
import org.slf4j.LoggerFactory;

/**
 * Explicit OpenTelemetry SDK wiring for dd-selenium-server.
 *
 * <p>No {@code -javaagent} and no bytecode rewriting. We build an {@link OpenTelemetrySdk}
 * by hand and hand it to Vert.x via {@code new OpenTelemetryOptions(openTelemetry())} in
 * {@link App}; Vert.x's first-class OpenTelemetry tracer then opens a SERVER span per HTTP
 * request on the event loop and propagates W3C {@code traceparent} automatically.
 *
 * <p>Configuration (env, all optional):
 * <ul>
 *   <li>{@code OTEL_EXPORTER_OTLP_ENDPOINT} — OTLP/HTTP base URL; defaults to the in-cluster
 *       {@code dd-otel-collector}. {@code /v1/traces} is appended for the traces signal.</li>
 *   <li>{@code OTEL_SERVICE_NAME} — resource {@code service.name}; defaults to
 *       {@code dd-selenium-server}.</li>
 *   <li>{@code POD_NAME} / {@code POD_NAMESPACE} — stamped as {@code k8s.pod.name} /
 *       {@code k8s.namespace.name} resource attributes (Kubernetes downward API).</li>
 * </ul>
 */
public final class Telemetry implements AutoCloseable {

  private static final Logger log = LoggerFactory.getLogger(Telemetry.class);

  private static final String DEFAULT_ENDPOINT =
      "http://dd-otel-collector.observability.svc.cluster.local:4318";

  private final OpenTelemetrySdk sdk;

  private Telemetry(final OpenTelemetrySdk sdk) {
    this.sdk = sdk;
  }

  /**
   * Builds and returns a configured {@link Telemetry}. The OTLP exporter connects lazily on first
   * export, so a bad collector endpoint degrades to dropped spans rather than a boot failure.
   */
  public static Telemetry init() {
    final String endpointBase = trimTrailingSlash(
        envOrDefault("OTEL_EXPORTER_OTLP_ENDPOINT", DEFAULT_ENDPOINT));
    final String serviceName = envOrDefault("OTEL_SERVICE_NAME", "dd-selenium-server");

    final AttributesBuilder attrs = Attributes.builder()
        .put(ServiceAttributes.SERVICE_NAME, serviceName);
    final String podName = System.getenv("POD_NAME");
    if (podName != null && !podName.isBlank()) {
      attrs.put("k8s.pod.name", podName.trim());
    }
    final String podNamespace = System.getenv("POD_NAMESPACE");
    if (podNamespace != null && !podNamespace.isBlank()) {
      attrs.put("k8s.namespace.name", podNamespace.trim());
    }

    final Resource resource = Resource.getDefault().merge(Resource.create(attrs.build()));

    final OtlpHttpSpanExporter exporter = OtlpHttpSpanExporter.builder()
        .setEndpoint(endpointBase + "/v1/traces")
        .build();

    final SdkTracerProvider tracerProvider = SdkTracerProvider.builder()
        .setResource(resource)
        .addSpanProcessor(BatchSpanProcessor.builder(exporter).build())
        .build();

    final OpenTelemetrySdk sdk = OpenTelemetrySdk.builder()
        .setTracerProvider(tracerProvider)
        .setPropagators(ContextPropagators.create(W3CTraceContextPropagator.getInstance()))
        .build();

    log.info("OpenTelemetry tracing initialised: service.name={} otlp.endpoint={}/v1/traces",
        serviceName, endpointBase);
    return new Telemetry(sdk);
  }

  /** The SDK instance handed to Vert.x's {@code OpenTelemetryOptions}. */
  public OpenTelemetrySdk openTelemetry() {
    return sdk;
  }

  /** Flushes batched spans and shuts the provider down on graceful termination. */
  @Override
  public void close() {
    try {
      sdk.getSdkTracerProvider().shutdown().join(5, java.util.concurrent.TimeUnit.SECONDS);
    } catch (final RuntimeException e) {
      log.warn("OpenTelemetry shutdown did not complete cleanly: {}", e.toString());
    }
  }

  private static String envOrDefault(final String name, final String fallback) {
    final String raw = System.getenv(name);
    return (raw == null || raw.isBlank()) ? fallback : raw.trim();
  }

  private static String trimTrailingSlash(final String s) {
    return s.endsWith("/") ? s.substring(0, s.length() - 1) : s;
  }
}
