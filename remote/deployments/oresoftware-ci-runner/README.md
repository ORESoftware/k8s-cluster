# ORESoftware ARC runner image

This image extends GitHub's official Actions runner `2.334.0` with the small
native baseline needed by repository workflows: compilers, CMake, Git/Git LFS,
OpenSSL headers, Python 3, SSH, and archive tools. It is intended only for
ephemeral ARC scale-set pods.

Rust, Node, pnpm, Gleam/Erlang, Java, Flutter, browsers, and kubectl remain
selected by pinned setup actions in each workflow. That keeps one image useful
across repositories without pretending to reproduce the full mutable
`ubuntu-latest` software inventory.

Builds on pull requests are verification-only. Pushes to `main` publish:

- `ghcr.io/oresoftware/oresoftware-ci-runner:main`
- `ghcr.io/oresoftware/oresoftware-ci-runner:sha-<commit>`

Before applying the `dd.dev/ci-runners=oresoftware` cluster label, replace the
bootstrap `:main` reference in the ApplicationSet with the published `sha-*`
tag and then its immutable digest.
