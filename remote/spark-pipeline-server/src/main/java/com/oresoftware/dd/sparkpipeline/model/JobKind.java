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
  SYNTHETIC_TEST
}
