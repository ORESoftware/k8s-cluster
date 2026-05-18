# dd-spark-pipeline-server

A Java / Vert.x HTTP server that orchestrates Spark and other JVM-ecosystem data
pipeline jobs. It is the Java-ecosystem-centric counterpart to the Python-based
`dd-ai-ml-pipeline` and the Gleam-based `dd-gleamlang-server` in this repo.

## Why a Java service?

Several of our data sinks and processing engines live in the JVM ecosystem
(Spark, Flink, Beam, JDBC drivers for Postgres / ClickHouse, Kafka clients,
Avro / Parquet codecs). Running orchestration code in-process with those
libraries — instead of shelling out from Python or Node — gets us:

* type-safe access to `SparkLauncher`, `SparkSession` and JDBC connection
  pools without inter-process JSON serialization,
* a non-blocking HTTP front end via Vert.x for high job-submission fan-out,
* fine-grained backpressure on the worker pool via `async-java/async.java`.

## Flow control: async-java/async.java

All multi-stage flow control inside a single job uses
[`org.ores.async.Asyncc`](https://github.com/async-java/async.java), the Java
port of the node `async` library. Different job kinds use different
combinators:

The `pom.xml` is pinned to the latest audited release,
`com.github.async-java:async.java:v0.2.9`, through JitPack. As of the
2026-05-18 audit, JitPack serves that tag and Maven Central does not yet expose
the future `io.github.async-java:async-java:0.2.9` coordinate.

| Job kind                  | Combinator           | Notes                                                                 |
| ------------------------- | -------------------- | --------------------------------------------------------------------- |
| `INGEST_VALIDATE_PUBLISH` | `Asyncc.Waterfall`   | Each stage publishes a `(key, value)` consumed by downstream stages.  |
| `SPARK_SUBMIT`            | `Asyncc.Parallel`    | Pre-flight checks (cluster / jar / config) fan out, then submission.  |
| `SYNTHETIC_TEST`          | `Asyncc.Series`      | N no-op stages, used by integration tests.                            |

For server-wide concurrency control (so we never run more than
`PIPELINE_MAX_CONCURRENT` jobs at once across requests) we wrap submission in
an `org.ores.async.NeoQueue`, which is the non-blocking equivalent of a worker
pool.

## Postgres access via the pg-defs jOOQ Tables

The service uses [jOOQ](https://www.jooq.org/) for type-safe SQL against the
canonical `dd_pg_defs` database. Column and table references come from the
generated bindings at
[`remote/libs/pg-defs/generated/jvm/jooq/src/main/java/dd/pgdefs/jooq/Tables.java`](../libs/pg-defs/generated/jvm/jooq/src/main/java/dd/pgdefs/jooq/Tables.java),
which is added to this module's source set via `build-helper-maven-plugin` in
`pom.xml` (no separate Maven install needed — the source is compiled in-place,
so regenerating `pg-defs` automatically refreshes this service's view).

Schema changes therefore fail the build instead of crashing at runtime: drop
a column from `schema/schema.sql`, regenerate, and the next `mvn compile`
points at the broken `Tables.KNOWN_GIT_REPOS_FOO` reference.

The runtime wiring lives in `db/PgDb.java`:

* HikariCP pool, sized via `PG_POOL_SIZE` (default 8).
* Connection string from `RDS_DATABASE_URL` — accepts either JDBC
  (`jdbc:postgresql://...`) or libpq (`postgres://user:pw@host/db`) form,
  matching what the other dd-* services consume.
* Optional: if `RDS_DATABASE_URL` is unset the pool is never opened, the
  service still boots, and DB-backed endpoints respond `503`.

### Postgres-backed endpoints

| Method | Path        | Backed by                                       |
| ------ | ----------- | ----------------------------------------------- |
| GET    | `/v1/repos` | `select * from known_git_repos` (non-soft-deleted, newest first, limit 500). |

### Postgres-aware jobs

`SPARK_SUBMIT` accepts a `params.repoId` UUID. When set, the parallel
precheck fan-out adds a 4th task that does a typed jOOQ lookup against
`Tables.KNOWN_GIT_REPOS_*` to resolve the repo URL + default branch and
appends them to the job's stage log. Missing rows / invalid UUIDs degrade
to `repo_unresolved` without failing the job.

## HTTP API

| Method | Path             | Purpose                                              |
| ------ | ---------------- | ---------------------------------------------------- |
| GET    | `/healthz`       | Liveness probe.                                      |
| GET    | `/readyz`        | Readiness probe.                                     |
| GET    | `/metrics`       | Prometheus scrape endpoint.                          |
| POST   | `/v1/jobs`       | Submit a new pipeline job.                           |
| GET    | `/v1/jobs`       | List all jobs known to this server.                  |
| GET    | `/v1/jobs/{id}`  | Fetch a single job's state.                          |
| GET    | `/v1/repos`      | List `known_git_repos` rows (requires Postgres).     |

### Submit example

```bash
curl -s -X POST http://localhost:8085/v1/jobs \
  -H 'content-type: application/json' \
  -d '{"kind":"SYNTHETIC_TEST","params":{"stages":5}}'
```

## Environment variables

| Var                       | Default     | Description                                                       |
| ------------------------- | ----------- | ----------------------------------------------------------------- |
| `HTTP_HOST`               | `0.0.0.0`   | Bind address.                                                     |
| `HTTP_PORT`               | `8085`      | Bind port.                                                        |
| `PIPELINE_MAX_CONCURRENT` | `4`         | NeoQueue concurrency (server-wide).                               |
| `JAVA_OPTS`               | G1, 70% RAM | Standard JVM tuning.                                              |
| `RDS_DATABASE_URL`        | _(unset)_   | Postgres URL (libpq or JDBC). If unset, DB endpoints return 503.  |
| `RDS_DATABASE_USER`       | _(from URL)_| Override Hikari user when URL has no embedded credentials.        |
| `RDS_DATABASE_PASSWORD`   | _(from URL)_| Override Hikari password when URL has no embedded credentials.    |
| `PG_POOL_SIZE`            | `8`         | HikariCP `maximumPoolSize`.                                       |

## Local build

```bash
cd remote/spark-pipeline-server
mvn -B -DskipTests package
java -jar target/dd-spark-pipeline-server.jar
```

## Kubernetes layout

* `k8s/dd-spark-pipeline-server.deployment.yaml` — main Deployment.
* `k8s/dd-spark-pipeline-server.service.yaml` — ClusterIP Service on 8085.
* `k8s/kustomization.yaml` — minikube / local kustomize entry point.
* `k8s/ec2/kustomization.yaml` — EC2 overlay consumed by Argo CD via
  `remote/argocd/apps/dd-spark-pipeline-server.application.yaml`.

The EC2 deployment mounts the repo as a hostPath at `/opt/dd-next-1` and runs
`mvn package` once on container start before booting the shaded jar. The same
container image (`eclipse-temurin:17-jre`) is used in CI and runtime.
