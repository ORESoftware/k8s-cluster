# GHA continuity immutable images

The continuity control plane has two runtime binaries built from one locked Rust crate:

- `gha-clone-server`: bounded workflow planner, webhook receiver, and run coordinator;
- `gha-executor-router`: fixed-profile AWS/Hetzner placement and provider-pinned status proxy.

The shared Dockerfile exposes separate `clone-server` and `executor-router` targets. The builder and runtime bases are pinned by multi-platform index digest. Final images contain one binary, the CA bundle inherited from the pinned builder, and the pinned Debian slim runtime. They run as UID/GID `65532:65532`; Kubernetes may override that with its existing non-root UID.

## CI contract

`.github/workflows/gha-continuity-images.yml` performs the following before publication:

1. BuildKit Dockerfile checks.
2. Locked release builds of both targets.
3. Read-only, capability-free, no-new-privileges startup tests against the real `/healthz` and `/readyz` endpoints.
4. Runtime-root inspection that rejects Cargo, rustc, Git, the sibling binary, and other build-stage leakage.
5. Local BuildKit exports with an SPDX SBOM and max-mode SLSA provenance; the validator requires an attested subject digest.
6. Image scans that fail on fixable HIGH or CRITICAL findings. The scanner action is pinned to the immutable safe Trivy 0.35.0 commit using Trivy 0.69.3.
7. Positive and adversarial tests for immutable release-metadata rendering, idempotency, conflict detection, permissions, and credential-marker rejection.

The pull-request job has read-only repository permissions and does not receive package-write or issue-write permission. Repository-wide observability coverage also requires `dd-gha-executor-router` in both the exporter source default and the deployed `WATCH_APPS` override, so the runtime cannot be activated without workload-level metrics.

## Publication

A successful push to `dev`, or an explicit `workflow_dispatch` with `publish=true`, publishes:

- `ghcr.io/oresoftware/gha-clone-server:sha-<40-hex revision>`;
- `ghcr.io/oresoftware/gha-executor-router:sha-<40-hex revision>`;
- mutable `:dev` pointers for discovery only.

Every published image carries an SPDX SBOM and max-mode SLSA provenance. CI reads both attestations back from GHCR by digest, scans the exact digest, and writes the immutable reference to the workflow summary. Production manifests must never use `:dev` or a SHA tag as authority; a follow-up GitOps PR must copy the exact `image@sha256:...` references.

## GHA continuity OCI release digest ledger

The trusted publisher also appends one validated machine-readable record per target to `ORESoftware/k8s-cluster#702`. Each record binds:

- schema version;
- source repository;
- exact 40-hex source revision;
- Docker target;
- canonical GHCR image;
- lowercase `sha256:` digest;
- canonical `image@sha256` deployment reference.

Publication uses only the workflow-scoped `GITHUB_TOKEN`. The trusted publish job has `packages: write` and `issues: write`; pull-request validation remains read-only and the publish job is skipped for pull requests. No personal token, GitHub App private key, Actions runner-registration credential, executor credential, repository secret, workflow input, or customer value is required or written.

The ledger is idempotent. An identical record for the same `(source_sha, target)` marker creates no new comment. If that marker already exists with a different body or digest, publication stops as a reproducibility conflict. The publisher reads every issue-comment page before classifying the marker.

GitOps must copy the exact `ref` from the record matching the reviewed source revision and target. The workflow summary, mutable `:dev` tag, and mutable or discoverable SHA tag are not release authority.

## Activation boundary

Image publication does not activate the services. The merged GitOps deployments retain:

- `replicas: 0`;
- clone API execution disabled;
- webhook execution disabled;
- router execution disabled;
- Hetzner disabled and without URL or credential state.

Activation requires a separate reviewed change that atomically removes runtime source cloning/compilation, pins both ledger-produced image digests, preserves the non-root/read-only/no-capability boundary, validates ExternalSecrets, and runs the AWS/provider-loss/no-duplicate smoke sequence. Native GitHub Actions parity remains the responsibility of official ARC; these images contain only the bounded independent continuity lane.
