# dd-public-data-server

Rust public-data ingestion and analysis service for the remote runtime.

This service is the control plane for collecting public/government/research data, not a browser
farm. It accepts normalized records and provider webhooks, delegates page scraping to
`dd-web-scraper`, keeps a bounded in-process evidence ledger for the first deployable slice,
publishes NATS results, and emits Spark/Airflow pipeline job intents for downstream big-data
workers.

## Source catalog

The built-in catalog covers the sources named for the first deployment:

- Data.gov
- Science.gov
- PubMed
- state libraries
- PLOS
- ProPublica
- Cambridge analytics / Cambridge public research sources
- SBIR.gov
- Pew Research Center

`GET /sources` returns the current catalog with default scraper strategy hints.

## Endpoints

- `GET /` - HTML operator home.
- `GET /descriptor` - service descriptor, source catalog, NATS subjects, and endpoint map.
- `GET /sources` - public-data source catalog.
- `GET /schema` - request/response contract summary.
- `GET /example` - sample ingest, scrape, and grant-match payloads.
- `GET /datasets` - authenticated in-memory dataset summary.
- `GET /jobs` - authenticated pipeline job ledger.
- `GET /healthz` - liveness probe.
- `GET /readyz` - readiness probe.
- `GET /metrics` - Prometheus text metrics.
- `POST /webhooks/ingest` - provider webhook receipt and optional record normalization.
- `POST /ingest` - authenticated normalized public-data record ingestion.
- `POST /scrape` - authenticated scrape orchestration through `dd-web-scraper`.
- `POST /grants/match` - authenticated grant-opportunity ranking.
- `POST /analysis/trends` - authenticated trend summaries plus graph data.
- `POST /analysis/correlations` - authenticated pairwise metric correlations.
- `POST /briefs/white-paper` - authenticated evidence-brief markdown plus graph/model data.
- `POST /pipeline/jobs` - authenticated Spark/Airflow pipeline job intent publication.
- `GET /docs/api`, `GET /api/docs`, `GET /api/docs.json` - generated API docs.

All operator endpoints require `X-Server-Auth` or `Auth` to match `SERVER_AUTH_SECRET`, unless
`PUBLIC_DATA_ALLOW_UNAUTHENTICATED=true` is set for local development. External webhook callers use
`X-Public-Data-Webhook-Secret` or `X-Webhook-Secret` when `PUBLIC_DATA_WEBHOOK_SECRET` is configured.
The gateway intentionally does not inject `X-Server-Auth` on `/public-data/webhooks/ingest`.

## NATS subjects

Subject names live in `remote/libs/nats/subject-defs/schema/public-data.schema.json` and are
generated into every supported runtime.

- `dd.remote.public_data.ingest.requests` - queued ingest/scrape requests consumed by this service.
- `dd.remote.public_data.ingest.results` - normalized ingest, scrape, and webhook results.
- `dd.remote.public_data.webhooks.events` - webhook receipt audit events.
- `dd.remote.public_data.pipeline.jobs` - Spark/Airflow pipeline job intents.
- `dd.remote.public_data.analysis.results` - grants, trend/correlation, graph, model, and brief results.

## Pipeline handoff

The first deployment publishes job envelopes rather than launching Spark or Airflow directly. A
future worker can subscribe to `dd.remote.public_data.pipeline.jobs` and map each envelope into:

- Spark ETL against `spark://spark-master.big-data.svc.cluster.local:7077`.
- Airflow DAG trigger against the `big-data` namespace Airflow deployment.
- MinIO/object-storage writes under a sink such as `minio://public-data/bronze/<dataset>`.

This keeps the Rust service focused on intake, normalization, evidence generation, and event
publication while the big-data platform owns durable batch execution.

## Runtime env

- `PORT` - default `8115`.
- `SERVER_AUTH_SECRET` - operator/service auth secret.
- `PUBLIC_DATA_WEBHOOK_SECRET` - optional external webhook shared secret.
- `PUBLIC_DATA_SCRAPER_BASE_URL` - default `http://dd-web-scraper.default.svc.cluster.local:8097`.
- `PUBLIC_DATA_SCRAPER_AUTH_SECRET` - defaults to `SERVER_AUTH_SECRET`.
- `NATS_URL` - NATS endpoint, normally `nats://dd-nats.messaging.svc.cluster.local:4222`.
- `PUBLIC_DATA_*_SUBJECT` - optional overrides for generated NATS subjects.

Secrets belong in AWS Secrets Manager / Kubernetes secrets, not Git.
