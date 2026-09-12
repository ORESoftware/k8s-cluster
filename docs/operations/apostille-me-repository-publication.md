# apostille-me repository publication

This one-shot publisher creates and verifies the eight public repositories requested for the `apostille-me` organization:

- `apme-interfaces`
- `apme-api`
- `apme-web-mash`
- `apme-web-leptos`
- `apme-web-dioxus`
- `apme-cli`
- `apme-sync`
- `apme-infra`

The source histories are carried in one namespaced Git bundle, split into reviewable base64 payload parts. Its SHA-256 is `d9df6bba633c27b56c6f9e7eb2fd007b70c652b2dde8d3c332637c3ea7ffbedd`. The workflow reconstructs and verifies that digest before the publisher verifies all 17 namespaced refs.

Each target repository receives an empty `main` review base and exactly one bootstrap commit on `agent/bootstrap-apostille-me`. The publisher verifies branch ancestry, commit counts, clean Git object integrity, live repository ownership, exact remote ref SHAs, and one open pull request per repository. It never force-pushes and stops if any pre-existing remote ref diverges.

The main-branch workflow uses GitHub CLI device OAuth. It accepts only a temporary `gho_` credential and rejects `ghp_` and `github_pat_` personal access-token formats. The temporary profile is removed at workflow completion.

Tracking issue: ORESoftware/k8s-cluster#845.
