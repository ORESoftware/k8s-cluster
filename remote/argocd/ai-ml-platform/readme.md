# `remote/argocd/ai-ml-platform`

GitOps seed layer for the remote AI/ML data platform.

This bundle installs the lightweight always-on pieces:

- `ai-ml` namespace
- `dd-ai-ml-tool-catalog` ConfigMap describing the selected open-source stack
- `dd-ai-ml-data-contracts` ConfigMap with raw telemetry, MDP telemetry, and subject-map contracts
- `airbyte`, `kafka`, and `spark` namespaces with baseline Pod Security enforcement plus
  restricted Pod Security audit/warn labels
- per-namespace `ResourceQuota` and `LimitRange` controls for `ai-ml`, `airbyte`, `kafka`, and
  `spark` so optional data tools have explicit dev-cluster budgets and default pod limits
- `dd-ai-ml-pipeline`, a Python3 online feature pipeline that bridges telemetry into the existing
  Rust MDP/POMDP/RL optimizer
- narrow `ExternalSecret` projections that mirror only `SERVER_AUTH_SECRET` and `RDS_DATABASE_URL`
  into the `ai-ml` namespace for the Python and Spark pipeline services
- `ExternalSecret` entries that project `dd/remote-dev/ai-ml-platform-secrets` into the chart
  secrets consumed by Airbyte auth, Airflow, Dagster, MLflow, and Qdrant
- Airflow chart values that keep the Airflow app containers and embedded Postgres secret-backed,
  resource-bounded, on no-token service accounts, behind the chart's primary ingress
  NetworkPolicy, and on read-only root filesystems with bounded writable temp, log, and `/dev/shm`
  volumes
- Dagster chart values that use a seed-layer Postgres StatefulSet instead of the chart's older
  bundled Postgres subchart, disable the chart-generated Secret, add DB client labels, run the
  daemon/webserver containers with read-only roots and bounded `/tmp`, and bound Dagster system/run
  pod resources
- MLflow chart values that use a seed-layer Postgres StatefulSet instead of the chart's bundled
  Postgres subchart, keep the app secret-backed, label it as the approved Postgres client, and keep
  the main container on a read-only root with bounded writable temp/artifact paths
- Qdrant chart values that use the verified unprivileged image variant, disable the root ownership
  init path, put storage and snapshots on PVCs, and make the non-root/read-only-root security
  context explicit
- secret-backed Airbyte, Dagster, and MLflow Postgres StatefulSets with bounded writable socket/temp
  volumes, digest-pinned Postgres images, read-only roots, and client-scoped NetworkPolicies, plus
  Airbyte S3 credential secret wiring so the Airbyte chart does not deploy its internal Postgres or
  MinIO default-credential paths
- Airbyte chart resource requests/limits for its web, server, worker, launcher, API, Temporal,
  cron, connector-builder, bootloader, and launched job pods
- Spark Operator chart values that use the chart's `operatorDeployment` path for explicit
  non-root pod/container security, read-only root filesystem, JVM temp redirected to the chart's
  writable logs volume, and bounded CPU/memory/ephemeral-storage
- Strimzi chart values that explicitly watch only the `kafka` namespace, bound `/tmp`, and pin
  operator CPU/memory plus read-only-root security settings
- a small `dd-mlflow-artifacts` PVC for the MLflow chart's local artifact root
- a locked-down ServiceAccount and NetworkPolicy for the Python pipeline
- PodDisruptionBudgets for the Python pipeline and the first-party Postgres StatefulSets so
  voluntary disruptions do not take the local data path down silently
- ingress-only namespace boundary NetworkPolicies for `ai-ml`, `airbyte`, `kafka`, and `spark`;
  the `ai-ml` and `airbyte` boundaries intentionally exclude the Python pipeline and externalized
  Postgres pods so their narrower workload-specific policies are effective. Externalized Postgres
  pods have client-label-only ingress and explicit `egress: []` policies.

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
RBAC, resource-control, ExternalSecret, NetworkPolicy, and hook Pod/Job kinds rendered by the
current platform apps. The AI/ML seed layer, optional chart apps, and Spark pipeline app do not use
Argo CD `CreateNamespace=true`; sync the seed layer first so the Git-owned Namespace labels are
present before those apps deploy. The Application resources encode that order for app-of-apps
syncs: AppProject wave `-20`, seed layer wave `-10`, Spark pipeline wave `5`, and optional Helm
apps wave `10`.
Airbyte, Airflow, Dagster, MLflow, Qdrant, Strimzi, Spark Operator, and the Spark pipeline server
apps are configured for automated prune/self-heal once their prerequisite secrets and storage exist.
Run `pnpm --dir remote/tests run audit:ai-ml-platform-helm -- --pull` to render the optional Helm
apps from their Argo CD values and compare the strict hardening findings against the documented
chart-owned residual allowlist. To reuse downloaded chart archives, set
`AI_ML_PLATFORM_CHART_CACHE=/path/to/cache`; `HELM_BIN` and `YQ_BIN` can point at local tool
binaries.

The intended data path is:

