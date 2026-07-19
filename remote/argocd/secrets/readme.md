# Remote Secrets GitOps

Do not commit raw API keys or database URLs.

This folder is the GitOps bridge between ArgoCD and the cluster's selected secret backend:

- GitHub stores only `ExternalSecret` and `ClusterSecretStore` manifests.
- the selected cloud secret backend stores the real values.
- External Secrets Operator reconciles those values into Kubernetes `Secret` objects consumed by
  the deployments through `envFrom` / `secretKeyRef`.

`remote/argocd/apps/external-secrets-operator.application.yaml` installs the operator through the
official Helm chart with CRDs enabled.

Required secret names:

- `dd/remote-dev/agent-secrets` -> creates `dd-agent-secrets`
- `dd/remote-dev/rest-api-secrets` -> creates `dd-remote-rest-api-secrets`
- `dd/remote-dev/lambda-runner-secrets` -> creates `dd-gleam-lambda-runner-secrets`
- `dd/remote-dev/idle-reaper-secret` -> creates `dd-idle-reaper-secret`
- `dd/remote-dev/mcp-secrets` -> creates `dd-gleam-mcp-server-secrets`
- `dd/remote-dev/gleamlang-server-secrets` -> creates `dd-gleamlang-server-secrets`
- `dd/remote-dev/lmx-admin-token` -> creates `dd-lmx-admin-token`
- `dd/remote-dev/ai-ml-platform-secrets` -> consumed by the optional AI/ML chart
  `ExternalSecret`s in `remote/argocd/ai-ml-platform`
- `dd/remote-dev/big-data-secrets` -> consumed by the optional
  `remote/argocd/big-data` ExternalSecrets for Airflow and MinIO

`dd/remote-dev/lmx-admin-token` must include `LMX_ADMIN_TOKEN`. Both broker
deployments (`dd-rust-network-mutex` and `dd-live-mutex`) consume it through
explicit `secretKeyRef`s so `/admin/*` endpoints stop falling back to a
literal default baked into the broker source. Rotate
this value separately from `dd-agent-secrets` so admin-token changes do not
force the wider Node coding-agent fleet to restart.

`dd/remote-dev/agent-secrets` is also the home for Git credentials used by remote-dev workers.
Expected Git keys are:

- `DD_REPO_URL`
- `DD_REPO_REF`
- `GH_DEPLOY_KEY`
- `GH_DEPLOY_KEY_PUBLIC` for operator/audit convenience
- `GH_PAT` if GitHub CLI PR creation needs a token

Never commit deploy-key material or bake it into a worker image. `dd-dev-server` writes
`GH_DEPLOY_KEY` from the Kubernetes secret to a private key file at container startup.

All `ExternalSecret` manifests reference the cloud-neutral `dd-cluster-secrets`
`ClusterSecretStore`. Provider directories under `providers/` decide how that store is backed:

- `providers/aws` uses AWS Secrets Manager through the External Secrets controller pod's default AWS
  credential chain. On EC2 this is the node instance role `dd-remote-k8s-role`, which also backs the
  `Remote K8s maintenance` GitHub Actions workflow over SSM.
- `providers/hetzner` also reads AWS Secrets Manager, but uses the
  `external-secrets/aws-sm-creds` Kubernetes secret for the AWS access key pair because Hetzner
  nodes do not have an EC2 instance role.
- `providers/gcp` uses Google Secret Manager with ambient GCP workload credentials.

A single inline
policy `ManageRemoteDevSecrets` covers both consumers:

