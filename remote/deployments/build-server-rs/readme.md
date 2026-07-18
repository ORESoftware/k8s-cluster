# `dd-build-server`

Rust CI/CD control surface for the EC2 Kubernetes runtime. It works alongside GitHub Actions:
GHA (or a repo push, or another service) triggers a build here, and the server clones, builds with
`nerdctl`, optionally pushes to ECR, and optionally deploys with `kubectl`.

It exposes authenticated JSON endpoints for:

- `POST /builds` to clone a repo, build an image with `nerdctl`, and optionally deploy a manifest
  or kustomize overlay with `kubectl`.
- `GET /builds` and `GET /builds/<jobId>` to inspect build state (in-memory, plus recent jobs from
  Postgres when persistence is enabled).
- `GET /builds/<jobId>/logs` to read the capped build log from disk.
- `POST /webhooks/github` and `POST /webhooks/registry` for GitHub push/workflow_run events and
  container-registry image events (see **Webhooks**).
- `POST /secrets/sync` and `GET /secrets/sync/status` to push selected secrets to GitHub Actions
  (see **GitHub Actions secret sync**).
- `GET /healthz` and `GET /metrics` for probes and Prometheus.

## CI/CD integration surface

Built on the cluster's shared infrastructure so it composes with the rest of the fleet:

- **Concurrency control — fiducia.cloud.** Each build takes a per-image lock from the fiducia.cloud
  coordination API (`/v1/locks/acquire`, monotonic fencing tokens), so concurrent builds of the
  same image serialize across replicas; webhooks/NATS submissions dedupe through fiducia
  idempotency leases. Opt-in via `BUILD_SERVER_COORDINATION_ENABLED`; the in-cluster endpoint and
  the `dd-contract-service` integration pattern are reused. See [src/fiducia.rs](src/fiducia.rs).
- **Persistence — Amazon RDS Postgres.** Its OWN database `dd_build_server` (its own namespace,
  separate from the shared pg-defs contract), via SeaORM. Jobs, webhook deliveries, and secret-sync
  audit rows survive restarts; jobs interrupted by a restart are marked failed on boot. Declarative
  schema contract at `remote/libs/pg-defs/schema/databases/dd_build_server/schema.sql`, converged
  with [`scripts/dpm.sh`](scripts/dpm.sh) (never at boot). See [src/db.rs](src/db.rs).
- **Messaging — NATS.** Lifecycle events on `dd.remote.build_server.events`, terminal results on
  `.results`, image events on `.images`, and alert-worthy failures on `dd.remote.events.critical`
  (Observability Contract). An optional durable JetStream work-queue intake on
  `.requests` (`BUILD_SERVER_NATS_INTAKE_ENABLED`) lets CI enqueue builds without holding an HTTP
  connection. Subjects are owned by `remote/libs/nats/subject-defs`
  (`build-server.schema.json`). See [src/events.rs](src/events.rs).
- **Secrets — External Secrets + AWS Secrets Manager.** The cluster's cross-cloud backbone (the
  ClusterSecretStore has AWS/GCP/Hetzner providers) is the source of truth; the server can also
  push selected values OUT to GitHub Actions repo secrets over the GitHub REST API (libsodium
  sealed box), keeping GHA and the cluster on one secret source. See [src/gh_secrets.rs](src/gh_secrets.rs).
- **Alternate executor — gleam-lambda-runner.** A job may set `executor: "lambda"` to run the build
  through the sandboxed gleam-lambda-runner build function instead of local `nerdctl`. Opt-in via
  `BUILD_SERVER_LAMBDA_ENABLED`. See [src/lambda_exec.rs](src/lambda_exec.rs).

## Webhooks

