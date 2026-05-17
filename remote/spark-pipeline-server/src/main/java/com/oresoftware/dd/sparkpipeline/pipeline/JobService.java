package com.oresoftware.dd.sparkpipeline.pipeline;

import com.oresoftware.dd.sparkpipeline.db.PgDb;
import com.oresoftware.dd.sparkpipeline.model.JobKind;
import com.oresoftware.dd.sparkpipeline.model.JobRecord;
import com.oresoftware.dd.sparkpipeline.model.JobState;
import dd.pgdefs.jooq.Tables;
import io.vertx.core.Future;
import io.vertx.core.Promise;
import io.vertx.core.Vertx;
import io.vertx.core.json.JsonObject;
import org.jooq.DSLContext;
import org.jooq.Record;
import org.ores.async.Asyncc;
import org.ores.async.NeoQueue;
import org.ores.async.NeoWaterfallI;
import org.slf4j.Logger;
import org.slf4j.LoggerFactory;

import java.time.Instant;
import java.util.ArrayList;
import java.util.Collection;
import java.util.Collections;
import java.util.HashMap;
import java.util.List;
import java.util.Map;
import java.util.Optional;
import java.util.UUID;
import java.util.concurrent.ConcurrentHashMap;

/**
 * Core orchestration service.
 *
 * <p>This class is intentionally a Plain Old Java service object (not a verticle). It is owned by
 * {@link com.oresoftware.dd.sparkpipeline.MainVerticle} and uses the Vert.x instance handed to it
 * to schedule blocking work and to communicate with handlers via {@link Future} chains.
 *
 * <p>For multi-stage flow control inside a single job we delegate to
 * {@link org.ores.async.Asyncc} from {@code async-java/async.java}:
 *
 * <ul>
 *   <li>{@link JobKind#INGEST_VALIDATE_PUBLISH} uses {@code Asyncc.Waterfall} so each stage can
 *       read keyed outputs published by the previous stages.</li>
 *   <li>{@link JobKind#SYNTHETIC_TEST} uses {@code Asyncc.Series} for a fixed list of stages.</li>
 *   <li>{@link JobKind#SPARK_SUBMIT} uses {@code Asyncc.Parallel} to drive a small fan-out of
 *       pre-flight checks before submitting.</li>
 * </ul>
 *
 * <p>For server-wide concurrency control (so that we never run more than {@code maxConcurrent}
 * jobs at once across all requests) we wrap submission in a {@link NeoQueue}, which is the
 * non-blocking equivalent of a worker pool.
 */
public final class JobService {

  private static final Logger log = LoggerFactory.getLogger(JobService.class);

  private final Vertx vertx;
  private final PgDb pg;
  private final Map<String, JobRecord> jobs = new ConcurrentHashMap<>();
  private final NeoQueue<JobRecord, JobRecord> queue;
  private volatile boolean shuttingDown = false;

  public JobService(final Vertx vertx, final PgDb pg) {
    this.vertx = vertx;
    this.pg = pg;
    final int maxConcurrent = parsePositiveIntEnv("PIPELINE_MAX_CONCURRENT", 4);
    this.queue = new NeoQueue<>(maxConcurrent, this::runJobOnQueue);
    log.info("JobService initialized with maxConcurrent={} pgConfigured={}",
        maxConcurrent, pg != null);
  }

  /**
   * Enqueue a new job and return the freshly-created {@link JobRecord} synchronously. The job
   * runs asynchronously through the {@link NeoQueue}; callers should poll {@link #get(String)}
   * for completion.
   *
   * @return the newly-created record, or {@link Optional#empty()} if the service is shutting down.
   */
  public Optional<JobRecord> enqueue(final JobKind kind, final JsonObject params) {
    if (shuttingDown) {
      return Optional.empty();
    }
    final JobRecord rec = new JobRecord(kind, params);
    jobs.put(rec.getId(), rec);
    rec.appendStage("queued");
    queue.push(new NeoQueue.Task<JobRecord, JobRecord>(rec), (err, finished) -> {
      if (err != null) {
        log.warn("job {} kind={} failed: {}", rec.getId(), rec.getKind(), err);
      } else {
        log.info("job {} kind={} succeeded", rec.getId(), rec.getKind());
      }
    });
    return Optional.of(rec);
  }

  /**
   * Enqueue a job and complete a {@link Future} when it finishes. Used by tests; production
   * callers should use {@link #enqueue(JobKind, JsonObject)} and poll.
   */
  public Future<JobRecord> submitAndAwait(final JobKind kind, final JsonObject params) {
    if (shuttingDown) {
      return Future.failedFuture("service shutting down");
    }
    final JobRecord rec = new JobRecord(kind, params);
    jobs.put(rec.getId(), rec);
    rec.appendStage("queued");
    final Promise<JobRecord> p = Promise.promise();
    queue.push(new NeoQueue.Task<JobRecord, JobRecord>(rec), (err, finished) -> {
      if (err != null) {
        p.fail(String.valueOf(err));
      } else {
        p.complete(finished);
      }
    });
    return p.future();
  }

