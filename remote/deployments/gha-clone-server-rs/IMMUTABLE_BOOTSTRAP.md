# Immutable bootstrap boundary

The inactive `dd-gha-clone-server` deployment fetches source only by an exact,
lowercase 40-hex Git commit SHA. It initializes an empty repository, fetches that
object directly, checks out `FETCH_HEAD` detached, and verifies that `HEAD`
exactly equals the configured revision. Branch and tag names are not accepted.

The reviewed source revision is currently:

```text
412f03155ba108890735414d6fbf5a1a72d9c554
```

This removes mutable source selection, but it does **not** make the deployment
activation-ready. The pod still starts from a mutable Rust toolchain tag and
builds source at startup. Therefore all of these gates remain mandatory:

1. keep `replicas: 0`;
2. keep API and webhook execution disabled;
3. build the server in reviewed CI;
4. publish a minimal runtime image by immutable digest;
5. replace the source-build container with that digest-pinned image;
6. verify ExternalSecrets, NetworkPolicies, fixed executor profiles, and the
   plan-only meta fixture before scaling to one;
7. retain ARC as the native-semantics lane and use this independent executor
   only for its explicitly supported, fail-closed workflow subset.

The AWS and Hetzner executor lanes must select and durably fence one provider
before submission. A job already accepted by one provider must not be replayed
on the other merely because status polling failed. Cross-provider takeover
requires a shared durable run identity, terminal-state reconciliation, and a
Fiducia fencing token.
