package com.oresoftware.dd.sparkpipeline.model;

/**
 * The kind of pipeline job this server can orchestrate.
 *
 * <p>The handlers in this codebase are JVM-ecosystem focused: each job kind ultimately translates
 * to running a process or RPC against a JVM-based system (Spark, Flink, Beam, JDBC, etc).
 */
public enum JobKind {
  /**
   * A Spark application submission. Typically translates to {@code spark-submit} against an
   * external Spark cluster or, in tests, a local Spark master URL.
   */
  SPARK_SUBMIT,

  /**
   * A multi-stage data validation + ingest pipeline. Runs three steps: SCHEMA_CHECK,
   * EXTRACT_LOAD, then PUBLISH_MANIFEST.
   */
  INGEST_VALIDATE_PUBLISH,

  /**
   * A generic synthetic job used by integration tests to exercise the flow-control plumbing
   * without depending on a real Spark cluster.
   */
  SYNTHETIC_TEST,

  /**
   * A synthetic but realistic-shaped pipeline that exercises ~10 different async.java
   * combinators in composition. The job models a "process a batch of records through a
   * multi-stage analytics pipeline" workflow:
   * <ol>
   *   <li>{@code Asyncc.Waterfall} drives the top-level keyed-map structure.</li>
   *   <li>{@code Asyncc.Times} produces a list of synthetic shard descriptors.</li>
   *   <li>{@code Asyncc.Map} enriches each shard, with an embedded
   *       {@code Asyncc.Parallel} fetching two upstream resources per shard.</li>
   *   <li>{@code Asyncc.FilterMap} drops shards that fail an async validation rule.</li>
   *   <li>{@code Asyncc.GroupBy} buckets the surviving shards by region.</li>
   *   <li>{@code Asyncc.Each} fan-out fire-and-forget side effects per bucket.</li>
   *   <li>{@code Asyncc.Race} between a fast-path and a careful-path scorer; first wins.</li>
   *   <li>{@code Asyncc.Reduce} folds per-bucket scores into an aggregate.</li>
   *   <li>{@code Asyncc.Inject} assembles the final manifest from named prior outputs.</li>
   *   <li>{@code NeoLock} serialises a shared "publication count" counter.</li>
   * </ol>
   * Combined with the {@code NeoQueue} that JobService uses for server-wide backpressure
   * that's 11 async.java combinators in one coherent job.
   *
   * <p>Purpose: exercise the composability of async.java end-to-end, surface any
   * cross-combinator integration issues, and give load-test harnesses something
   * non-trivial to drive.
   */
  COMPOSITION_DEMO
}
