package com.oresoftware.dd.sparkpipeline;

import com.oresoftware.dd.sparkpipeline.handlers.HealthHandler;
import com.oresoftware.dd.sparkpipeline.handlers.JobStatusHandler;
import com.oresoftware.dd.sparkpipeline.handlers.ListJobsHandler;
import com.oresoftware.dd.sparkpipeline.handlers.MetricsHandler;
import com.oresoftware.dd.sparkpipeline.handlers.SubmitJobHandler;
import com.oresoftware.dd.sparkpipeline.pipeline.JobService;
import io.vertx.core.AbstractVerticle;
import io.vertx.core.Promise;
import io.vertx.core.http.HttpServerOptions;
import io.vertx.ext.web.Router;
import io.vertx.ext.web.handler.BodyHandler;
import org.slf4j.Logger;
import org.slf4j.LoggerFactory;

/**
 * The main Vert.x HTTP verticle.
 *
 * <p>Exposes:
 * <ul>
 *   <li>{@code GET  /healthz}        – liveness probe</li>
 *   <li>{@code GET  /readyz}         – readiness probe</li>
 *   <li>{@code GET  /metrics}        – Prometheus metrics scrape</li>
 *   <li>{@code POST /v1/jobs}        – submit a new pipeline job</li>
 *   <li>{@code GET  /v1/jobs}        – list all jobs known to this server</li>
 *   <li>{@code GET  /v1/jobs/:id}    – fetch status of a single job</li>
 * </ul>
 */
public final class MainVerticle extends AbstractVerticle {

  private static final Logger log = LoggerFactory.getLogger(MainVerticle.class);

  private static final String ENV_HTTP_HOST = "HTTP_HOST";
  private static final String ENV_HTTP_PORT = "HTTP_PORT";

  private final JobService jobService;

  /**
   * Constructed with a pre-built {@link JobService} so a single instance is shared across all
   * deployed replicas of this verticle (Vert.x deploys multiple instances of a verticle, one per
   * event loop, when {@code setInstances} &gt; 1).
   */
  public MainVerticle(final JobService jobService) {
    this.jobService = jobService;
  }

  @Override
  public void start(final Promise<Void> startPromise) {

    final String host = System.getenv().getOrDefault(ENV_HTTP_HOST, "0.0.0.0");
    final int port = parsePort(System.getenv(ENV_HTTP_PORT), 8085);

    final Router router = Router.router(vertx);
    router.route().handler(BodyHandler.create().setBodyLimit(8L * 1024L * 1024L));

    router.get("/healthz").handler(HealthHandler.liveness());
    router.get("/readyz").handler(HealthHandler.readiness(jobService));
    router.get("/metrics").handler(MetricsHandler.create());

    router.post("/v1/jobs").handler(new SubmitJobHandler(jobService));
    router.get("/v1/jobs").handler(new ListJobsHandler(jobService));
    router.get("/v1/jobs/:id").handler(new JobStatusHandler(jobService));

    router.errorHandler(500, ctx -> {
      log.error("Unhandled error on {}: {}", ctx.request().path(), ctx.failure() == null ? "n/a" : ctx.failure().toString(), ctx.failure());
      if (!ctx.response().ended()) {
        ctx.response()
            .setStatusCode(500)
            .putHeader("content-type", "application/json")
            .end("{\"error\":\"internal_server_error\"}");
      }
    });

    final HttpServerOptions httpOptions = new HttpServerOptions()
        .setLogActivity(false)
        .setIdleTimeout(120);

    vertx.createHttpServer(httpOptions)
        .requestHandler(router)
        .listen(port, host)
        .onSuccess(server -> {
          log.info("dd-spark-pipeline-server listening on {}:{}", host, server.actualPort());
          startPromise.complete();
        })
        .onFailure(err -> {
          log.error("dd-spark-pipeline-server failed to bind {}:{}", host, port, err);
          startPromise.fail(err);
        });
  }

  @Override
  public void stop(final Promise<Void> stopPromise) {
    // Note: JobService is owned by App and shared across replicas; it is shut down once in the
    // process shutdown hook, not per-verticle.
    stopPromise.complete();
  }

  private static int parsePort(final String raw, final int fallback) {
    if (raw == null || raw.isBlank()) {
      return fallback;
    }
    try {
      return Integer.parseInt(raw.trim());
    } catch (NumberFormatException nfe) {
      log.warn("invalid HTTP_PORT={}; using {}", raw, fallback);
      return fallback;
    }
  }
}
