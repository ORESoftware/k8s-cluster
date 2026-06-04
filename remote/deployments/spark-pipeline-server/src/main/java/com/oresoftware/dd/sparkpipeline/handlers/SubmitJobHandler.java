package com.oresoftware.dd.sparkpipeline.handlers;

import com.oresoftware.dd.sparkpipeline.model.JobKind;
import com.oresoftware.dd.sparkpipeline.model.JobRecord;
import com.oresoftware.dd.sparkpipeline.pipeline.JobService;
import io.vertx.core.Handler;
import io.vertx.core.json.JsonObject;
import io.vertx.ext.web.RoutingContext;
import org.slf4j.Logger;
import org.slf4j.LoggerFactory;

import java.util.Optional;

/**
 * {@code POST /v1/jobs} — accept a job request and enqueue it.
 *
 * <p>Body: {@code { "kind": "SPARK_SUBMIT" | "INGEST_VALIDATE_PUBLISH" | "SYNTHETIC_TEST",
 * "params": { ... } }}.
 *
 * <p>Response (202 Accepted): the initial {@link JobRecord} serialized to JSON. The client
 * should subsequently poll {@code GET /v1/jobs/{id}} for status.
 */
public final class SubmitJobHandler implements Handler<RoutingContext> {

  private static final Logger log = LoggerFactory.getLogger(SubmitJobHandler.class);

  private final JobService svc;

  public SubmitJobHandler(final JobService svc) {
    this.svc = svc;
  }

  @Override
  public void handle(final RoutingContext ctx) {

    final JsonObject body;
    try {
      body = ctx.body().asJsonObject();
    } catch (Exception e) {
      replyError(ctx, 400, "invalid_json");
      return;
    }
    if (body == null) {
      replyError(ctx, 400, "missing_body");
      return;
    }

    final String kindStr = body.getString("kind");
    if (kindStr == null) {
      replyError(ctx, 400, "missing_kind");
      return;
    }

    final JobKind kind;
    try {
      kind = JobKind.valueOf(kindStr);
    } catch (IllegalArgumentException iae) {
      ctx.response().setStatusCode(400).putHeader("content-type", "application/json")
          .end("{\"error\":\"unknown_kind\",\"kind\":\"" + kindStr + "\"}");
      return;
    }

    final JsonObject params = body.getJsonObject("params", new JsonObject());

    final Optional<JobRecord> rec = svc.enqueue(kind, params);
    if (rec.isEmpty()) {
      replyError(ctx, 503, "service_draining");
      return;
    }

    log.info("enqueued job {} kind={}", rec.get().getId(), kind);
    ctx.response()
        .setStatusCode(202)
        .putHeader("content-type", "application/json")
        .end(rec.get().toJson().encode());
  }

  private static void replyError(final RoutingContext ctx, final int code, final String err) {
    ctx.response()
        .setStatusCode(code)
        .putHeader("content-type", "application/json")
        .end("{\"error\":\"" + err + "\"}");
  }
}
