# gha-capacity-broker-rs

`gha-capacity-broker` is a per-organization GitHub Actions capacity and routing broker. It is deliberately **not** a clone of GitHub's proprietary workflow service.

The compatibility boundary is:

- GitHub's official Actions Runner Controller (ARC) and runner execute normal trusted Linux workflow/action steps on AWS or Hetzner.
- This service reads the current-month organization Actions billing summary, evaluates an explicit policy, and can reconcile two selected-repository organization variables: `CI_EXECUTION_MODE` and `CI_LINUX_RUNS_ON_JSON`.
- `remote/deployments/gha-clone-server-rs` remains the independent fail-closed workflow planner and fixed-profile dispatcher.
- The existing `dd-build-server` remains the bounded cluster-local fallback for reviewed `run-profile` requests. The capacity broker never accepts or executes repository-supplied shell commands.

Linear: `DEN-1549`; related: `DEN-1550`, `DEN-378`, `DEN-381`, and `DEN-27`.

## API

- `GET /healthz`
- `GET /readyz`
- `GET /metrics`
- `GET /api/v1/capabilities`
- `GET /api/v1/organizations/:org/decision`
- `POST /api/v1/organizations/:org/reconcile`
- `GET /api/docs.json`
- `GET /api/docs` and `GET /docs/api`

Decision and reconcile routes require `x-server-auth`. Reconciliation is dry-run unless `GHA_MUTATION_ENABLED=true`.

## Organization boundary

One process represents exactly one GitHub organization, one billing-read GitHub App installation, and one variable-mutation GitHub App installation. `GHA_ORGANIZATION` names the organization and `GHA_ORG_POLICY_JSON` carries its policy. A request naming another organization receives `404`; do not model one installation as cross-organization authority.

A personal-account owner such as `ORESoftware` is not an organization and cannot use this organization billing/variable path. Use repository-scoped runners for personal repositories or move the repository under an organization.

## Three-App authority model

Use three independently installed GitHub Apps per organization:

1. **ARC registration App** — organization self-hosted-runner registration only. ARC owns this credential; the broker never mounts it.
2. **Billing-read App** — organization `Administration: read`, used only for the public-preview billing summary endpoint.
3. **Variable-mutation App** — only the permissions required to create or update `CI_EXECUTION_MODE` and `CI_LINUX_RUNS_ON_JSON` for explicitly selected repositories.

The broker mints separate short-lived installation tokens for the billing and mutation Apps. Their App IDs, installation IDs, private-key files, token caches, and secret-manager records must remain distinct. A configuration that reuses the same App installation or private-key path fails at startup.

The long-lived PAT pasted into chat is not an activation credential. Revoke and rotate it under `DEN-27`; this implementation does not store or use it.

## Billing API and quantity semantics

The broker queries the current UTC month from the public-preview endpoint:

```text
GET /organizations/{org}/settings/billing/usage/summary?year=YYYY&month=M&product=Actions
```

The current response uses `grossQuantity`, `discountQuantity`, and `netQuantity` rather than the older detailed-report `quantity` field.

- `grossQuantity` is the total Actions-minute consumption before included-usage or other quantity discounts. It is the numerator for the configured included-minute threshold.
- `netQuantity` is retained separately as billable minutes after quantity discounts. It is useful for cost reporting but must not drive included-minute capacity routing.

The parser requires the current `usageItems` shape and has a checked-in fixture matching the public-preview contract. Missing or malformed billing data fails closed to certified ARC capacity or hold.

## Configuration

Required variables:

- `GHA_ORGANIZATION`
- `GHA_ORG_POLICY_JSON`
- `GITHUB_MUTATION_APP_ID`
- `GITHUB_MUTATION_APP_INSTALLATION_ID`
- `GITHUB_MUTATION_APP_PRIVATE_KEY_PATH`
- `GITHUB_BILLING_APP_ID`
- `GITHUB_BILLING_APP_INSTALLATION_ID`
- `GITHUB_BILLING_APP_PRIVATE_KEY_PATH`
- `SERVER_AUTH_SECRET` (at least 32 characters)

Optional variables:

- `HOST` (default `0.0.0.0`)
- `PORT` (default `8117`)
- `GHA_MUTATION_ENABLED` (default `false`)
- `GHA_RECONCILE_INTERVAL_SECONDS` (default `900`)

Example Sonus policy:

```json
{
  "includedMinutes": 2000,
  "warnPercent": 75,
  "selfHostedPercent": 90,
  "hardStopPercent": 100,
  "preferSelfHosted": false,
  "selfHostedReady": false,
  "buildServerEnabled": true,
  "hostedRunsOn": ["ubuntu-latest"],
  "selfHostedRunsOn": ["sonus-ci"],
  "selectedRepositoryIds": [1294558398]
}
```

`selfHostedReady` is an operator-controlled certification bit and must remain false until runner groups exist, a scale set registers, the manual smoke passes, and hosted-vs-ARC parity is recorded.

If billing cannot be read, policy fails closed: use validated ARC capacity when certified, otherwise hold. `build-server` mode is advisory and applies only to jobs already represented by a reviewed `dd-build-server` run profile.

## Safe workflow adoption

A compatible trusted Linux job must gate execution mode as well as selecting the runner label:

```yaml
jobs:
  test:
    if: vars.CI_EXECUTION_MODE == 'hosted' || vars.CI_EXECUTION_MODE == 'self-hosted'
    runs-on: ${{ fromJSON(vars.CI_LINUX_RUNS_ON_JSON || '["ubuntu-latest"]') }}
    steps:
      - run: ./ci/test.sh
```

For `build-server` and `hold`, the broker writes the deliberately nonexistent label `ci-capacity-hold-no-runner` instead of invalid empty JSON. The mode gate is still required: it makes the job skip immediately rather than waiting for a runner. The independent continuity server may dispatch an allowlisted build-server profile when `build-server` is selected.

Do not use the variable for macOS, Windows, iOS signing, Android emulator/KVM, service-container, privileged image-build, or public-fork jobs. Do not replace required checks before hosted-vs-ARC parity and required-check semantics are proven.

## Image and deployment

The Dockerfile creates an unprivileged runtime image. Publish it through the existing build server or a dedicated container-build lane, scan it, generate SBOM/provenance, and record the immutable digest. The deployment and policy templates remain excluded from active Kustomizations until the digest, all three Apps, runner groups, and parity evidence exist.

The Sonus template mounts only the two Apps needed by the broker:

- `/var/run/gha-mutation-app/github_app_private_key` from `sonus-auris-gha-capacity-broker`;
- `/var/run/gha-billing-app/github_app_private_key` from `sonus-auris-gha-billing`.

The ARC registration App is consumed only by ARC through `sonus-auris-arc-github`.

## Local and CI checks

```sh
cargo fmt --check
cargo test --all-features
cargo clippy --all-targets --all-features -- -D warnings
python3 remote/argocd/ci-runners/validate-sonus-arc-scaffold.py
```
