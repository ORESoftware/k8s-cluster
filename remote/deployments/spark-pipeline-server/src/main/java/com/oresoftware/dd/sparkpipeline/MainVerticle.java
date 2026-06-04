package com.oresoftware.dd.sparkpipeline;

import com.oresoftware.dd.sparkpipeline.db.PgDb;
import com.oresoftware.dd.sparkpipeline.handlers.HealthHandler;
import com.oresoftware.dd.sparkpipeline.handlers.JobStatusHandler;
import com.oresoftware.dd.sparkpipeline.handlers.ListJobsHandler;
import com.oresoftware.dd.sparkpipeline.handlers.ListReposHandler;
import com.oresoftware.dd.sparkpipeline.handlers.MetricsHandler;
import com.oresoftware.dd.sparkpipeline.handlers.SubmitJobHandler;
import com.oresoftware.dd.sparkpipeline.pipeline.JobService;
import io.vertx.core.AbstractVerticle;
import io.vertx.core.Handler;
import io.vertx.core.Promise;
import io.vertx.core.http.HttpServerOptions;
import io.vertx.ext.web.Router;
import io.vertx.ext.web.RoutingContext;
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

  private final Config config;
  private final JobService jobService;
  private final PgDb pg;
  private final Auth auth;

  /**
   * Constructed with a pre-built {@link JobService} and an optional {@link PgDb} so a single
   * instance of each is shared across all deployed replicas of this verticle (Vert.x deploys
   * multiple instances of a verticle, one per event loop, when {@code setInstances} &gt; 1).
   *
   * @param pg may be {@code null} when {@code RDS_DATABASE_URL} is unset; DB-backed endpoints
   *           respond 503 in that case.
   */
  public MainVerticle(final Config config, final JobService jobService, final PgDb pg) {
    this.config = config;
    this.jobService = jobService;
    this.pg = pg;
    this.auth = new Auth(config);
  }

  @Override
  public void start(final Promise<Void> startPromise) {

    final Router router = Router.router(vertx);
    router.route().handler(BodyHandler.create().setBodyLimit(8L * 1024L * 1024L));

    router.get("/healthz").handler(HealthHandler.liveness());
    router.get("/readyz").handler(HealthHandler.readiness(jobService));
    router.get("/metrics").handler(MetricsHandler.create());

    final Handler<RoutingContext> authGate = ctx -> {
      if (auth.isAuthorized(ctx)) {
        ctx.next();
      } else {
        ctx.response()
            .setStatusCode(401)
            .putHeader("content-type", "application/json")
            .end("{\"error\":\"unauthorized\"}");
      }
    };

    router.post("/v1/jobs").handler(authGate).handler(new SubmitJobHandler(jobService));
    router.get("/v1/jobs").handler(authGate).handler(new ListJobsHandler(jobService));
    router.get("/v1/jobs/:id").handler(authGate).handler(new JobStatusHandler(jobService));
    router.get("/v1/repos").handler(authGate).handler(new ListReposHandler(vertx, pg));

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
        .listen(config.httpPort, config.httpHost)
        .onSuccess(server -> {
          log.info("dd-spark-pipeline-server listening on {}:{}", config.httpHost, server.actualPort());
          startPromise.complete();
        })
        .onFailure(err -> {
          log.error("dd-spark-pipeline-server failed to bind {}:{}", config.httpHost, config.httpPort, err);
          startPromise.fail(err);
        });
  }

  @Override
  public void stop(final Promise<Void> stopPromise) {
    // Note: JobService is owned by App and shared across replicas; it is shut down once in the
    // process shutdown hook, not per-verticle.
    stopPromise.complete();
  }

}
