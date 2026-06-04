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
[`remote/libs/pg-defs/generated/jvm/jooq/src/main/java/dd/pgdefs/jooq/Tables.java`](../../libs/pg-defs/generated/jvm/jooq/src/main/java/dd/pgdefs/jooq/Tables.java),
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
| GET    | `/docs/api`, `/api/docs` | Generated API docs HTML.                    |
| GET    | `/api/docs.json` | Generated machine-readable API docs.                 |
| POST   | `/v1/jobs`       | Submit a new pipeline job. Requires `X-Server-Auth` / `Auth`. |
| GET    | `/v1/jobs`       | List all jobs known to this server. Requires `X-Server-Auth` / `Auth`. |
| GET    | `/v1/jobs/{id}`  | Fetch a single job's state. Requires `X-Server-Auth` / `Auth`. |
| GET    | `/v1/repos`      | List `known_git_repos` rows. Requires Postgres and `X-Server-Auth` / `Auth`. |

Route docs are generated from `MainVerticle.java` by `remote/tools/generate-api-docs.mjs`.
Run `pnpm --dir remote/tests run generate:api-docs` after changing routes and
`pnpm --dir remote/tests run check:api-docs` in CI-style validation.

### Submit example

```bash
curl -s -X POST http://localhost:8085/v1/jobs \
  -H "X-Server-Auth: $SERVER_AUTH_SECRET" \
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
| `SERVER_AUTH_SECRET`      | _(unset)_   | Shared server secret required by all `/v1/*` routes unless local unauthenticated mode is enabled. |
| `SPARK_PIPELINE_ALLOW_UNAUTHENTICATED` | `false` | Set to `true` only for local smoke tests without a shared secret. |
| `RDS_DATABASE_URL`        | _(unset)_   | Postgres URL (libpq or JDBC). If unset, DB endpoints return 503.  |
| `RDS_DATABASE_USER`       | _(from URL)_| Override Hikari user when URL has no embedded credentials.        |
| `RDS_DATABASE_PASSWORD`   | _(from URL)_| Override Hikari password when URL has no embedded credentials.    |
| `PG_POOL_SIZE`            | `8`         | HikariCP `maximumPoolSize`.                                       |

## Local build

```bash
cd remote/deployments/spark-pipeline-server
mvn -B -DskipTests package
java -jar target/dd-spark-pipeline-server.jar
```

## Container image

The EC2 Kubernetes overlay runs the prebuilt image
`docker.io/library/dd-spark-pipeline-server:dev`. Build it from the k8s-cluster repo root so the
Dockerfile can include the generated JVM pg-defs sources and generated API docs:

```bash
docker build -f remote/deployments/spark-pipeline-server/Dockerfile \
  -t docker.io/library/dd-spark-pipeline-server:dev .
```

On the EC2 node/containerd path, build the first-party AI/ML images in one pass before syncing
Argo CD:

```bash
bash remote/tools/build-ai-ml-platform-images.sh
```

Or build this service directly with the same tag in the `k8s.io` namespace:

```bash
nerdctl -n k8s.io build -f remote/deployments/spark-pipeline-server/Dockerfile \
  -t docker.io/library/dd-spark-pipeline-server:dev .
```

## Kubernetes layout

* `k8s/ec2/dd-spark-pipeline-server.deployment.yaml` — main EC2 Deployment.
* `k8s/ec2/dd-spark-pipeline-server.service.yaml` — ClusterIP Service on 8085.
* `k8s/ec2/dd-spark-pipeline-server.networkpolicy.yaml` — authenticated ingress plus DNS and
  private-CIDR Postgres egress.
* `k8s/ec2/dd-spark-pipeline-server.pdb.yaml` — `minAvailable: 1` disruption guard.
* `k8s/kustomization.yaml` — local kustomize entry point.
* `k8s/ec2/kustomization.yaml` — EC2 overlay consumed by Argo CD via
  `remote/argocd/apps/dd-spark-pipeline-server.application.yaml`.

The EC2 deployment runs in the `ai-ml` namespace from the prebuilt shaded-jar image instead of
mounting the repo with `hostPath` or fetching Maven dependencies at runtime. Rollouts use
`maxUnavailable: 0` plus the PDB so an ordinary deploy does not intentionally drop the API to zero
ready pods. The NetworkPolicy allows DNS for service/RDS resolution and keeps optional TCP 5432
Postgres traffic on private CIDRs only. The optional `RDS_DATABASE_URL` key is read from
`dd-remote-rest-api-secrets`, mirrored into `ai-ml` by the AI/ML seed layer, so the manifest reuses
the existing External Secrets bridge instead of depending on an uncreated service-specific secret.