```text
Airbyte/Kafka/NATS/HTTP telemetry
  -> dbt/Spark/Dagster/Airflow jobs when batch or warehouse work is needed
  -> dd-ai-ml-pipeline for online features and risk/anomaly scoring
  -> MLflow for model lineage as models become trained artifacts
  -> Qdrant/LlamaIndex for vector and LLM retrieval workloads
  -> dd-mdp-optimizer for policy selection
```

The pipeline pod does not mount a Kubernetes API token, runs with a read-only root filesystem, runs
from `docker.io/library/python:3.12-slim` with the EC2 host checkout mounted read-only at
`/opt/dd-next-1`, opens only a bounded writable `/tmp`, and only allows egress to kube-dns plus the
NATS client port in the `messaging` namespace when a NetworkPolicy-capable CNI is installed.
First-party platform Dockerfiles remain available for local image testing:

```bash
bash remote/tools/build-ai-ml-platform-images.sh
```

The namespace boundary NetworkPolicies are ingress-only. They keep same-namespace traffic open,
allow the expected platform namespaces to call into Airbyte/Kafka/Spark, and allow the
`observability` namespace to scrape, without blocking egress to S3, registries, the Kubernetes API,
or external data systems. Inbound traffic to `ai-ml` stays limited to same-namespace callers,
`default`, and `observability`; this avoids widening the pipeline-specific ingress policies through
an additive namespace-wide rule.
The chart values also request runtime-default seccomp profiles where the current charts expose
security-context knobs, and the Spark pipeline server runs without an API token as a non-root pod.
Spark Operator chart `1.6.0` hardcodes the operator `logs-volume` as `emptyDir: {}` before
user-provided volumes, so sizing that writable path requires a chart upgrade, fork, or
post-render patch.
Airflow uses an ExternalSecret-rendered metadata connection for the chart's `data.metadataSecretName`
instead of letting Helm generate a default `postgres:postgres` connection secret.
The Airflow app, migration, and StatsD containers run with read-only roots; bounded `/tmp` and log
volumes provide the writable paths the chart templates need.
The embedded Airflow Postgres primary keeps its data path on the PVC, uses a read-only container
root, and only opens bounded writable `/tmp` and `/dev/shm` volumes.
Qdrant mounts an ExternalSecret-rendered `local.yaml` for API key configuration instead of relying
on the chart's Helm `lookup` path, and keeps snapshots on a PVC instead of the chart's default
snapshots `emptyDir`. Chart `1.17.1` still renders a small unbounded `qdrant-init` marker
`emptyDir`; fixing that requires a chart upgrade, fork, or post-render patch.
MLflow's main container runs with a read-only root filesystem and bounded writable `/tmp` plus
artifact PVC paths. Its Postgres runs as the seed-layer `dd-mlflow-postgresql` StatefulSet because
chart `1.8.1` hardcodes an unbounded subchart temp volume. The chart still hardcodes an unbounded
auth ini `emptyDir`, leaves the `ini-file-initializer` init container without a container-level
security context, always emits an empty env Secret, and generates the Flask signing-key hook Secret
unless a literal key is supplied, so keep that value out of Git until the chart gains an
existing-secret value.
Airbyte's rendered app, API, worker, launcher, cron, Temporal, connector-builder, and bootloader
containers now use read-only roots with bounded writable temp/config volumes; the external Airbyte
Postgres StatefulSet also bounds its socket and temp `emptyDir`s and only accepts ingress from pods
labeled `dd-airbyte-postgresql-client=true`; the policy also denies all pod egress from the database.
Chart `1.9.2` still hardcodes service-account token automount on `cron`, `worker`, and
`workload-launcher`, and emits an empty pre-install/pre-upgrade `dd-airbyte-airbyte-secrets` hook
Secret when all real database and storage credentials are supplied through external Secret
references. Do not populate that hook Secret with fallback literals; fix the remaining Airbyte token
gap with a chart upgrade, fork, or post-render patch.
Dagster's Postgres runs as the seed-layer `dd-dagster-postgresql` StatefulSet because chart
`1.13.3` bundles an older PostgreSQL subchart that cannot expose a container-level
`readOnlyRootFilesystem` value. The Dagster chart still hardcodes
`automountServiceAccountToken: true` into its daemon, webserver, and migration pod templates even
though the chart-created ServiceAccount disables token automount; fixing that requires a chart
upgrade, fork, or post-render patch path rather than a normal values override.

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

`AIRFLOW_POSTGRES_PASSWORD` is projected both to the bundled PostgreSQL password keys and to the
Airflow metadata `connection` key through the External Secrets Operator template in
`dd-ai-ml-platform-secrets.externalsecret.yaml`.

Airbyte's chart is configured with `postgresql.enabled=false`, `global.database.type=external`, and
`global.storage.type=s3`. The external database endpoint is the `dd-airbyte-postgresql` StatefulSet
in the `airbyte` namespace, and all DB/S3 credentials come from External Secrets. The S3 bucket name
is `dd-remote-dev-airbyte` in `us-east-1`; create or verify that bucket before syncing Airbyte. The
version-controlled Terraform stack for that bucket lives at
`remote/terraform/aws/airbyte-s3`.
