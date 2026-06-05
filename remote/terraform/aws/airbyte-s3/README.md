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

It does not create CloudFront. It also does not write Airbyte access keys into
AWS Secrets Manager. Airbyte still reads `AIRBYTE_S3_ACCESS_KEY_ID` and
`AIRBYTE_S3_SECRET_ACCESS_KEY` from `dd/remote-dev/ai-ml-platform-secrets` via
External Secrets.

## Apply

Use the local operator AWS profile from `~/.aws/credentials`:

```sh
aws sts get-caller-identity --profile dd-codex
AWS_PROFILE=dd-codex terraform init
AWS_PROFILE=dd-codex terraform plan
AWS_PROFILE=dd-codex terraform apply
```

Terraform state is intentionally not committed; see the repository `.gitignore`.
