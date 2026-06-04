package com.oresoftware.dd.selenium.handlers;

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
}
