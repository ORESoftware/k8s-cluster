# Build-server exact repository profile policy

`dd-build-server` executes repository code for `jobKind=run-profile`, so repository admission and profile admission are separate security decisions.

## Policy layers

1. `BUILD_SERVER_ALLOWED_REPO_PREFIXES` controls which repositories the service may clone for any job.
2. `BUILD_SERVER_ALLOWED_PROFILES` controls which compiled fixed profiles are globally enabled.
3. `BUILD_SERVER_ALLOWED_PROFILE_REPO_PREFIXES` is the legacy organization/repository prefix fallback for profile jobs.
4. `BUILD_SERVER_PROFILE_REPOSITORY_RULES_JSON` binds an exact GitHub repository identity to the only fixed profiles it may execute.

Exact rules override prefix fallback. Once a repository identity matches an exact rule, a different profile is rejected even when an organization-wide HTTPS or SSH prefix would otherwise allow it.

## JSON schema

The value is a JSON array:

```json
[
  {
    "repository": "https://github.com/ORESoftware/k8s-cluster.git",
    "profiles": ["rust-verify"]
  }
]
```

Each configuration key must satisfy all of these conditions:

- canonical `https://github.com/OWNER/REPO.git` URL;
- one owner and one repository component;
- non-empty profile list;
- no duplicate repository identities or duplicate profile names;
- every profile exists in the compiled registry;
- every profile is enabled by `BUILD_SERVER_ALLOWED_PROFILES`;
- no unknown JSON fields;
- at most 256 exact repositories and 32 profiles per repository;
- total JSON size at most 64 KiB.

Invalid policy is a startup error. It is never logged and ignored, because that would silently reopen the broader prefix path.

## Canonical identity and aliases

Configuration uses one canonical HTTPS URL, but admission compares a lower-case `owner/repository` identity. The same exact rule therefore applies to every supported clone form of that repository:

```text
https://github.com/ORESoftware/k8s-cluster.git
https://github.com/ORESoftware/k8s-cluster
https://github.com/oresoftware/K8S-CLUSTER.git/
git@github.com:ORESoftware/k8s-cluster.git
ssh://git@github.com/ORESoftware/k8s-cluster.git
```

This prevents an SSH, case, optional `.git`, or trailing-slash alias from escaping the exact rule and falling back to the broader organization prefix. Query strings, fragments, nested paths, unsupported transports, whitespace, and control characters are rejected rather than normalized.

A genuinely different repository in an organization remains governed by the reviewed prefix fallback until it receives its own exact rule. Exact identity binding is not a glob or sibling-repository denylist. Branch or revision selection is validated separately and is never included in the repository-to-profile identity, so a mutable ref cannot widen the fixed-profile policy.

## Initial binding

The first exact rule binds:

```text
https://github.com/ORESoftware/k8s-cluster.git -> rust-verify
```

That permits the GHA continuity server to dogfood its Rust verification profile while rejecting a downgrade of the same repository identity to `node-verify`, `python-verify`, browser profiles, or Flutter profiles.

Repositories without an exact rule continue to use the reviewed prefix fallback. Adding a sensitive repository should normally include an exact rule in the same pull request as its fixed profile and workflow contract.

## Review and rollout rule

An exact binding is a code-execution policy change, not a convenience mapping. Every new or modified rule must be reviewed with the corresponding fixed profile, immutable workflow contract, repository URL aliases, and GitOps value in the same pull request. The change must prove both the intended allow decision and at least one denied profile that the broader organization prefix would otherwise admit.

A rollout must start with one exact repository, keep the broader prefix fallback unchanged for unrelated repositories, and verify startup failure for malformed or disabled-profile policy. Do not remove a broad prefix merely to simulate an exact denylist; migrate repositories incrementally and remove a prefix only after every intended repository beneath it has an explicit reviewed binding.

## Test contract

The Rust policy tests prove:

- exact allow and deny decisions across HTTPS, SSH, case, optional `.git`, and trailing-slash aliases;
- exact identity rules override broad HTTPS and SSH prefixes;
- unrelated repositories preserve reviewed prefix behavior;
- query, fragment, nested-path, unsupported-transport, whitespace, control-character, and separator-injection inputs fail closed;
- malformed, duplicate, unknown, globally disabled, empty, and oversized policies fail closed;
- encoded identities are lower-case and profile sets are deterministic;
- malformed compiled state does not grant access.

The GitOps contract test verifies that the deployment binds `k8s-cluster` only to `rust-verify` and that the dedicated GHA workflow formats and executes the policy tests.

Temporary formatter or branch-writing workflows are not part of the deployable policy. The reviewed pull-request diff must contain only the policy source, its GitOps configuration, documentation, and tests.
