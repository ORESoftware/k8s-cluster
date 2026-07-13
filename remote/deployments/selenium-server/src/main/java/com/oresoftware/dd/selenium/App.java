package com.oresoftware.dd.selenium;

import com.oresoftware.dd.selenium.run.ScenarioRunner;
import io.vertx.core.Vertx;
import io.vertx.core.VertxOptions;
import io.vertx.core.WorkerExecutor;
import io.vertx.micrometer.MicrometerMetricsOptions;
import io.vertx.micrometer.VertxPrometheusOptions;
import io.vertx.tracing.opentelemetry.OpenTelemetryOptions;
import org.slf4j.Logger;
import org.slf4j.LoggerFactory;

import java.util.concurrent.TimeUnit;

/**
 * Process entry point for dd-selenium-server.
 *
 * <p>The service is a Vert.x HTTP server whose {@code POST /run} endpoint drives the in-pod
 * Selenium Grid over RemoteWebDriver. WebDriver calls are blocking, so each scenario runs on a
 * bounded {@link WorkerExecutor} (sized to {@code SELENIUM_MAX_CONCURRENT}) rather than the event
 * loop.
 */
public final class App {

  private static final Logger log = LoggerFactory.getLogger(App.class);

  private App() {
  }

  public static void main(final String[] args) {

    final MicrometerMetricsOptions metricsOptions = new MicrometerMetricsOptions()
        .setPrometheusOptions(new VertxPrometheusOptions().setEnabled(true))
        .setEnabled(true);

    // Explicit OpenTelemetry SDK (no -javaagent), handed to Vert.x's native tracer so every HTTP
    // request gets a SERVER span and W3C traceparent propagation, on the event loop.
    final Telemetry telemetry = Telemetry.init();

    final VertxOptions vertxOptions = new VertxOptions()
        .setMetricsOptions(metricsOptions)
        .setTracingOptions(new OpenTelemetryOptions(telemetry.openTelemetry()));

    final Vertx vertx = Vertx.vertx(vertxOptions);
    final Config config = Config.fromEnv();

    // One bounded worker pool caps real browser parallelism per pod. The in-flight gauge plus a
    // 429 in RunHandler stops work from silently queueing behind the cap.
    final WorkerExecutor seleniumExecutor = vertx.createSharedWorkerExecutor(
        "selenium-worker",
        Math.max(1, config.maxConcurrent),
        Math.max(config.maxTimeoutMs + 30_000L, 60_000L),
        TimeUnit.MILLISECONDS);

    final ScenarioRunner runner = new ScenarioRunner(config);

    vertx.deployVerticle(new MainVerticle(config, runner, seleniumExecutor), depRes -> {
      if (depRes.succeeded()) {
        log.info("dd-selenium-server deployed verticle, deployment id={}", depRes.result());
      } else {
        log.error("dd-selenium-server failed to deploy", depRes.cause());
        vertx.close();
        System.exit(1);
      }
    });

    Runtime.getRuntime().addShutdownHook(new Thread(() -> {
      log.info("dd-selenium-server shutting down");
      seleniumExecutor.close();
      vertx.close();
      telemetry.close();
    }, "shutdown-hook"));
  }
}
