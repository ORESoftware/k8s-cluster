# GHA continuity immutable images

The GHA continuity program has three independently deployable Rust runtime images:

- `gha-clone-server`: bounded workflow planner, webhook receiver, and run coordinator;
- `gha-executor-router`: fixed-profile AWS/Hetzner placement and provider-pinned status proxy;
- `gha-capacity-broker`: organization billing-aware routing policy for GitHub-hosted Actions, official ARC, and the separately bounded build-server lane.

The clone server and executor router are built as separate targets from the locked `gha-clone-server-rs` crate. The capacity broker is built from its own locked crate and publication workflow. All builder and runtime bases are pinned by immutable multi-platform digest.

Final images contain one executable plus the CA bundle required for outbound HTTPS. They run as UID/GID `65532:65532`, declare exact compiled entrypoints, and contain no Cargo, rustc, Git, sibling continuity binary, shell-based source bootstrap, or package-manager installation layer.

## CI contract

The two image workflows perform equivalent evidence checks before publication:

- `.github/workflows/gha-continuity-images.yml` — clone server and executor router;
- `.github/workflows/gha-capacity-broker-image.yml` — capacity broker.

The validation boundary includes:

1. BuildKit Dockerfile checks.
2. Locked release builds from exact `Cargo.lock` files.
3. Read-only, capability-free, no-new-privileges startup tests against real health/readiness endpoints.
4. Runtime-root inspection that rejects Cargo, rustc, Git, sibling continuity binaries, and build-stage leakage.
5. Local BuildKit exports with an SPDX SBOM and max-mode SLSA provenance; the shared validator requires an attested subject digest.
6. Image scans that fail on fixable HIGH or CRITICAL findings. Both workflows use the immutable safe Trivy 0.35.0 action commit with Trivy 0.69.3.
7. Positive and adversarial tests for immutable release-metadata rendering, target/image identity, idempotency, conflict detection, permissions, and credential-marker rejection.

The capacity-broker smoke mounts two distinct dummy GitHub App key paths, keeps `GHA_MUTATION_ENABLED=false`, and proves `/healthz`, `/readyz`, `/api/v1/capabilities`, and `/metrics` without requesting an installation token or contacting the GitHub API.

Pull-request jobs have read-only repository permissions and do not receive package-write or issue-write permission. Trusted publication jobs use only the workflow-scoped `GITHUB_TOKEN`; no PAT, GitHub App private key, ARC registration credential, billing credential, mutation credential, executor credential, repository secret, or workflow-provided customer value is used.

Repository-wide observability coverage requires the clone server, executor router, Sonus capacity broker, and StreemPilot capacity broker in the applicable resource-exporter inventories before activation.

## Publication

A successful trusted push to `dev`, or an explicit workflow dispatch with `publish=true`, publishes:

- `ghcr.io/oresoftware/gha-clone-server:sha-<40-hex revision>`;
- `ghcr.io/oresoftware/gha-executor-router:sha-<40-hex revision>`;
- `ghcr.io/oresoftware/gha-capacity-broker:sha-<40-hex revision>`;
- mutable `:dev` pointers for discovery only.

Every published image carries an SPDX SBOM and max-mode SLSA provenance. CI reads both attestations back from GHCR by digest, scans the exact digest, and writes the immutable reference to the workflow summary. Production manifests must never use `:dev` or a SHA tag as authority; a follow-up GitOps PR must copy the exact `image@sha256:...` reference.

The capacity broker is published only by `.github/workflows/gha-capacity-broker-image.yml`. Adding its Docker target to the ledger renderer does not retroactively create a broker release record. The first authoritative broker digest exists only after a trusted `dev` publication completes and issue #702 contains the exact record.

## GHA continuity OCI release digest ledger

The trusted publishers append one validated machine-readable record per target to `ORESoftware/k8s-cluster#702`. The allowed target/image identities are exactly:

| Target | Canonical image |
| --- | --- |
| `clone-server` | `ghcr.io/oresoftware/gha-clone-server` |
| `executor-router` | `ghcr.io/oresoftware/gha-executor-router` |
| `capacity-broker` | `ghcr.io/oresoftware/gha-capacity-broker` |

Each record binds:

- schema version;
- source repository;
- exact 40-hex source revision;
- release target;
- canonical GHCR image;
- lowercase `sha256:` digest;
- canonical `image@sha256` deployment reference.

Publication uses only the workflow-scoped `GITHUB_TOKEN`. Trusted publish jobs have `packages: write` and `issues: write`; pull-request validation remains read-only and publication is skipped for pull requests.

The ledger is idempotent. An identical record for the same `(source_sha, target)` marker creates no new comment. If that marker already exists with a different body or digest, publication stops as a reproducibility conflict. Publishers read every issue-comment page before classifying the marker.

GitOps must copy the exact `ref` from the record matching the reviewed source revision and target. Workflow summaries, mutable `:dev` tags, and mutable or discoverable SHA tags are not release authority.

## Capacity-broker runtime boundary

The broker image contains only the capacity service. Its runtime configuration still requires distinct billing-read and selected-repository mutation GitHub App installations plus an operator authentication secret. Those credentials are mounted by the eventual deployment and are never embedded in the image, build context, attestations, ledger record, or publication workflow.

Image publication does not authorize billing reads or variable mutation. `GHA_MUTATION_ENABLED` remains false until runner groups, official ARC scale sets, hosted-versus-ARC parity, provider-loss behavior, exact organization billing evidence, and rollback are independently certified.

## Activation boundary

Image publication does not activate any continuity service. Merged GitOps remains intentionally inert:

- clone-server replicas `0`;
- executor-router replicas `0`;
- clone API execution disabled;
- webhook execution disabled;
- router execution disabled;
- capacity-broker templates excluded from active overlays;
- capacity-broker mutation disabled;
- `selfHostedReady=false`;
- ARC `minRunners=0`;
- Hetzner disabled in the independent executor inventory and without URL or credential state.

Activation requires separate reviewed changes that atomically replace all-zero digest sentinels with matching ledger-produced references, preserve non-root/read-only/no-capability boundaries, validate ExternalSecrets, retain separate GitHub App authorities, and execute AWS/Hetzner parity, provider-loss, no-duplicate, billing, and rollback evidence.

Native GitHub Actions parity remains the responsibility of GitHub-hosted Actions and official ARC. These images implement only billing-aware routing and the bounded independent continuity lane; they do not clone GitHub's proprietary Actions control plane.
