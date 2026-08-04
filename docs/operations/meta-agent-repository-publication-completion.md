# Meta Agent repository publication completion

Status: completed on 2026-08-03 UTC and reverified on 2026-08-04 UTC.

The canonical public repository exists at
`meta-agents-demo/meta-agent-control-plane.rs` with default branch `main`.
The recovered publication was verified against the sealed bundle inventory:

- initial `main`: `4d6ec3ad0ec7b688f0e777129eee7e0f0d999df1`;
- initial `agent/den-1057-meta-agent-control-plane`:
  `789d48039da232faed985d4f8de176959f117e08`.

The initial implementation pull request was merged in the target repository.
Normal source review, CI, dependency updates, runtime hardening, and deep
conformance now happen there. Repository creation is no longer an active
cluster operation and must not be retriggered from ordinary GitHub Actions.

## Retired execution path

The one-shot owner-device workflow
`.github/workflows/ops-owner-device-create-meta-agent-repo.yml` was removed after
live metadata and both exact initial refs were verified. Its draft execution
carrier was closed by the successful run. Retaining an interactive owner OAuth
flow after completion would unnecessarily preserve a privileged repository-
administration surface and invite confusing or expired authorization prompts.

The retired workflow must not be restored merely because an old carrier,
document, bundle, or commit references it. Any future recovery requires a new
reviewed threat model and pull request.

## Retained recovery evidence

The following remain as immutable audit and disaster-recovery inputs:

- source commit `55ee15c190b7cfa4e075f6984c7cb551acd4b9d3`;
- bundle SHA-256
  `1ddaa03743b864348162149b7d2d2e2dce7eab585cf092ea14547c647fcec031`;
- publisher SHA-256
  `e2fe6eaa622db02a54f83e27a822f64ad4b54971c883f97bbda4ac0a4db5d278`;
- `scripts/critical-org-fleet/assets/meta.part*`;
- `scripts/ops/publish_meta_agent_control_plane_from_actions.sh`, which
  reconstructs and verifies the pinned publisher from the immutable source;
- `scripts/ops/verify_meta_agent_source_snapshot.py`;
- `docs/operations/meta-agent-ephemeral-credential-publication.md`;
- credential-free source-snapshot verification and the publication evidence in
  Linear issues DEN-1057, DEN-1058, and DEN-319.

These artifacts are not routine sources of truth after the target repository
exists. Future recovery must first prove the repository is absent or
irrecoverable, verify that no safe repair or restoration is possible, and use a
newly reviewed create-only path. Recovery must preserve the exact target
identity and reviewed history, refuse semantic divergence, and never use force
pushes to replace a live repository.

## Ongoing controls

- Product changes use pull requests in
  `meta-agents-demo/meta-agent-control-plane.rs`.
- Public metadata, default branch, and initial refs are checked by the focused
  retirement contract workflow.
- Publication-carrier uniqueness remains enforced in `ORESoftware/k8s-cluster`.
- Repository bootstrap is excluded from steady-state Kubernetes reconciliation
  and retained only as explicit recovery/audit material.
- Credentials disclosed in chat, logs, comments, or other non-secret channels
  must be revoked and rotated; they are never part of the recovery record.
- Branch protection, required checks, semantic conflict handling, and normal
  target-repository review remain independent post-bootstrap controls.
