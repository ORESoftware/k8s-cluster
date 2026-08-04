# Build-server exact repository profile policy

`dd-build-server` executes repository code for `jobKind=run-profile`, so repository admission and profile admission are separate security decisions.

## Policy layers

1. `BUILD_SERVER_ALLOWED_REPO_PREFIXES` controls which repositories the service may clone for any job.
2. `BUILD_SERVER_ALLOWED_PROFILES` controls which compiled fixed profiles are globally enabled.
3. `BUILD_SERVER_ALLOWED_PROFILE_REPO_PREFIXES` is the legacy organization/repository prefix fallback for profile jobs.
4. `BUILD_SERVER_PROFILE_REPOSITORY_RULES_JSON` binds an exact canonical repository URL to the only fixed profiles it may execute.

Exact rules override prefix fallback. Once a repository matches an exact rule, a different profile is rejected even when an organization-wide prefix would otherwise allow it.

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

Each rule must satisfy all of these conditions:

- canonical `https://github.com/OWNER/REPO.git` URL;
- one owner and one repository component;
- non-empty profile list;
- no duplicate repository rules or duplicate profile names;
- every profile exists in the compiled registry;
- every profile is enabled by `BUILD_SERVER_ALLOWED_PROFILES`;
- no unknown JSON fields;
- at most 256 exact repositories and 32 profiles per repository;
- total JSON size at most 64 KiB.

Invalid policy is a startup error. It is never logged and ignored, because that would silently reopen the broader prefix path.

## Initial binding

The first exact rule binds:

```text
https://github.com/ORESoftware/k8s-cluster.git -> rust-verify
```

That permits the GHA continuity server to dogfood its Rust verification profile while rejecting a downgrade to `node-verify`, `python-verify`, browser profiles, or Flutter profiles.

Repositories without an exact rule continue to use the reviewed prefix fallback. Adding a sensitive repository should normally include an exact rule in the same pull request as its fixed profile and workflow contract.

## Test contract

The Rust policy tests prove:

- exact-match allow and deny decisions;
- exact rules override broad prefixes;
- suffix, sibling, SSH-alias, nested-path, query, fragment, whitespace, control-character, wildcard, and case-variant lookalikes are rejected;
- malformed, duplicate, unknown, globally disabled, empty, oversized, and separator-injection policies fail closed;
- encoded policy output is deterministic;
- a manually malformed compiled rule does not grant access.

The GitOps contract test verifies that the deployment binds `k8s-cluster` only to `rust-verify` and that the dedicated GHA workflow formats and executes the policy tests.

Temporary formatter or branch-writing workflows are not part of the deployable policy. The reviewed pull-request diff must contain only the policy source, its GitOps configuration, documentation, and tests.
