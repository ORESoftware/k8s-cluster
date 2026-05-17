package com.oresoftware.dd.sparkpipeline.pipeline;

import com.oresoftware.dd.sparkpipeline.model.JobKind;
import com.oresoftware.dd.sparkpipeline.model.JobState;
import io.vertx.core.Vertx;
import io.vertx.core.json.JsonObject;
import io.vertx.junit5.VertxExtension;
import io.vertx.junit5.VertxTestContext;
import org.junit.jupiter.api.AfterEach;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.extension.ExtendWith;

import java.util.concurrent.TimeUnit;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNotNull;

@ExtendWith(VertxExtension.class)
class JobServiceTest {

  private Vertx vertx;
  private JobService svc;

  @BeforeEach
  void setUp() {
    vertx = Vertx.vertx();
    // null PgDb is the documented behaviour for tests / dev environments without Postgres;
    // SPARK_SUBMIT prechecks skip the repo lookup but the rest of the pipeline still runs.
    svc = new JobService(vertx, null);
  }

  @AfterEach
  void tearDown() throws InterruptedException {
    svc.shutdown();
    vertx.close().toCompletionStage().toCompletableFuture().orTimeout(5, TimeUnit.SECONDS);
  }

  @Test
  void syntheticSeriesCompletes(final VertxTestContext ctx) {
    svc.submitAndAwait(JobKind.SYNTHETIC_TEST, new JsonObject().put("stages", 4))
        .onComplete(ctx.succeeding(rec -> ctx.verify(() -> {
          assertEquals(JobState.SUCCEEDED, rec.getState());
          assertNotNull(rec.getResult());
          ctx.completeNow();
        })));
  }

  @Test
  void ingestWaterfallCompletes(final VertxTestContext ctx) {
    svc.submitAndAwait(JobKind.INGEST_VALIDATE_PUBLISH, new JsonObject())
        .onComplete(ctx.succeeding(rec -> ctx.verify(() -> {
          assertEquals(JobState.SUCCEEDED, rec.getState());
          assertNotNull(rec.getResult());
          assertNotNull(rec.getResult().getString("manifest"));
          ctx.completeNow();
        })));
  }

  @Test
  void sparkSubmitParallelCompletes(final VertxTestContext ctx) {
    svc.submitAndAwait(JobKind.SPARK_SUBMIT, new JsonObject())
        .onComplete(ctx.succeeding(rec -> ctx.verify(() -> {
          assertEquals(JobState.SUCCEEDED, rec.getState());
          assertNotNull(rec.getResult().getString("submission"));
          ctx.completeNow();
        })));
  }
}