  public Optional<JobRecord> get(final String id) {
    return Optional.ofNullable(jobs.get(id));
  }

  public Collection<JobRecord> list() {
    return Collections.unmodifiableCollection(jobs.values());
  }

  public boolean isReady() {
    return !shuttingDown;
  }

  public void shutdown() {
    shuttingDown = true;
  }

  /**
   * Body of the {@link NeoQueue} worker. Called by async.java's queue when a slot is available.
   * Any exception thrown synchronously is converted to a callback failure so the queue stays
   * healthy.
   */
  private void runJobOnQueue(final NeoQueue.Task<JobRecord, JobRecord> task,
                             final NeoQueue.IAsyncErrFirstCb<JobRecord> doneCb) {
    final JobRecord rec = task.getValue();
    rec.setState(JobState.RUNNING);
    rec.setStartedAt(Instant.now());
    rec.appendStage("started kind=" + rec.getKind());

    // We hop onto a Vert.x worker thread for the actual stage execution so any blocking JDBC /
    // RPC inside a stage doesn't pin async.java's queue executor thread.
    vertx.<JobRecord>executeBlocking(p -> runStages(rec, p), false)
        .onComplete(ar -> {
          if (ar.succeeded()) {
            rec.setState(JobState.SUCCEEDED);
            rec.setFinishedAt(Instant.now());
            rec.appendStage("succeeded");
            doneCb.done(null, rec);
          } else {
            rec.setState(JobState.FAILED);
            rec.setFinishedAt(Instant.now());
            rec.setErrorMessage(rootMessage(ar.cause()));
            rec.appendStage("failed: " + rec.getErrorMessage());
            doneCb.done(ar.cause(), rec);
          }
        });
  }

  /**
   * Run the per-kind stage list using the appropriate async.java combinator.
   */
  private void runStages(final JobRecord rec, final Promise<JobRecord> outcome) {
    switch (rec.getKind()) {
      case INGEST_VALIDATE_PUBLISH:
        runIngestWaterfall(rec, outcome);
        break;
      case SPARK_SUBMIT:
        runSparkSubmitParallel(rec, outcome);
        break;
      case SYNTHETIC_TEST:
      default:
        runSyntheticSeries(rec, outcome);
        break;
    }
  }

  // --- INGEST_VALIDATE_PUBLISH: Waterfall(SCHEMA_CHECK -> EXTRACT_LOAD -> PUBLISH_MANIFEST) ---
  //
  // Waterfall stages each emit a (key, value) pair via cb.done(err, key, value) and downstream
  // stages can read prior pairs via cb.get(key). The final callback receives a HashMap of all
  // key->value pairs the stages published.
  private void runIngestWaterfall(final JobRecord rec, final Promise<JobRecord> outcome) {

    final List<NeoWaterfallI.AsyncTask<String, Object>> stages = new ArrayList<>();

    stages.add(cb -> {
      rec.appendStage("schema_check");
      cb.done(null, "schemaOk", "true");
    });

    stages.add(cb -> {
      final String schemaOk = cb.get("schemaOk");
      rec.appendStage("extract_load schemaOk=" + schemaOk);
      cb.done(null, "rows", "100");
    });

    stages.add(cb -> {
      final String rows = cb.get("rows");
      rec.appendStage("publish_manifest rows=" + rows);
      cb.done(null, "manifest", "s3://example/manifest-" + rec.getId() + ".json");
    });

    Asyncc.Waterfall(stages, (err, results) -> {
      if (err != null) {
        outcome.fail(String.valueOf(err));
        return;
      }
      rec.setResult(new JsonObject(new HashMap<String, Object>(results)));
      outcome.complete(rec);
    });
  }

  // --- SPARK_SUBMIT: Parallel(precheck-cluster, precheck-jar, precheck-config, lookup-repo) ---
  //
  // The repo lookup uses the generated jOOQ Tables from remote/libs/pg-defs to fetch the
  // known_git_repos row identified by params.repoId. The Parallel combinator from async.java
  // fans out all prechecks concurrently; the jOOQ call runs inside the per-task lambda which
  // means it executes on the same Vert.x worker thread that drives the pipeline (we're already
  // inside executeBlocking from runJobOnQueue, so a synchronous JDBC call is safe here).
  private void runSparkSubmitParallel(final JobRecord rec, final Promise<JobRecord> outcome) {

    final List<Asyncc.AsyncTask<String, Object>> prechecks = new ArrayList<>();

    prechecks.add(cb -> {
      rec.appendStage("precheck.cluster");
      cb.done(null, "cluster_ok");
    });

    prechecks.add(cb -> {
      rec.appendStage("precheck.jar");
      cb.done(null, "jar_ok");
    });

    prechecks.add(cb -> {
      rec.appendStage("precheck.config");
      cb.done(null, "config_ok");
    });

    final String repoIdRaw = rec.getParams().getString("repoId");
    if (repoIdRaw != null && !repoIdRaw.isBlank()) {
      prechecks.add(cb -> {
        final String resolved = lookupRepo(rec, repoIdRaw);
        cb.done(null, resolved);
      });
    }

    Asyncc.Parallel(prechecks, (err, results) -> {
      if (err != null) {
        outcome.fail(String.valueOf(err));
        return;
      }
      rec.appendStage("spark_submit prechecks=" + results);
      // In a real deployment this is where we'd invoke spark-submit / SparkLauncher.
      rec.setResult(new JsonObject()
          .put("submission", "spark-app-" + rec.getId().substring(0, 8))
          .put("prechecks", results.toString()));
      outcome.complete(rec);
    });
  }

