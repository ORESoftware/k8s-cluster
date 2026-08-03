# ORESoftware GitHub Actions fallback fleet

This directory is the GitOps control plane for organization-level self-hosted
GitHub Actions capacity and the signed failure bridge.

## Components

- `control-plane/applicationset.yaml` generates one ARC controller and runner
  scale set in each explicitly opted-in Kubernetes cluster.
- `bridge/` deploys `gha-clone-server`, its exact-path public webhook ingress,
  its allowlisted rules, and least-privilege network policy.
- `remote/deployments/oresoftware-ci-runner/` owns the ephemeral runner image.
- `.github/workflows/self-hosted-fallback.yml` is the first repository-owned
  fallback workflow.

The runner ApplicationSets are active but produce **zero child Applications**
until a registered Argo cluster has both labels:

```text
dd.dev/managed=true
dd.dev/ci-runners=oresoftware
```

That activation gate prevents a half-configured scale set from repeatedly
failing while GitHub runner groups, GitHub App credentials, or immutable images
are still missing.

## AWS and Hetzner active-active contract

Both clusters use `runnerScaleSetName: oresoftware-ci`, which is the label used
by `runs-on`. Their Argo cluster secret must also have `dd.dev/cloud=aws` or
`dd.dev/cloud=hetzner`; the ApplicationSet derives distinct runner groups:

- `oresoftware-ci-aws`
- `oresoftware-ci-hetzner`

Create both groups in the ORESoftware organization before adding the activation
label. Give each group access only to trusted repositories. GitHub may assign a
queued job to either online scale set; loss of one cluster leaves the other
eligible to acquire jobs.

## Required secret contract

In `arc-runners-oresoftware` on every activated cluster, create
`oresoftware-arc-github` with GitHub App keys:

```text
github_app_id
github_app_installation_id
github_app_private_key
```

The bridge reuses the existing `BUILD_SERVER_GITHUB_WEBHOOK_SECRET` from
`dd-build-server-secrets`, `SERVER_AUTH_SECRET` from `dd-agent-secrets`, and the
existing `GH_PAT` bootstrap key. Rotate any exposed PAT before activation and
migrate the bridge to a renewable GitHub App installation token.

## Promotion sequence

1. Merge the image workflow and publish both images.
2. Replace both `:main` image references with immutable digests.
3. Upgrade the active ARC controller and existing scale sets to the chart
   version used here (`0.14.2`) as one reviewed change.
4. Create the two organization runner groups.
5. Materialize `oresoftware-arc-github` in both runner namespaces.
6. Add `dd.dev/ci-runners=oresoftware` to the AWS and Hetzner Argo cluster
   secrets one cluster at a time.
7. Dispatch `Self-hosted fallback` manually with a known exact SHA.
8. Register the organization `workflow_run` webhook only after the smoke passes.

The complete operator runbook is
`docs/operations/github-actions-self-hosted-fallback.md`.