`POST /webhooks/github` verifies the GitHub `X-Hub-Signature-256` HMAC
(`BUILD_SERVER_GITHUB_WEBHOOK_SECRET`, constant-time) and dedupes on `X-GitHub-Delivery`. Rules
(`BUILD_SERVER_WEBHOOK_RULES` inline JSON, or `BUILD_SERVER_WEBHOOK_RULES_PATH` mounted from the
`dd-build-server-rules` ConfigMap) map `repo`/`branch`/`event` to a `build-server.v1` job, with
`{sha}`/`{shortSha}`/`{ref}` substituted into the image tag. `POST /webhooks/registry` authenticates
a shared-secret header (`BUILD_SERVER_REGISTRY_WEBHOOK_SECRET`), normalizes ECR EventBridge and
docker distribution v2 payloads, and relays them to NATS. These two paths are reachable through the
gateway WITHOUT the operator cookie (GitHub can't present it) and carry no operator credential — the
server authenticates them itself.

## GitHub Actions secret sync

`POST /secrets/sync` (operator-authenticated) or a periodic loop
(`BUILD_SERVER_GH_SYNC_INTERVAL_SECONDS`, serialized across replicas by a fiducia lock) pushes
env-provided values (populated by External Secrets from AWS Secrets Manager) to GitHub Actions repo
secrets. Values are encrypted client-side with each repo's libsodium public key
(`GET /repos/{repo}/actions/secrets/public-key` → sealed box → `PUT .../secrets/{name}`). Only
SHA-256 hashes are persisted, for change detection — never the values. Rules come from
`BUILD_SERVER_GH_SYNC_RULES`/`_PATH`; the PAT from `GH_PAT` (dd-agent-secrets) or
`GH_SECRETS_SYNC_TOKEN`. Disabled by default (`BUILD_SERVER_GH_SYNC_ENABLED`).

The server intentionally does not accept arbitrary shell commands. A submitted job is a
`build-server.v1` JSON document:

- `schemaVersion`: optional; when present it must be `build-server.v1`.
- `jobKind`: optional; `build-image` or `build-and-deploy`.
- `repoUrl`: `https://`, `ssh://`, or `git@` repo URL.
- `gitRef`: optional branch or tag, passed to `git clone --branch`.
- `image`: explicit image tag or digest to build. The deployment currently allowlists
  `710156900967.dkr.ecr.us-east-1.amazonaws.com/`.
- `contextDir` and `dockerfile`: relative paths inside the cloned repo.
- `buildArgs`: optional non-secret Docker build args. Keys containing `SECRET`, `PASSWORD`,
  `TOKEN`, `CREDENTIAL`, or `PRIVATE_KEY` are rejected and values are redacted from command logs.
- `push`: optional; when `true`, the image is pushed after a successful build.
- `deploy.kind`: `kustomize`, `manifest`, or `none`.
- `deploy.path`: relative path inside the cloned repo.
- `deploy.namespace`: namespace allowlisted by `BUILD_SERVER_ALLOWED_NAMESPACES`.

Example:

```json
{
  "schemaVersion": "build-server.v1",
  "jobKind": "build-and-deploy",
  "repoUrl": "https://github.com/example/app.git",
  "gitRef": "main",
  "image": "710156900967.dkr.ecr.us-east-1.amazonaws.com/example-app:dev",
  "contextDir": ".",
  "dockerfile": "Dockerfile",
  "push": true,
  "deploy": {
    "kind": "kustomize",
    "path": "k8s/overlays/ec2",
    "namespace": "default",
    "rollout": "deployment/example-app"
  }
}
```

The Argo runtime deployment runs this from the host-mounted checkout and mounts:

- the EC2 containerd socket
- `/usr/local/bin/nerdctl`
- `/usr/bin/kubectl`
- `/var/lib/dd-build-server`

`SERVER_AUTH_SECRET` must come from `dd-agent-secrets`. ECR push support is enabled by
`BUILD_SERVER_PUSH_ENABLED=true` and `BUILD_SERVER_ECR_LOGIN_ENABLED=true`. For ECR auth, provide
`AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, and optional `AWS_SESSION_TOKEN` through
`dd-agent-secrets` or replace the env-based signer with an instance-role credential provider.
Deploys are limited to namespaces listed in `BUILD_SERVER_ALLOWED_NAMESPACES`.

## Security notes

This service is safe from arbitrary shell execution at the HTTP/API layer: callers can only request
the fixed sequence `git clone`, `nerdctl build`, optional ECR `nerdctl login` + `nerdctl push`, and
optional `kubectl apply` + `kubectl rollout status`.

It is not a fully untrusted code sandbox. A Dockerfile is code, and a Kubernetes Deployment
manifest is also code that can run pods. Today this build server should be used for trusted repos
and authenticated operators. For untrusted repos, run builds in a separate empty namespace with no
valuable secrets, replace the host containerd socket with rootless BuildKit or Kaniko, and add an
admission policy that blocks secret mounts, privileged pods, hostPath, host networking, and
service-account token automounting.

Current hardening:

- API auth header compared in constant time over SHA-256 digests (no timing/length leak)
- git clone runs with `protocol.{ext,file,local}.allow=never`, `--no-tags`, and a `--` separator so
  a repo URL can never be reinterpreted as a git option or a non-network transport
- command execution uses direct argv, not `/bin/sh -c`
- child commands run with a stripped environment
- repo, image, namespace, and path allowlists
- ECR push only for allowlisted ECR image prefixes
- queue backpressure (`BUILD_SERVER_MAX_QUEUED`) so authenticated callers cannot grow memory/disk
  without bound; per-job wall-clock deadline (`BUILD_SERVER_JOB_DEADLINE_SECONDS`) on top of the
  per-command timeout; cloned workdirs are removed after each job (logs are kept)
- webhook paths verify their own HMAC/secret and are not handed the operator credential at the
  gateway; secret-sync persists only value hashes, never values
- reduced Kubernetes RBAC: Deployments, Services, ConfigMaps, Ingresses, HPAs, and read-only Events
- no Secret, Pod, ServiceAccount, Job, DaemonSet, StatefulSet, or NetworkPolicy write permissions
- NetworkPolicy restricts ingress to the gateway/observability paths and egress to DNS, kube API,
  and public git/ECR endpoints

> **ORM policy:** prefer **SeaORM** over sqlx for new database code (MASH stack: maud, axum, SeaORM, supabase, htmx).
