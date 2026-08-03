
# gha-capacity-broker-rs

`gha-capacity-broker` is a per-organization GitHub Actions capacity and routing broker. It is deliberately **not** a replacement implementation of GitHub's proprietary Actions control plane.

The compatibility boundary is:

- GitHub's official ARC controller and runner execute normal trusted Linux workflow/action steps.
- This service reads current-month organization Actions usage, evaluates an explicit policy, and can reconcile two selected-repository organization variables: `CI_EXECUTION_MODE` and `CI_LINUX_RUNS_ON_JSON`.
- The existing `dd-build-server` remains an independent bounded fallback for reviewed `run-profile` requests. `gha-capacity-broker` never accepts or executes repository-supplied shell commands.

Linear: `DEN-1549`.

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

One process represents exactly one GitHub organization and one GitHub App installation. `GHA_ORGANIZATION` names that organization and `GHA_ORG_POLICY_JSON` carries its policy. A request naming another organization receives `404`; do not model one installation ID as cross-organization authority.

A personal-account owner such as `ORESoftware` is not an organization and cannot use this organization billing/variable path. Use repository-scoped runners for personal repositories or move the repository under an organization.

## Authentication

Only GitHub App authentication is implemented. The process signs a short-lived App JWT from a mounted private-key file and exchanges it for an installation access token. It does not read a PAT environment variable and does not assume installation tokens have a fixed length.

The capacity-broker App needs:

- organization Administration: read, for enhanced billing usage;
- organization Variables: write, for the two routing variables;
- repository access limited to selected repositories adopting the routing contract.

ARC uses a separate App with organization self-hosted-runner permissions. Store both Apps in AWS Secrets Manager and project them with External Secrets. Never commit private keys or long-lived tokens.

## Configuration

Required variables:

- `GHA_ORGANIZATION`
- `GHA_ORG_POLICY_JSON`
- `GITHUB_APP_ID`
- `GITHUB_APP_INSTALLATION_ID`
- `GITHUB_APP_PRIVATE_KEY_PATH`
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

The billing request explicitly supplies the current UTC year and month. `selfHostedReady` is an operator-controlled certification bit and must remain false until runner groups exist, a scale set registers, the manual smoke passes, and hosted-vs-ARC parity is recorded.

If billing cannot be read, policy fails closed: use validated ARC capacity when certified, otherwise hold. `build-server` mode is advisory and applies only to workflows already written to submit a reviewed `dd-build-server` profile.

## Workflow adoption

A compatible trusted Linux job uses:

```yaml
runs-on: ${{ fromJSON(vars.CI_LINUX_RUNS_ON_JSON || '["ubuntu-latest"]') }}
```

Do not use the variable for macOS, Windows, iOS signing, Android emulator/KVM, service-container, privileged image-build, or public-fork jobs. Do not replace required checks before parity.

## Image and deployment

The Dockerfile creates an unprivileged runtime image. Publish it through the existing build server or a dedicated container-build lane, scan it, generate SBOM/provenance, and record the immutable digest. The deployment and policy templates remain excluded from active Kustomizations until that digest, App secret, and parity evidence exist.

## Local and CI checks

```sh
cargo test
python3 remote/argocd/ci-runners/validate-sonus-arc-scaffold.py
```
