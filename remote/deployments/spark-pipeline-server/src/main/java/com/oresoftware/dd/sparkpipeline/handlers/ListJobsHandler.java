package com.oresoftware.dd.sparkpipeline.handlers;

import com.oresoftware.dd.sparkpipeline.pipeline.JobService;
import io.vertx.core.Handler;
import io.vertx.core.json.JsonArray;
import io.vertx.core.json.JsonObject;
import io.vertx.ext.web.RoutingContext;

/**
 * {@code GET /v1/jobs} — list all jobs known to this server.
 */
public final class ListJobsHandler implements Handler<RoutingContext> {

  private final JobService svc;

  public ListJobsHandler(final JobService svc) {
    this.svc = svc;
  }

  @Override
  public void handle(final RoutingContext ctx) {
    final JsonArray arr = new JsonArray();
    svc.list().forEach(r -> arr.add(r.toJson()));
    ctx.response()
        .putHeader("content-type", "application/json")
        .end(new JsonObject().put("jobs", arr).encode());
  }
}
