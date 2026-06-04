# `remote/argocd/ai-ml-platform`

GitOps seed layer for the remote AI/ML data platform.

This bundle installs the lightweight always-on pieces:

- `ai-ml` namespace
- `dd-ai-ml-tool-catalog` ConfigMap describing the selected open-source stack
- `dd-ai-ml-data-contracts` ConfigMap with raw telemetry, MDP telemetry, and subject-map contracts
- `airbyte`, `kafka`, and `spark` namespaces with restricted Pod Security audit/warn labels
- `dd-ai-ml-pipeline`, a Python3 online feature pipeline that bridges telemetry into the existing
  Rust MDP/POMDP/RL optimizer
- narrow `ExternalSecret` projections that mirror only `SERVER_AUTH_SECRET` and `RDS_DATABASE_URL`
  into the `ai-ml` namespace for the Python and Spark pipeline services
- `ExternalSecret` entries that project `dd/remote-dev/ai-ml-platform-secrets` into the chart
  secrets consumed by Airbyte auth, Airflow, Dagster, MLflow, and Qdrant
- a secret-backed Airbyte Postgres StatefulSet plus S3 credential secret so the Airbyte chart does
  not deploy its internal Postgres or MinIO default-credential paths
- a small `dd-mlflow-artifacts` PVC for the MLflow chart's local artifact root
- a locked-down ServiceAccount and NetworkPolicy for the Python pipeline
- ingress-only namespace boundary NetworkPolicies for `ai-ml`, `airbyte`, `kafka`, and `spark`

Heavier platform tools are kept as separate Argo CD `Application` manifests in
`remote/argocd/apps/` so they can be applied intentionally:

| Category | Tool | Cluster entry |
| --- | --- | --- |
| Orchestration | Dagster | `dd-dagster.application.yaml` |
| Enterprise orchestration | Airflow | `dd-airflow.application.yaml` |
| ML tracking | MLflow | `dd-mlflow.application.yaml` |
| Data transformation | dbt | workflow image dependency |
| Streaming | Kafka | `dd-kafka-strimzi.application.yaml` |
| Distributed compute | Spark | `dd-spark-operator.application.yaml` |
| ML workflows | Metaflow | workflow image dependency |
| LLM pipelines | LlamaIndex | service/workflow image dependency |
| Vector DB | Qdrant | `dd-qdrant.application.yaml` |
| Data ingestion | Airbyte | `dd-airbyte.application.yaml` with external DB/S3 chart values |

Apply `remote/argocd/apps/dd-ai-ml-platform.appproject.yaml` before syncing these optional apps.
The AppProject restricts them to the expected chart/git repos and the `ai-ml`, `airbyte`, `kafka`,
and `spark` namespaces. Its cluster-resource allowlist is limited to Namespace, CRDs,
and operator ClusterRole/ClusterRoleBinding resources required by the Spark and Strimzi charts.
Its namespaced-resource allowlist is also explicit, covering only the workload, service, storage,
RBAC, ExternalSecret, NetworkPolicy, and hook Pod/Job kinds rendered by the current platform apps.
The optional chart apps and Spark pipeline app do not use Argo CD `CreateNamespace=true`; sync the
seed layer first so the Git-owned Namespace labels are present before those apps deploy.

The intended data path is:

```text
Airbyte/Kafka/NATS/HTTP telemetry
  -> dbt/Spark/Dagster/Airflow jobs when batch or warehouse work is needed
  -> dd-ai-ml-pipeline for online features and risk/anomaly scoring
  -> MLflow for model lineage as models become trained artifacts
  -> Qdrant/LlamaIndex for vector and LLM retrieval workloads
  -> dd-mdp-optimizer for policy selection
```

The pipeline pod does not mount a Kubernetes API token, runs with a read-only root filesystem, reads
the repo hostPath as read-only, and only allows egress to kube-dns plus the NATS client port in the
`messaging` namespace when a NetworkPolicy-capable CNI is installed.

The namespace boundary NetworkPolicies are ingress-only. They keep same-namespace traffic open,
allow the expected platform namespaces to call into Airbyte/Kafka/Spark, and allow the
`observability` namespace to scrape, without blocking egress to S3, registries, the Kubernetes API,
or external data systems. Inbound traffic to `ai-ml` stays limited to same-namespace callers,
`default`, and `observability`; this avoids widening the pipeline-specific ingress policies through
an additive namespace-wide rule.
The chart values also request runtime-default seccomp profiles where the current charts expose
security-context knobs, and the Spark pipeline server runs without an API token as a non-root pod.

`dd/remote-dev/ai-ml-platform-secrets` must exist before syncing the optional chart applications
that depend on it. Expected JSON keys:

- `AIRBYTE_JWT_SIGNATURE_SECRET`
- `AIRBYTE_INSTANCE_ADMIN_PASSWORD`
- `AIRBYTE_INSTANCE_ADMIN_CLIENT_ID`
- `AIRBYTE_INSTANCE_ADMIN_CLIENT_SECRET`
- `AIRBYTE_DATAPLANE_CLIENT_ID`
- `AIRBYTE_DATAPLANE_CLIENT_SECRET`
- `AIRBYTE_DATABASE_USER`
- `AIRBYTE_DATABASE_PASSWORD`
- `AIRBYTE_S3_ACCESS_KEY_ID`
- `AIRBYTE_S3_SECRET_ACCESS_KEY`
- `AIRFLOW_FERNET_KEY`
- `AIRFLOW_API_SECRET_KEY`
- `AIRFLOW_JWT_SECRET`
- `AIRFLOW_WEBSERVER_SECRET_KEY`
- `AIRFLOW_POSTGRES_PASSWORD`
- `DAGSTER_POSTGRES_PASSWORD`
- `MLFLOW_ADMIN_USERNAME`
- `MLFLOW_ADMIN_PASSWORD`
- `MLFLOW_POSTGRES_USER`
- `MLFLOW_POSTGRES_PASSWORD`
- `QDRANT_API_KEY`
- `QDRANT_READ_ONLY_API_KEY`

Airbyte's chart is configured with `postgresql.enabled=false`, `global.database.type=external`, and
`global.storage.type=s3`. The external database endpoint is the `dd-airbyte-postgresql` StatefulSet
in the `airbyte` namespace, and all DB/S3 credentials come from External Secrets. The S3 bucket name
is `dd-remote-dev-airbyte` in `us-east-1`; create or verify that bucket before syncing Airbyte.
