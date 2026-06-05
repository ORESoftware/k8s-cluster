# Airbyte S3 Bucket

This Terraform stack owns the private AWS S3 bucket used by the Airbyte Helm
deployment in `remote/argocd/apps/dd-airbyte.application.yaml`.

It creates:

- `dd-remote-dev-airbyte` in `us-east-1`
- S3 public access block with all public access disabled
- bucket-owner-enforced object ownership
- bucket versioning
- default server-side encryption with S3-managed keys
- a bucket policy that denies non-TLS requests
- a least-privilege IAM user/access key for Airbyte object access
- `dd/remote-dev/airbyte-s3` in AWS Secrets Manager for External Secrets

It does not create CloudFront. Terraform state contains the generated IAM
secret access key because AWS only returns it at creation time; keep state in an
operator-owned/backend location and do not commit it.

## Apply

Use the local operator AWS profile from `~/.aws/credentials`. The applying
principal must be allowed to create/configure the bucket, create the dedicated
IAM user/access key, and write `dd/remote-dev/airbyte-s3` in Secrets Manager.
`apply-policy.template.json` is the narrow policy shape for a temporary
operator grant or CI/OIDC role; replace `<account-id>` before attaching it.

```sh
aws sts get-caller-identity --profile dd-codex
AWS_PROFILE=dd-codex terraform init
AWS_PROFILE=dd-codex terraform plan
AWS_PROFILE=dd-codex terraform apply
```

Terraform state is intentionally not committed; see the repository `.gitignore`.

The EC2 node role `dd-remote-k8s-role` is intentionally limited to the
External Secrets read/write path documented in `remote/argocd/secrets/readme.md`.
Do not permanently broaden that role just so Kubernetes can create buckets or
IAM users; use a short-lived operator profile or workflow role, then let
External Secrets consume the generated Secrets Manager value.
