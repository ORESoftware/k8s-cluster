# GitHub Actions continuity profile repository admission

`dd-build-server` has two separate repository boundaries:

1. `BUILD_SERVER_ALLOWED_REPO_PREFIXES` controls which repositories may be cloned for any build-server job.
2. `BUILD_SERVER_ALLOWED_PROFILE_REPO_PREFIXES` is the narrower executable-code gate for `jobKind=run-profile`.

The second list supports two deliberately different rule forms:

- `https://github.com/ORESoftware/` — an ordinary prefix rule, granting reviewed profile execution to repositories in that organization URL namespace.
- `=https://github.com/messaging-intel/msgint-connectors.git` — an exact canonical URL rule, granting only that one HTTPS repository URL.

The leading `=` is security-significant. A repository URL written without it is evaluated as a prefix. Therefore a single-repository grant must always use the exact form. The validator rejects empty exact rules, sibling repositories, SSH aliases that were not separately reviewed, and suffix-appended lookalikes such as:

```text
https://github.com/messaging-intel/msgint-connectors.git-evil
https://github.com/messaging-intel/msgint-connectors-extra.git
git@github.com:messaging-intel/msgint-connectors.git
```

The broader clone allowlist does not imply profile-execution permission. Adding either an organization prefix or an exact repository requires a reviewed manifest change, and the dedicated build-server unit suite must exercise both the positive rule and nearby negative cases.

For Messaging Intel, the current reviewed rule is:

```text
=https://github.com/messaging-intel/msgint-connectors.git
```

The GHA continuity server reserves the exact `messaging-intel/msgint-connectors` repository and `.github/workflows/gha-clone-operator-config.yml` workflow path, requires reviewed revision `a9cc977d78347ec0efdbe8e6766967f80d425882`, validates the exact workflow name, trigger, ordered DAG, action SHAs, action inputs, and command arrays before privileged profile assignment, and sends only the canonical HTTPS repository URL plus a fixed profile name to the build server. A reserved identity mismatch is terminal and cannot fall back to `node-verify`.

This reservation is additive and does not widen the existing organization-prefix rules: another Messaging Intel repository, another workflow path, another revision, or another command sequence must be admitted by a separate reviewed contract before it can receive a fixed executable profile.

Permanent pull-request verification is read-only. Branch-normalization or materialization helpers must remove themselves before review readiness; no `contents: write` job is part of the accepted Messaging Intel continuity workflow.
