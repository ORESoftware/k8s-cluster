# `remote/argocd/ai-ml-platform`

GitOps seed layer for the remote AI/ML data platform.

This bundle installs the lightweight always-on pieces:

- `ai-ml` namespace
- `dd-ai-ml-tool-catalog` ConfigMap describing the selected open-source stack
- `dd-ai-ml-pipeline`, a Python3 online feature pipeline that bridges telemetry into the existing
  Rust MDP/POMDP/RL optimizer
- an `ExternalSecret` that mirrors `dd/remote-dev/agent-secrets` into the `ai-ml` namespace for
  `SERVER_AUTH_SECRET`
- a locked-down ServiceAccount and NetworkPolicy for the Python pipeline

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
| Data ingestion | Airbyte | `dd-airbyte.application.yaml` |

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
