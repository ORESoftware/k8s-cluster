# StreemPilot GitHub Actions capacity scaffold

Linear: DEN-1549, DEN-1550. Related: DEN-905, DEN-918, DEN-27.

This directory is an **inert, credential-free review scaffold** for native
GitHub Actions capacity in both AWS and Hetzner plus the organization capacity
broker. Nothing here is added to an active cloud Kustomization by this change.

## Four execution lanes

1. GitHub-hosted runners while the authorized billing summary and policy allow.
2. Official Actions Runner Controller (ARC) scale sets named `streempilot-ci` in
   AWS and Hetzner for native trusted-Linux Actions semantics.
3. `gha-clone-server-rs` for the explicitly supported fail-closed workflow
   subset in `.github/workflows/ci-mirror.yml`.
4. `dd-build-server` for reviewed fixed profiles selected by that mirror.

ARC, not the independent mirror, is the native-parity lane. Browser automation,
Dart, matrices, environments, artifacts, service containers, macOS, Windows,
signing, and other unsupported semantics stay on GitHub-hosted runners or ARC.

## Stable selected repositories

The policy template uses GitHub's numeric repository identities rather than
names that can be renamed:

| Repository | ID |
| --- | ---: |
| `StreemPilot/streempilot-interfaces` | `1318677845` |
| `StreemPilot/streempilot-api-server.rs` | `1318677882` |
| `StreemPilot/streempilot-web-server.rs` | `1318677908` |
| `StreemPilot/streempilot-e2e` | `1318678075` |

The broker may mutate only `CI_EXECUTION_MODE` and
`CI_LINUX_RUNS_ON_JSON` for those selected repositories after mutation is
explicitly enabled. The mirror server keeps its separate exact repository and
workflow-path allowlist.

## Cloud isolation

Both clouds expose the workflow label `streempilot-ci`, but registration uses
distinct organization runner groups:

- AWS: `streempilot-aws`
- Hetzner: `streempilot-hetzner`

The broker routes before GitHub assigns a job. Do not enable both providers for
one ambiguous post-assignment job without the separately reviewed no-duplicate
lease/fencing contract. A provider may take over only after the prior lane is
made ineligible or is proven never to have accepted the job.

Runner pods are one-job, non-root, have no Docker/containerd socket, no mounted
Kubernetes service-account token, bounded emptyDir workspaces, restricted Pod
Security, no ingress, DNS egress, and public TCP/443 egress excluding private,
loopback, carrier-grade NAT, and link-local ranges.

## Three GitHub Apps

Provision separate organization installations and secret-manager records:

1. `streempilot-arc` — ARC registration only.
2. `streempilot-capacity-broker` — selected repository variable mutation and
   broker server authentication only.
3. `streempilot-billing` — organization Actions billing summary read only.

Their App IDs, installation IDs, private keys, External Secrets, and token
caches must remain distinct. A classic PAT is not an activation credential.

## Activation gates

1. Confirm `StreemPilot` organization plan and replace the conservative
   `includedMinutes` placeholder with an authorized billing-policy value.
2. Create least-privilege Apps and reconcile all three External Secrets without
   displaying values.
3. Audit existing ARC CRDs/controllers before installing chart `0.14.2`.
4. Create the `streempilot-aws` and `streempilot-hetzner` runner groups and
   restrict them to selected private repositories.
5. Scan and pin the runner and broker images by immutable digest with SBOM and
   provenance.
6. Promote one cloud first, run the manual smoke, and record hosted-versus-ARC
   check name, conclusion, logs, cache, and artifact parity.
7. Promote the second cloud with `minRunners: 0`, prove provider-loss behavior,
   and retain evidence that one job cannot execute twice.
8. Keep `selfHostedReady: false` and `GHA_MUTATION_ENABLED=false` until both the
   native runner and rollback proof are approved.
9. Enable the independent clone-server lane only after its exact-SHA plan and
   live fixed-profile smokes pass.
10. Do not replace required checks until their native GitHub identity and
    rollback behavior are recorded.

## Rollback

- Set broker mutation false and restore hosted variables.
- Set both scale-set maxima to zero or remove selected repository access from
  the affected runner group.
- Disable clone-server API/webhook execution and scale it to zero.
- Preserve broker decisions, ARC listener/controller logs, GitHub run/check
  evidence, clone-server plans/runs, and build-server jobs for incident review.
- Never bypass required checks because one capacity lane is unavailable.
