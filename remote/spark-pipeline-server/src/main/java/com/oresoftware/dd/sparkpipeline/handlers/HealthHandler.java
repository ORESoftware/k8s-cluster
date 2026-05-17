package com.oresoftware.dd.sparkpipeline.handlers;

import com.oresoftware.dd.sparkpipeline.pipeline.JobService;
import io.vertx.core.Handler;
import io.vertx.ext.web.RoutingContext;

public final class HealthHandler {

  private HealthHandler() {
  }

  public static Handler<RoutingContext> liveness() {
    return ctx -> ctx.response()
        .putHeader("content-type", "application/json")
        .end("{\"status\":\"ok\"}");
  }

  public static Handler<RoutingContext> readiness(final JobService svc) {
    return ctx -> {
      if (svc.isReady()) {
        ctx.response()
            .putHeader("content-type", "application/json")
            .end("{\"status\":\"ready\"}");
      } else {
        ctx.response()
            .setStatusCode(503)
            .putHeader("content-type", "application/json")
            .end("{\"status\":\"draining\"}");
      }
    };
  }
}
