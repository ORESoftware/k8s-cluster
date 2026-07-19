# Data-plane desired state

One directory per production provider. `manifests.yaml` is generated from the
reviewed `apps/fiducia-infra` gitlink and then pinned to exact GHCR registry
digests. Argo CD reads only these generated directories.

Run `node tools/gitops-release.mjs check` to verify the release bill of
materials, hashes, image pins, Secret exclusion, and Kustomize builds.
