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

| Job kind                  | Combinator           | Notes                                                                 |
| ------------------------- | -------------------- | --------------------------------------------------------------------- |
| `INGEST_VALIDATE_PUBLISH` | `Asyncc.Waterfall`   | Each stage publishes a `(key, value)` consumed by downstream stages.  |
| `SPARK_SUBMIT`            | `Asyncc.Parallel`    | Pre-flight checks (cluster / jar / config) fan out, then submission.  |
| `SYNTHETIC_TEST`          | `Asyncc.Series`      | N no-op stages, used by integration tests.                            |

For server-wide concurrency control (so we never run more than
`PIPELINE_MAX_CONCURRENT` jobs at once across requests) we wrap submission in
an `org.ores.async.NeoQueue`, which is the non-blocking equivalent of a worker
pool.

## HTTP API

| Method | Path             | Purpose                              |
| ------ | ---------------- | ------------------------------------ |
| GET    | `/healthz`       | Liveness probe.                      |
| GET    | `/readyz`        | Readiness probe.                     |
| GET    | `/metrics`       | Prometheus scrape endpoint.          |
| POST   | `/v1/jobs`       | Submit a new pipeline job.           |
| GET    | `/v1/jobs`       | List all jobs known to this server.  |
| GET    | `/v1/jobs/{id}`  | Fetch a single job's state.          |

### Submit example

```bash
curl -s -X POST http://localhost:8085/v1/jobs \
  -H 'content-type: application/json' \
  -d '{"kind":"SYNTHETIC_TEST","params":{"stages":5}}'
```

## Environment variables

| Var                          | Default     | Description                                  |
| ---------------------------- | ----------- | -------------------------------------------- |
| `HTTP_HOST`                  | `0.0.0.0`   | Bind address.                                |
| `HTTP_PORT`                  | `8085`      | Bind port.                                   |
| `PIPELINE_MAX_CONCURRENT`    | `4`         | NeoQueue concurrency (server-wide).          |
| `JAVA_OPTS`                  | G1, 70% RAM | Standard JVM tuning.                         |

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
