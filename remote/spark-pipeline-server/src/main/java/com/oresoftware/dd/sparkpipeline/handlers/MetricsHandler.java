package com.oresoftware.dd.sparkpipeline.handlers;

import io.micrometer.prometheus.PrometheusMeterRegistry;
import io.vertx.core.Handler;
import io.vertx.ext.web.RoutingContext;
import io.vertx.micrometer.backends.BackendRegistries;

public final class MetricsHandler {

  private MetricsHandler() {
  }

  public static Handler<RoutingContext> create() {
    return ctx -> {
      final var registry = BackendRegistries.getDefaultNow();
      if (registry instanceof PrometheusMeterRegistry prom) {
        ctx.response()
            .putHeader("content-type", "text/plain; version=0.0.4; charset=utf-8")
            .end(prom.scrape());
      } else {
        ctx.response()
            .setStatusCode(503)
            .putHeader("content-type", "text/plain")
            .end("prometheus registry unavailable\n");
      }
    };
  }
}
