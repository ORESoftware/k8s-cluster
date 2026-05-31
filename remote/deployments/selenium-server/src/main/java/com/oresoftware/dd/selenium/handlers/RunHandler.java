package com.oresoftware.dd.selenium.handlers;

import com.oresoftware.dd.selenium.Config;
import com.oresoftware.dd.selenium.run.ScenarioRunner;
import io.vertx.core.Handler;
import io.vertx.core.WorkerExecutor;
import io.vertx.core.json.JsonArray;
import io.vertx.core.json.JsonObject;
import io.vertx.ext.web.RoutingContext;
import org.slf4j.Logger;
import org.slf4j.LoggerFactory;

import java.time.Instant;
import java.util.UUID;

/**
 * {@code POST /run} — accept a bounded scenario, run it once against the Grid, and return the
 * structured result. Status codes mirror dd-browser-test-server: 400 invalid, 429 over the
 * concurrency cap, 422 when a step fails, 200 on success, 500 on a harness-level failure.
 */
public final class RunHandler implements Handler<RoutingContext> {

  private static final Logger log = LoggerFactory.getLogger(RunHandler.class);

  private final Config config;
  private final ScenarioRunner runner;
  private final WorkerExecutor executor;

  public RunHandler(final Config config, final ScenarioRunner runner, final WorkerExecutor executor) {
    this.config = config;
    this.runner = runner;
    this.executor = executor;
  }

  @Override
  public void handle(final RoutingContext ctx) {
    final JsonObject body;
    try {
      body = ctx.body().asJsonObject();
    } catch (Exception parseError) {
      replyError(ctx, 400, "invalid_json");
      return;
    }
    if (body == null) {
      replyError(ctx, 400, "missing_body");
      return;
    }

    final JsonArray steps = body.getJsonArray("steps");
    final String validationError = validate(steps);
    if (validationError != null) {
      replyError(ctx, 400, validationError);
      return;
    }

    if (!runner.tryAcquire()) {
      ctx.response()
          .setStatusCode(429)
          .putHeader("content-type", "application/json")
          .end(new JsonObject()
              .put("ok", false)
              .put("error", "selenium concurrency limit reached")
              .put("maxConcurrent", config.maxConcurrent)
              .encode());
      return;
    }

    final String rawRequestId = body.getString("requestId");
    final String requestId = (rawRequestId == null || rawRequestId.isBlank())
        ? UUID.randomUUID().toString()
        : rawRequestId;
    final String startedAtIso = Instant.now().toString();

    executor.<JsonObject>executeBlocking(promise -> {
      try {
        promise.complete(runner.run(body, requestId, startedAtIso));
      } catch (Exception runError) {
        promise.fail(runError);
      }
    }, false, ar -> {
      runner.release();
      if (ar.succeeded()) {
        final JsonObject result = ar.result();
        final boolean ok = result.getBoolean("ok", false);
        ctx.response()
            .setStatusCode(ok ? 200 : 422)
            .putHeader("content-type", "application/json")
            .end(result.encode());
      } else {
        // ScenarioRunner already converts scenario failures into ok=false results, so reaching
        // here means the worker dispatch itself failed.
        log.error("selenium run dispatch failed for request {}", requestId, ar.cause());
        ctx.response()
            .setStatusCode(500)
            .putHeader("content-type", "application/json")
            .end(new JsonObject()
                .put("ok", false)
                .put("requestId", requestId)
                .put("tool", "selenium")
                .put("error", ar.cause() == null ? "internal_error" : ar.cause().getMessage())
                .encode());
      }
    });
  }

  private String validate(final JsonArray steps) {
    if (steps == null || steps.isEmpty()) {
      return "steps_required";
    }
    if (steps.size() > config.maxSteps) {
      return "too_many_steps";
    }
    for (int i = 0; i < steps.size(); i++) {
      final Object step = steps.getValue(i);
      if (!(step instanceof JsonObject)) {
        return "invalid_step";
      }
      final String action = ((JsonObject) step).getString("action");
      if (action == null || action.isBlank()) {
        return "step_missing_action";
      }
    }
    return null;
  }

  private static void replyError(final RoutingContext ctx, final int code, final String error) {
    ctx.response()
        .setStatusCode(code)
        .putHeader("content-type", "application/json")
        .end(new JsonObject().put("ok", false).put("error", error).encode());
  }
}
