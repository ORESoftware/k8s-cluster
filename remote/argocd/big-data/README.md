# Big Data Kubernetes Stack

This bundle deploys a small development-oriented big-data stack with the same basic hardening
posture as the surrounding remote runtime:

- Apache Spark standalone cluster: one master and two workers.
- Apache Airflow: one webserver/scheduler pod with a smoke-test DAG.
- MinIO: S3-compatible object storage for data-lake style inputs and outputs.

Databricks is not included because it is normally consumed as a managed service
or external control plane, not deployed as a plain in-cluster Kubernetes
`Deployment`.

## Apply

Install/sync External Secrets first, then create the AWS Secrets Manager JSON secret
`dd/remote-dev/big-data-secrets` with these keys:

- `MINIO_ROOT_USER`
- `MINIO_ROOT_PASSWORD`
- `AIRFLOW_ADMIN_USERNAME`
- `AIRFLOW_ADMIN_PASSWORD`

Authenticate to the Kubernetes cluster, verify the local AWS profile, then apply the
kustomization.

```bash
aws sts get-caller-identity --profile dd-codex
kubectl apply -k remote/argocd/big-data
kubectl -n big-data get externalsecret,secret,pvc,pods,svc
```

## Local UIs

```bash
kubectl -n big-data port-forward svc/spark-master 8080:8080
kubectl -n big-data port-forward svc/airflow 8082:8080
kubectl -n big-data port-forward svc/minio 9001:9001
```

- Spark UI: <http://localhost:8080>
- Airflow UI: <http://localhost:8082> (credentials from `dd/remote-dev/big-data-secrets`)
- MinIO console: <http://localhost:9001> (credentials from `dd/remote-dev/big-data-secrets`)

## Notes

These manifests are still a development stack, not the full production path:

- Credentials come from External Secrets; do not commit fallback credentials.
- Spark, Airflow, and MinIO images are pinned as `tag@sha256:<digest>` references so audited tags
  cannot drift between syncs.
- MinIO uses a small `ReadWriteOnce` PVC so object data survives pod restarts, and its Deployment
  uses `Recreate` rollouts to avoid two pods contending for the same RWO volume during updates.
- Airflow runs the webserver and scheduler in one pod.
- Pod service-account tokens are disabled, containers drop Linux capabilities, Spark/MinIO/Airflow
  use runtime-default seccomp profiles, and a same-namespace default-deny NetworkPolicy is included
  for NetworkPolicy-capable CNIs.
- Workloads use a dedicated `big-data-workload` ServiceAccount with token automount disabled and no
  RBAC bindings.
- Spark, Airflow, and MinIO run with read-only root filesystems; their runtime state is limited to
  explicit `emptyDir` or PVC mounts.
- The namespace includes a ResourceQuota and LimitRange so future dev jobs cannot consume the whole
  EC2 cluster by accident.
- Spark workers have a PodDisruptionBudget with `minAvailable: 1`; the single-replica Spark master,
  Airflow, and MinIO pods remain development singletons.
- For production, prefer the official Airflow Helm chart, a Spark Operator or managed Spark
  platform, external Postgres for Airflow, larger persistent volumes, and backup/restore runbooks.
