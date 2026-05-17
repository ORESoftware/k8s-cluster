package com.oresoftware.dd.sparkpipeline.handlers;

import com.oresoftware.dd.sparkpipeline.model.JobRecord;
import com.oresoftware.dd.sparkpipeline.pipeline.JobService;
import io.vertx.core.Handler;
import io.vertx.ext.web.RoutingContext;

import java.util.Optional;

/**
 * {@code GET /v1/jobs/:id} — retrieve a single job's current state and stage log.
 */
public final class JobStatusHandler implements Handler<RoutingContext> {

  private final JobService svc;

  public JobStatusHandler(final JobService svc) {
    this.svc = svc;
  }

  @Override
  public void handle(final RoutingContext ctx) {
    final String id = ctx.pathParam("id");
    if (id == null || id.isBlank()) {
      ctx.response().setStatusCode(400).putHeader("content-type", "application/json")
          .end("{\"error\":\"missing_id\"}");
      return;
    }
    final Optional<JobRecord> rec = svc.get(id);
    if (rec.isEmpty()) {
      ctx.response().setStatusCode(404).putHeader("content-type", "application/json")
          .end("{\"error\":\"not_found\"}");
      return;
    }
    ctx.response()
        .putHeader("content-type", "application/json")
        .end(rec.get().toJson().encode());
  }
}
