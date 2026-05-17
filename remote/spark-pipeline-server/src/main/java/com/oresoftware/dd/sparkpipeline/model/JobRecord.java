package com.oresoftware.dd.sparkpipeline.model;

import io.vertx.core.json.JsonArray;
import io.vertx.core.json.JsonObject;

import java.time.Instant;
import java.util.ArrayList;
import java.util.Collections;
import java.util.List;
import java.util.Objects;
import java.util.UUID;

/**
 * In-memory record of a single pipeline job. Mutable, intentionally — concurrent access is
 * serialized by {@link com.oresoftware.dd.sparkpipeline.pipeline.JobService} via Vert.x context
 * confinement and async.java flow control (no caller ever mutates a record off the owning
 * event-loop thread).
 */
public final class JobRecord {

  private final String id;
  private final JobKind kind;
  private final JsonObject params;
  private final Instant createdAt;

  private JobState state;
  private Instant startedAt;
  private Instant finishedAt;
  private String errorMessage;
  private final List<String> stageLog;
  private JsonObject result;

  public JobRecord(final JobKind kind, final JsonObject params) {
    this.id = UUID.randomUUID().toString();
    this.kind = Objects.requireNonNull(kind, "kind");
    this.params = params == null ? new JsonObject() : params.copy();
    this.createdAt = Instant.now();
    this.state = JobState.PENDING;
    this.stageLog = new ArrayList<>();
  }

  public String getId() {
    return id;
  }

  public JobKind getKind() {
    return kind;
  }

  public JsonObject getParams() {
    return params;
  }

  public JobState getState() {
    return state;
  }

  public void setState(final JobState state) {
    this.state = state;
  }

  public Instant getStartedAt() {
    return startedAt;
  }

  public void setStartedAt(final Instant startedAt) {
    this.startedAt = startedAt;
  }

  public Instant getFinishedAt() {
    return finishedAt;
  }

  public void setFinishedAt(final Instant finishedAt) {
    this.finishedAt = finishedAt;
  }

  public String getErrorMessage() {
    return errorMessage;
  }

  public void setErrorMessage(final String errorMessage) {
    this.errorMessage = errorMessage;
  }

  public List<String> getStageLog() {
    return Collections.unmodifiableList(stageLog);
  }

  public void appendStage(final String message) {
    stageLog.add(Instant.now() + " " + message);
  }

  public JsonObject getResult() {
    return result;
  }

  public void setResult(final JsonObject result) {
    this.result = result;
  }

  public JsonObject toJson() {
    return new JsonObject()
        .put("id", id)
        .put("kind", kind.name())
        .put("state", state.name())
        .put("params", params)
        .put("createdAt", createdAt.toString())
        .put("startedAt", startedAt == null ? null : startedAt.toString())
        .put("finishedAt", finishedAt == null ? null : finishedAt.toString())
        .put("errorMessage", errorMessage)
        .put("stageLog", new JsonArray(stageLog))
        .put("result", result);
  }
}