  /**
   * Resolve a {@code known_git_repos} row by its UUID using the jOOQ Tables generated by
   * pg-defs. Returns a human-readable summary that's appended to the job stage log; never
   * throws — the precheck always succeeds with either the resolved repo URL or a clear
   * "unresolved" reason so a missing-repo doesn't fail the whole Spark submission.
   */
  private String lookupRepo(final JobRecord rec, final String repoIdRaw) {
    if (pg == null) {
      rec.appendStage("precheck.repo[skipped:pg_not_configured]");
      return "repo_unresolved";
    }
    final Optional<DSLContext> dslOpt = pg.context();
    if (dslOpt.isEmpty()) {
      rec.appendStage("precheck.repo[skipped:pool_closed]");
      return "repo_unresolved";
    }

    final UUID repoId;
    try {
      repoId = UUID.fromString(repoIdRaw);
    } catch (IllegalArgumentException iae) {
      rec.appendStage("precheck.repo[invalid_uuid: " + repoIdRaw + "]");
      return "repo_unresolved";
    }

    try {
      final Record row = dslOpt.get()
          .select(
              Tables.KNOWN_GIT_REPOS_REPO_URL,
              Tables.KNOWN_GIT_REPOS_DEFAULT_BRANCH,
              Tables.KNOWN_GIT_REPOS_STATUS)
          .from(Tables.KNOWN_GIT_REPOS)
          .where(Tables.KNOWN_GIT_REPOS_ID.eq(repoId))
          .and(Tables.KNOWN_GIT_REPOS_IS_SOFT_DELETED.isFalse()
              .or(Tables.KNOWN_GIT_REPOS_IS_SOFT_DELETED.isNull()))
          .fetchOne();
      if (row == null) {
        rec.appendStage("precheck.repo[not_found:" + repoId + "]");
        return "repo_unresolved";
      }
      final String repoUrl = row.get(Tables.KNOWN_GIT_REPOS_REPO_URL);
      final String branch = row.get(Tables.KNOWN_GIT_REPOS_DEFAULT_BRANCH);
      final String status = row.get(Tables.KNOWN_GIT_REPOS_STATUS);
      rec.appendStage("precheck.repo[resolved url=" + repoUrl
          + " branch=" + branch + " status=" + status + "]");
      return "repo_ok:" + repoUrl + "@" + branch;
    } catch (Exception e) {
      log.warn("precheck.repo query failed for {}", repoId, e);
      rec.appendStage("precheck.repo[query_failed: " + e.getClass().getSimpleName() + "]");
      return "repo_unresolved";
    }
  }

  // --- SYNTHETIC_TEST: Series of N no-op stages, count from params or default 3 ---
  private void runSyntheticSeries(final JobRecord rec, final Promise<JobRecord> outcome) {

    final int count = Math.max(1, rec.getParams().getInteger("stages", 3));
    final List<Asyncc.AsyncTask<String, Object>> stages = new ArrayList<>(count);

    for (int i = 0; i < count; i++) {
      final int idx = i;
      stages.add(cb -> {
        rec.appendStage("synthetic.stage[" + idx + "]");
        cb.done(null, "stage-" + idx);
      });
    }

    Asyncc.Series(stages, (err, results) -> {
      if (err != null) {
        outcome.fail(String.valueOf(err));
        return;
      }
      rec.setResult(new JsonObject().put("stages", results.toString()));
      outcome.complete(rec);
    });
  }

  private static int parsePositiveIntEnv(final String name, final int fallback) {
    final String raw = System.getenv(name);
    if (raw == null || raw.isBlank()) {
      return fallback;
    }
    try {
      final int v = Integer.parseInt(raw.trim());
      return v > 0 ? v : fallback;
    } catch (NumberFormatException nfe) {
      return fallback;
    }
  }

  private static String rootMessage(final Throwable t) {
    if (t == null) {
      return "unknown error";
    }
    Throwable cur = t;
    while (cur.getCause() != null && cur.getCause() != cur) {
      cur = cur.getCause();
    }
    return cur.getClass().getSimpleName() + ": " + String.valueOf(cur.getMessage());
  }
}
