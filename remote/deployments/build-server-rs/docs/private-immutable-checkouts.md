# Private immutable checkouts

The Messaging Intel continuity path combines two independent controls:

1. `BUILD_SERVER_GIT_TOKEN_FILE` is read before every Git process. A projected, short-lived GitHub App installation token can therefore rotate between `git init`, `remote add`, `fetch`, `checkout`, or verification without restarting the build-server pod.
2. An exact 40-hex `gitRef` is treated as an immutable commit identity. The server initializes an empty repository, adds the validated remote, fetches that exact object at depth one, checks it out detached, and verifies `<sha>^{commit}`. It never passes a commit SHA to `git clone --branch` and never falls back to a mutable branch or default-branch tip.

Every command retains the `protocol.ext.allow=never`, `protocol.file.allow=never`, and `protocol.local.allow=never` restrictions. Authentication is injected through the Git subprocess environment and is not included in argv or the build log.

Human-readable branch and tag refs continue to use the bounded shallow-clone path. Executable profile jobs remain restricted by `BUILD_SERVER_ALLOWED_PROFILE_REPO_PREFIXES`; the exact Messaging Intel repository rule does not grant sibling, suffix-lookalike, SSH-alias, or organization-wide access.

## Validation contract

The combined build-server test suite must prove:

- exact 40-hex detection, including uppercase hexadecimal and rejection of short/non-hex values;
- five-command detached checkout with no `--branch` fallback;
- checkout of a requested non-tip commit from a real bare Git remote;
- credential-file reload after the token value changes;
- redacted debug output and bounded, absolute token-file paths;
- exact repository admission and rejection of suffix lookalikes.

A live Kubernetes canary additionally requires the reviewed GitHub App installed on `messaging-intel/msgint-connectors` with repository contents read permission and a projected installation-token Secret mounted into both continuity services.
