# GitHub Actions continuity profile repository admission

`dd-build-server` has two separate repository boundaries:

1. `BUILD_SERVER_ALLOWED_REPO_PREFIXES` controls which repositories may be cloned for any build-server job.
2. `BUILD_SERVER_ALLOWED_PROFILE_REPO_PREFIXES` is the narrower executable-code gate for `jobKind=run-profile`.

The second list supports two deliberately different rule forms:

- `https://github.com/ORESoftware/` — an ordinary prefix rule, granting reviewed profile execution to repositories in that organization URL namespace.
- `=https://github.com/messaging-intel/msgint-connectors.git` — an exact canonical URL rule, granting only that one HTTPS repository URL.
- `=https://github.com/3FA-app/3fa-interfaces.git` — an exact canonical URL rule for the bounded 3FA interfaces workflow only.

The leading `=` is security-significant. A repository URL written without it is evaluated as a prefix. Therefore a single-repository grant must always use the exact form. The validator rejects empty exact rules, sibling repositories, SSH aliases that were not separately reviewed, and suffix-appended lookalikes such as:

```text
https://github.com/messaging-intel/msgint-connectors.git-evil
https://github.com/messaging-intel/msgint-connectors-extra.git
git@github.com:messaging-intel/msgint-connectors.git
https://github.com/3FA-app/3fa-interfaces.git-evil
https://github.com/3FA-app/3fa-backend.rs.git
git@github.com:3FA-app/3fa-interfaces.git
```

The broader clone allowlist does not imply profile-execution permission. Adding either an organization prefix or an exact repository requires a reviewed manifest change, and the dedicated build-server unit suite must exercise both the positive rule and nearby negative cases.

For Messaging Intel, the current reviewed rule is:
For Messaging Intel, the reviewed rule is:

```text
=https://github.com/messaging-intel/msgint-connectors.git
```

The GHA continuity server independently allowlists the exact `messaging-intel/msgint-connectors` repository and `.github/workflows/gha-clone-operator-config.yml` workflow path, requires an immutable 40-hex revision for execution, and sends only the canonical HTTPS repository URL plus a fixed profile name to the build server.

## Validation lanes

Normal pull-request and push validation is credential free: it compiles the bounded workflow fixture, runs the exact repository-admission and fixed-profile tests, executes a dependency-free hardened Node fixture, and verifies the real Rust server dispatches only the reviewed profiles. The live private-repository smoke is manual and uses a short-lived GitHub App token restricted to `messaging-intel/msgint-connectors`; it is never a prerequisite for untrusted pull-request code.
For 3FA interfaces, the reviewed rule is:

```text
=https://github.com/3FA-app/3fa-interfaces.git
```

The continuity server independently allowlists only `3FA-app/3fa-interfaces` and `.github/workflows/gha-clone-contracts.yml`. The bounded workflow maps its Node job to `node-hardened-test` and its generated-Rust job to `rust-generated-verify`. The generated-Rust profile accepts one exact command sequence targeting `generated/rust/Cargo.toml`; reordered, additional, or alternate Cargo commands are not approximated.

Neither exact rule authorizes sibling repositories, organization-wide profile execution, publication, formal-model downloads, secrets, environments, service containers, or caller-selected commands.
The GHA continuity server reserves the exact `messaging-intel/msgint-connectors` repository and `.github/workflows/gha-clone-operator-config.yml` workflow path, requires reviewed revision `a9cc977d78347ec0efdbe8e6766967f80d425882`, validates the exact workflow name, trigger, ordered DAG, action SHAs, action inputs, and command arrays before privileged profile assignment, and sends only the canonical HTTPS repository URL plus a fixed profile name to the build server. A reserved identity mismatch is terminal and cannot fall back to `node-verify`.

This reservation is additive and does not widen the existing organization-prefix rules: another Messaging Intel repository, another workflow path, another revision, or another command sequence must be admitted by a separate reviewed contract before it can receive a fixed executable profile.

Permanent pull-request verification is read-only. Branch-normalization or materialization helpers must remove themselves before review readiness; no `contents: write` job is part of the accepted Messaging Intel continuity workflow.
