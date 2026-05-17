# Remote Secrets GitOps

Do not commit raw API keys or database URLs.

This folder is the GitOps bridge between ArgoCD and AWS Secrets Manager:

- GitHub stores only `ExternalSecret` and `ClusterSecretStore` manifests.
- AWS Secrets Manager stores the real values.
- External Secrets Operator reconciles those values into Kubernetes `Secret` objects consumed by
  the deployments through `envFrom` / `secretKeyRef`.

`remote/argocd/apps/external-secrets-operator.application.yaml` installs the operator through the
official Helm chart with CRDs enabled.

Required AWS secret names:

- `dd/remote-dev/agent-secrets` -> creates `dd-agent-secrets`
- `dd/remote-dev/rest-api-secrets` -> creates `dd-remote-rest-api-secrets`
- `dd/remote-dev/lambda-runner-secrets` -> creates `dd-gleam-lambda-runner-secrets`
- `dd/remote-dev/idle-reaper-secret` -> creates `dd-idle-reaper-secret`
- `dd/remote-dev/mcp-secrets` -> creates `dd-gleam-mcp-server-secrets` if MCP write tools are
  enabled

The `dd-aws-secrets-manager` store uses the External Secrets controller pod's default AWS
credential chain. On EC2 this means the node instance role must allow
`secretsmanager:GetSecretValue`, `secretsmanager:DescribeSecret`, and
`secretsmanager:ListSecretVersionIds` on `arn:aws:secretsmanager:us-east-1:<account>:secret:dd/remote-dev/*`.

`dd/remote-dev/lambda-runner-secrets` must include `LAMBDA_DATABASE_URL`; the Gleam lambda runner
consumes that key through an explicit `secretKeyRef` so function invocation can look up lambda
definitions by UUID without inheriting the REST API secret bundle.

## Updating Values

Update live values in AWS Secrets Manager, not in Git. This repo should only change when the secret
shape changes, such as adding a new key, adding a new service-specific `ExternalSecret`, or
changing which deployment consumes a generated Kubernetes secret.

Safe rotation flow:

1. Put a new secret version in AWS Secrets Manager.
2. Sync the `dd-secrets` ArgoCD application, or wait for External Secrets Operator refresh.
3. Restart deployments that read the changed secret through env vars.
4. Verify the consuming service with health checks and telemetry.

The future admin UI should never display existing values. It should accept replacement values,
write them through a server-side AWS SDK call, record a redacted audit event, sync ArgoCD, and
restart only affected deployments. A manual GitHub Actions `workflow_dispatch` can do the same
thing with AWS OIDC and masked inputs; GitHub hooks should trigger manifest syncs only, not
transmit secret values.
