# Nix fleet audit

`scripts/nix-fleet-audit.sh` performs a read-only inventory of GitHub organizations or accounts and writes machine-readable JSON plus a Markdown summary.

## Usage

Authenticate `gh` with read access to the organizations being inspected, then run:

```sh
nix develop -c bash scripts/nix-fleet-audit.sh \
  --org ORESoftware \
  --org fiducia-cloud \
  --org sonus-auris \
  --output-dir .cache/nix-fleet-audit
```

For a larger fleet, put one owner name per line in an ignored local file:

```sh
nix develop -c bash scripts/nix-fleet-audit.sh \
  --org-file .cache/github-owners.txt \
  --output-dir .cache/nix-fleet-audit
```

The scanner never writes through the GitHub API. It lists repositories, reads recursive Git trees, and reads workflow or Dockerfile contents needed for classification.

## Classification

Each repository is assigned one of the rollout states required by DEN-323:

- `full flake`: root `flake.nix`, committed `flake.lock`, `.nix/`, flake CI, and agent-command CI were detected;
- `shell only`: some Nix support exists, but the complete agent contract is missing;
- `not applicable`: the repository has no default branch;
- `deferred with reason`: no repository-level Nix contract was found, or detailed inspection was intentionally skipped.

The output also records devcontainer, Dockerfile, Compose, Kubernetes, OCI workflow, digest-pinning, non-root, and supply-chain signals. These checks are intentionally conservative heuristics. Review every proposed bulk change against the repository's actual runtime and release contract.

## Outputs

- `report.json`: stable structured data for Linear updates, dashboards, or follow-up automation;
- `report.md`: concise repository table for human review.

Do not commit reports that reveal private repository names unless the destination has equivalent access controls.

## Tests

The fixture test replaces `gh` with a deterministic read-only mock:

```sh
nix develop -c shellcheck scripts/nix-fleet-audit.sh scripts/tests/nix-fleet-audit-test.sh
nix develop -c shfmt -d scripts/nix-fleet-audit.sh scripts/tests/nix-fleet-audit-test.sh
nix develop -c bash scripts/tests/nix-fleet-audit-test.sh
```
