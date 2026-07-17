# dd-knowledge-graph-builder

Rust knowledge-graph construction and query service for the AI/ML + big-data runtime.

This service turns ingested records, entity/relation triples, and free text into a bounded
in-process knowledge graph. It deduplicates entities, accumulates relation weights, answers
neighbor/subgraph and shortest-path queries, ranks entities by degree centrality, publishes NATS
change/result events, and emits Spark/Airflow graph-analytics job intents for downstream big-data
workers (PageRank, community detection, embeddings).

It is the control plane for graph construction, not a distributed graph database. Durable
large-scale analytics are handed off to the `big-data` / `ai-ml-platform` Spark and Airflow stacks.

## Endpoints

- `GET /` - HTML operator home.
- `GET /descriptor` - service descriptor, NATS subjects, and endpoint map.
- `GET /schema` - request/response contract summary.
- `GET /example` - sample upsert, extract, query, and path payloads.
- `GET /graph/stats` - authenticated node/edge counts, type/relation histograms, degree stats.
- `GET /graph/export` - authenticated node-link graph export.
- `POST /graph/upsert` - authenticated node/edge upsert (auto-creates referenced endpoints).
- `POST /graph/extract` - authenticated entity/relation extraction from records, with optional
  co-occurrence edge construction and naive capitalized-phrase extraction when no entities are given.
- `POST /graph/query` - authenticated breadth-first subgraph around a seed entity.
- `POST /graph/paths` - authenticated unweighted shortest path between two entities.
- `POST /graph/centrality` - authenticated degree/weighted-degree centrality ranking.
- `POST /pipeline/jobs` - authenticated Spark/Airflow graph-analytics job intent publication.
- `GET /healthz` - liveness probe.
- `GET /readyz` - readiness probe.
- `GET /metrics` - Prometheus text metrics.
- `GET /docs/api`, `GET /api/docs`, `GET /api/docs.json` - generated API docs.

All operator endpoints require `X-Server-Auth` or `Auth` to match `SERVER_AUTH_SECRET`, unless
`KNOWLEDGE_GRAPH_ALLOW_UNAUTHENTICATED=true` is set for local development.

## NATS subjects

Subject names live in `remote/libs/nats/subject-defs/schema/knowledge-graph.schema.json` and are
generated into every supported runtime.

- `dd.remote.knowledge_graph.build.requests` - queued upsert/extract requests consumed by this
  service (queue group `dd-knowledge-graph-builder`).
- `dd.remote.knowledge_graph.updates` - incremental graph mutation change feed.
- `dd.remote.knowledge_graph.results` - query, path, and centrality results.
- `dd.remote.knowledge_graph.pipeline.jobs` - Spark/Airflow graph-analytics job intents.

## Pipeline handoff

The service publishes job envelopes rather than launching Spark or Airflow directly. A downstream
worker can subscribe to `dd.remote.knowledge_graph.pipeline.jobs` and map each envelope into:

- Spark GraphX/GraphFrames analytics against `spark://spark-master.big-data.svc.cluster.local:7077`.
- Airflow DAG triggers in the `big-data` namespace.
- MinIO/object-storage writes under a sink such as `minio://knowledge-graph/exports/<graph>`.

## Runtime env

- `PORT` - default `8137`.
- `SERVER_AUTH_SECRET` - operator/service auth secret.
- `NATS_URL` - NATS endpoint, normally `nats://dd-nats.messaging.svc.cluster.local:4222`.
- `KNOWLEDGE_GRAPH_ALLOW_UNAUTHENTICATED` - default `false`.
- `KNOWLEDGE_GRAPH_*_SUBJECT` / `KNOWLEDGE_GRAPH_QUEUE_GROUP` - optional overrides for generated
  NATS subjects.

Secrets belong in AWS Secrets Manager / Kubernetes secrets, not Git.
