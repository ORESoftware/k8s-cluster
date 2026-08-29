# Daedalus Fab GitHub Actions continuity

This directory defines the inert, review-first prerequisites and operating
contracts for a `daedalus-fab` organization runner scale set replicated across
AWS and Hetzner.

It is not a reimplementation of GitHub Actions. Normal trusted Linux jobs run on
GitHub's official Actions Runner Controller (ARC) and runner. The independent
`gha-clone-server-rs` plus `dd-build-server` path remains a narrower fallback for
workflows that compile to reviewed fixed profiles.

## Execution lanes

| Lane | Scope | Reviewed identity |
| --- | --- | --- |
| ARC on Hetzner | `daedalus-fab` organization | scale set `daedalus-ci`, runner group `daedalus-hetzner` |
| ARC on AWS | `daedalus-fab` organization | scale set `daedalus-ci`, runner group `daedalus-aws` |
| Fixed-profile build server | exact repository + immutable SHA | `gha-clone-server-rs` to `dd-build-server` |

`ORESoftware/k8s-cluster` is owned by a personal account, not a GitHub
organization. Organization billing and runner-group APIs do not apply to that
owner. Its continuity path is repository-scoped ARC or the fixed-profile
build-server adapter; do not pretend an organization runner group exists there.

## Deliberately inert defaults

- The prerequisite namespace, quota, limits, and NetworkPolicy are safe to
  reconcile.
- The ARC controller and scale-set Applications contain no automated sync
  policy. They must be synced manually after every activation gate is proven.
- Both clouds use `minRunners: 0`; no warm capacity is purchased merely by
  merging this scaffold.
- The GitHub App ExternalSecret is a `.template.yaml` file excluded from all
  Kustomizations.
- The manual smoke workflow is a template and consumes no repository secrets.
- The example continuity snapshot is entirely false and the evaluator exits
  nonzero until a reviewed lane is certified.

## Activation gates

1. Audit existing ARC controllers and cluster-scoped CRD ownership. Do not let
   two Helm releases fight over the same CRDs.
2. Create organization runner groups `daedalus-aws` and
   `daedalus-hetzner`, restricted to reviewed repositories.
3. Create a dedicated `daedalus-fab` ARC GitHub App with only the permissions
   required by ARC. Do not reuse the billing-read or variable-mutation Apps.
4. Store the App fields at `dd/ci/github-apps/daedalus-fab-arc`, review the
   ExternalSecret template, and intentionally materialize it.
5. Sync the controller Application for one cloud, then the runner-set
   Application for that cloud.
6. Copy the smoke template to a reviewed Daedalus repository and run it
   manually. Record the exact run URL, commit, provider, runner name, and
   successful non-privileged checks.
7. Set that provider's `configured`, `registered`, and `smokePassed` evidence to
   true in a private operational snapshot and run:

   ```sh
   python3 scripts/ops/gha_continuity_status.py \
     --snapshot /path/to/sanitized-status.json \
     --require arc
   ```

8. Repeat for the second cloud. Hosted-vs-ARC parity and required-check behavior
   must be recorded before replacing a required GitHub-hosted job.
9. Enable `gha-clone-server` webhook execution only after its `/readyz` and
   `dd-build-server` `/readyz` are green and the exact repository/workflow rules
   are installed.

## Hosted-minute routing

For true GitHub organizations, the existing `gha-capacity-broker-rs` can read
organization Actions usage and reconcile reviewed repository variables. It must
fail closed to certified ARC or hold when billing data is unavailable. The
broker cannot query organization billing for the personal `ORESoftware` owner.

The connected GitHub surface used by ChatGPT does not expose the exact remaining
Actions billing balance. A hosted workflow starting or completing proves only
that hosted execution is not globally blocked at that moment; it does not prove
how many included minutes remain.

## Rollback

- Disable the failure webhook or set webhook execution false.
- Set scale-set `maxRunners` to zero and wait for running jobs to finish.
- Remove the manually synced runner-set Application, then its controller only
  after confirming no other scale set depends on it.
- Leave the namespace policies in place while collecting sanitized evidence.
- Revert repository runner variables to GitHub-hosted labels or hold.

Never use a PAT pasted into chat as an activation credential. Revoke and rotate
any exposed token and use narrowly scoped GitHub Apps through the secret manager.