```jsonc
{
  "Version": "2012-10-17",
  "Statement": [
    {
      "Sid": "ManageRemoteDevSecretsByPath",
      "Effect": "Allow",
      "Action": [
        "secretsmanager:CreateSecret",      // workflow: repair-gleamlang-secret bootstrap path
        "secretsmanager:DescribeSecret",    // ESO + workflow probe
        "secretsmanager:PutSecretValue",    // workflow: repair-gleamlang-secret
        "secretsmanager:GetSecretValue",    // ESO read path
        "secretsmanager:ListSecretVersionIds", // ESO read path
        "secretsmanager:TagResource"        // future tag-on-create paths
      ],
      "Resource": "arn:aws:secretsmanager:us-east-1:<account>:secret:dd/remote-dev/*"
    },
    {
      "Sid": "ReadBenefactorGhcrPullCredential",
      "Effect": "Allow",
      "Action": [
        "secretsmanager:DescribeSecret",
        "secretsmanager:GetSecretValue"
      ],
      "Resource": "arn:aws:secretsmanager:us-east-1:<account>:secret:dd/benefactor/ghcr-pull-*"
    },
    {
      "Sid": "ListSecretsForInspect",
      "Effect": "Allow",
      "Action": "secretsmanager:ListSecrets", // not resource-scopeable
      "Resource": "*"
    }
  ]
}
```

Do not split the `dd/remote-dev/*` statement back into separate read-only and
write policies — the bootstrap path needs `CreateSecret` and the inspect path
needs `ListSecrets`, and the ESO read path is a strict subset of those actions
on the same resource prefix. The benefactor registry statement is deliberately
separate and read-only so the cluster can pull its private image without gaining
write access to that credential.

`dd/remote-dev/lambda-runner-secrets` must include `LAMBDA_DATABASE_URL`; the Gleam lambda runner
consumes that key through an explicit `secretKeyRef` so function invocation can look up lambda
definitions by UUID without inheriting the REST API secret bundle.

`dd/remote-dev/mcp-secrets` must include `RDS_DATABASE_URL` and
`AGENT_TASKS_RDS_DATABASE_URL`; the Gleam MCP server consumes those keys through explicit
`secretKeyRef`s so read-only MCP tools can inspect database-backed contracts without inheriting the
broader REST API or agent secret bundles.

`dd/remote-dev/agent-secrets` and `dd/remote-dev/rest-api-secrets` are also projected into `ai-ml`,
but only as narrow key projections: `SERVER_AUTH_SECRET` for the Python/Spark pipeline auth path,
and `RDS_DATABASE_URL` for `dd-spark-pipeline-server` Postgres access. Do not use broad
`dataFrom.extract` projections for the AI/ML namespace.

`dd/remote-dev/big-data-secrets` must include `MINIO_ROOT_USER`, `MINIO_ROOT_PASSWORD`,
`AIRFLOW_ADMIN_USERNAME`, and `AIRFLOW_ADMIN_PASSWORD` before applying the optional
`remote/argocd/big-data` bundle or syncing `dd-big-data.application.yaml`. Keep those values
rotation-friendly in the selected secret backend; do not restore literal fallback credentials to the
manifests or docs.

`dd/remote-dev/ai-ml-platform-secrets` must include the chart-owned Airbyte auth, Airbyte
database/storage, Airflow, Dagster, MLflow, and Qdrant keys listed in
`remote/argocd/ai-ml-platform/readme.md`. Those chart apps should consume generated Kubernetes
secrets through `existingSecret`, `existingAdminSecret`, or `secretKeyRef` values; do not commit
fallback chart credentials such as `admin/admin`, `postgres`, `minio123`, or `test`.

## Updating Values

Update live values in the selected secret backend, not in Git. This repo should only change when
the secret shape changes, such as adding a new key, adding a new service-specific `ExternalSecret`,
or changing which deployment consumes a generated Kubernetes secret.

Safe rotation flow:

1. Put a new secret version in the selected secret backend.
2. Sync the `dd-secrets` ArgoCD application, or wait for External Secrets Operator refresh.
3. Restart deployments that read the changed secret through env vars.
4. Verify the consuming service with health checks and telemetry.

The future admin UI should never display existing values. It should accept replacement values,
write them through a server-side AWS SDK call, record a redacted audit event, sync ArgoCD, and
restart only affected deployments. A manual GitHub Actions `workflow_dispatch` can do the same
thing with AWS OIDC and masked inputs; GitHub hooks should trigger manifest syncs only, not
transmit secret values.
