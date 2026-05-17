package com.oresoftware.dd.sparkpipeline;

import com.oresoftware.dd.sparkpipeline.db.PgDb;
import com.oresoftware.dd.sparkpipeline.pipeline.JobService;
import io.vertx.core.DeploymentOptions;
import io.vertx.core.Vertx;
import io.vertx.core.VertxOptions;
import io.vertx.micrometer.MicrometerMetricsOptions;
import io.vertx.micrometer.VertxPrometheusOptions;
import org.slf4j.Logger;
import org.slf4j.LoggerFactory;

import java.util.Optional;

/**
 * Process entry point for the dd-spark-pipeline-server.
 *
 * <p>The service is a Vert.x HTTP server that orchestrates Spark and other JVM-ecosystem data
 * pipeline jobs. All multi-stage flow control inside job handlers is done with
 * {@code org.ores.async.Asyncc} ("async.java") to keep the Vert.x event loop unblocked.
 */
public final class App {

  private static final Logger log = LoggerFactory.getLogger(App.class);

  private App() {
  }

  public static void main(final String[] args) {

    final MicrometerMetricsOptions metricsOptions = new MicrometerMetricsOptions()
        .setPrometheusOptions(new VertxPrometheusOptions().setEnabled(true))
        .setEnabled(true);

    final VertxOptions vertxOptions = new VertxOptions()
        .setMetricsOptions(metricsOptions);

    final Vertx vertx = Vertx.vertx(vertxOptions);

    // PgDb is optional: if RDS_DATABASE_URL is unset, fromEnv() returns empty and DB-backed
    // endpoints respond 503 instead of crash-looping. Shared across MainVerticle replicas so
    // we only open one HikariCP pool per process.
    final Optional<PgDb> pg = PgDb.fromEnv();

    // JobService owns the in-memory job state and the NeoQueue. It is shared across all
    // MainVerticle replicas so that POST/GET requests routed to different event loops still see
    // the same set of jobs. JobService consults Postgres (via the pg-defs jOOQ Tables) for
    // metadata lookups during SPARK_SUBMIT prechecks.
    final JobService jobService = new JobService(vertx, pg.orElse(null));

    final DeploymentOptions opts = new DeploymentOptions()
        .setInstances(Math.max(1, Runtime.getRuntime().availableProcessors()));

    vertx.deployVerticle(() -> new MainVerticle(jobService, pg.orElse(null)), opts).onComplete(ar -> {
      if (ar.succeeded()) {
        log.info("dd-spark-pipeline-server deployed verticles, deployment id={}", ar.result());
      } else {
        log.error("dd-spark-pipeline-server failed to deploy", ar.cause());
        vertx.close();
        System.exit(1);
      }
    });

    Runtime.getRuntime().addShutdownHook(new Thread(() -> {
      log.info("dd-spark-pipeline-server shutting down");
      jobService.shutdown();
      pg.ifPresent(PgDb::close);
      vertx.close();
    }, "shutdown-hook"));
  }
}
