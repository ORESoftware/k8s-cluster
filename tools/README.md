# tools

Release tooling for the superproject. `gitops-release.mjs` renders the reviewed
`fiducia-infra` overlays into self-contained production manifests and verifies
that every Fiducia workload image is pinned by registry digest.

The tool never contacts a Kubernetes API. `kubectl kustomize` is used only as a
local renderer and validator.
