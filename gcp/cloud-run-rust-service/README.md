# GCP Cloud Run Rust service module

This module is the centralized deployment implementation for the ORES Rust
service lifecycle contract. Product `*-infra` repositories remain lightweight
indexes: they declare desired services and point here rather than maintaining a
second Terraform state or copying this module.

## Required HTTP contract

Every image deployed through this module must implement:

| Route | Purpose | Required behavior |
| --- | --- | --- |
| `/healthz` | Liveness | Process-only and dependency-free. Return 200 while the process can serve HTTP. |
| `/readyz` | Startup/readiness | Fail closed with 503 until validated configuration, current required migrations, required dependencies, and admission/drain state are ready. |
| `/version` | Build identity | Return a bounded, non-secret repository slug, immutable commit SHA, UTC build timestamp, and configuration-contract version with `Cache-Control: no-store`. |
| `/metrics` | Metrics | Optional for the module, but required by the wider fleet observability contract. Keep it private or authenticated; do not expose tenant data or secrets. |

Cloud Run startup deliberately probes `/readyz`, not `/healthz`, because traffic
can be routed as soon as startup succeeds. Periodic liveness remains
process-only so a downstream outage cannot create a restart storm. Periodic
readiness removes an unhealthy instance from traffic without killing it.

The readiness probe is a Cloud Run launch-stage feature. This module therefore
uses `google-beta` and sets `launch_stage = "BETA"`. Callers must keep the
provider within the supported 7.x range and review provider release notes before
upgrading.

## Security and ownership boundaries

- Pass only an Artifact Registry image pinned by `@sha256:<digest>`.
- Pass a dedicated least-privilege runtime service account for each service.
- `environment` is for non-secret values only. Bind Secret Manager values in the
  calling root module so IAM and secret versions remain explicit and reviewable.
- Terraform state, project creation, APIs, IAM, DNS, and production credentials
  stay in the central cluster/cloud root modules. Product infra indexes must not
  create an independent state backend.
- Health routes are unauthenticated platform endpoints. Their responses must be
  bounded and must not contain dependency error strings, hostnames, environment
  variables, credentials, tenant identifiers, or database metadata.
- Public API documentation follows the separate `/docs/api`, `/api/docs`, and
  `/api/docs.json` contract; it is not part of these probes.

## Example

```hcl
module "api" {
  source = "../../gcp/cloud-run-rust-service"

  project_id      = var.project_id
  region          = "us-central1"
  name            = "example-api-server"
  image           = "us-central1-docker.pkg.dev/example/prod/api@sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
  service_account = google_service_account.api.email

  min_instance_count = 0
  max_instance_count = 10

  labels = {
    organization = "example-org"
    role         = "api-server"
  }

  environment = {
    RUST_LOG = "info"
  }
}
```

Before applying a caller, run `terraform fmt -check`, `terraform validate`, the
repository's policy checks, and a plan review. After deployment, capture the
image digest, `/version` response, readiness transitions, and bounded SIGTERM
drain evidence in the matching Linear issue.
