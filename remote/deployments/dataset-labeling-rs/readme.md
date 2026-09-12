# dd-dataset-labeling

Rust dataset labeling pipeline for the AI/ML + big-data runtime.

This service is the control plane for producing labeled training data. It registers datasets and
their label-class schema, creates labeling task items, accepts human and model annotations, applies
weak-supervision labeling functions (keyword rules), aggregates annotations into majority-vote gold
labels with inter-annotator agreement, exports gold datasets, publishes NATS label/result events,
and emits Spark/Airflow training-data materialization job intents for downstream big-data workers.

It keeps a bounded in-process store for the first deployable slice; durable training-data
materialization is handed off to the `big-data` / `ai-ml-platform` Spark and Airflow stacks.

## Labeling flow

1. `POST /datasets` - register a dataset with a label-class schema (empty schema = open vocabulary).
2. `POST /tasks` - add items (text and/or JSON payload) to label.
3. `POST /labels` - submit human/model annotations; re-labels from the same annotator are idempotent.
4. `POST /functions/apply` - apply weak-supervision labeling functions (keyword rules) as additional
   programmatic annotations.
5. `POST /aggregate` - majority-vote aggregation across annotators into gold labels, with class
   distribution and mean inter-annotator agreement; optionally hands off a pipeline job.
6. `GET /datasets/export` - export the gold-labeled dataset.

## Endpoints

- `GET /` - HTML operator home.
- `GET /descriptor` - service descriptor, NATS subjects, and endpoint map.
- `GET /schema` - request/response contract summary.
- `GET /example` - sample dataset, tasks, labels, function, and aggregate payloads.
- `GET /datasets` - authenticated dataset summary with item/annotation/gold counts.
- `POST /datasets` - authenticated dataset + label-schema registration.
- `POST /tasks` - authenticated labeling task item creation.
- `POST /labels` - authenticated human/model annotation submission.
- `POST /functions/apply` - authenticated weak-supervision labeling function application.
- `POST /aggregate` - authenticated majority-vote aggregation, agreement, and pipeline handoff.
- `GET /datasets/export` - authenticated gold-label dataset export (`datasetId`, `limit`, `goldOnly`).
- `POST /pipeline/jobs` - authenticated Spark/Airflow training-data job intent publication.
- `GET /healthz` - liveness probe.
- `GET /readyz` - readiness probe.
- `GET /metrics` - Prometheus text metrics.
- `GET /docs/api`, `GET /api/docs`, `GET /api/docs.json` - generated API docs.

All operator endpoints require `X-Server-Auth` or `Auth` to match `SERVER_AUTH_SECRET`, unless
`DATASET_LABELING_ALLOW_UNAUTHENTICATED=true` is set for local development.

## NATS subjects

Subject names live in `remote/libs/nats/subject-defs/schema/dataset-labeling.schema.json` and are
generated into every supported runtime.

- `dd.remote.dataset_labeling.task.requests` - queued dataset/task/label/function requests consumed
  by this service (queue group `dd-dataset-labeling`).
- `dd.remote.dataset_labeling.label.events` - per-label annotation events.
- `dd.remote.dataset_labeling.results` - weak-supervision and aggregation results.
- `dd.remote.dataset_labeling.pipeline.jobs` - Spark/Airflow training-data job intents.

## Pipeline handoff

The service publishes job envelopes rather than launching Spark or Airflow directly. A downstream
worker can subscribe to `dd.remote.dataset_labeling.pipeline.jobs` and map each envelope into:

- Spark jobs that materialize gold labels into Parquet training sets against
  `spark://spark-master.big-data.svc.cluster.local:7077`.
- Airflow DAG triggers in the `big-data` namespace.
- MinIO/object-storage writes under a sink such as `minio://datasets/gold/<dataset>`.

## Runtime env

- `PORT` - default `8138`.
- `SERVER_AUTH_SECRET` - operator/service auth secret.
- `NATS_URL` - NATS endpoint, normally `nats://dd-nats.messaging.svc.cluster.local:4222`.
- `DATASET_LABELING_ALLOW_UNAUTHENTICATED` - default `false`.
- `DATASET_LABELING_*_SUBJECT` / `DATASET_LABELING_QUEUE_GROUP` - optional overrides for generated
  NATS subjects.

Secrets belong in AWS Secrets Manager / Kubernetes secrets, not Git.
