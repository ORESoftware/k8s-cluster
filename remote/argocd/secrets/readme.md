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

The cluster also needs a bootstrap Kubernetes secret named `dd-aws-secrets-manager-auth` in
namespace `default` with:

- `AWS_ACCESS_KEY_ID`
- `AWS_SECRET_ACCESS_KEY`

`dd/remote-dev/lambda-runner-secrets` must include `LAMBDA_DATABASE_URL`; the
Gleam lambda runner consumes that key through an explicit `secretKeyRef` so
function invocation can look up lambda definitions by UUID without inheriting
the REST API secret bundle.

Prefer replacing that static bootstrap key with an EC2 instance profile or IRSA equivalent once the
cluster identity path is settled.

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
