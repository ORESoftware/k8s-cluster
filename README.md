# canonical.cloud — legacy mirror

> [!CAUTION]
> **This repository is superseded by [`canonical-cloud/canonical-monorepo`](https://github.com/canonical-cloud/canonical-monorepo). Do not add application code, package manifests, releases, or deployment work here.**

`canonical.cloud` is a historical copied snapshot, not the Canonical Cloud
monorepo. Most directories in this repository are ordinary tracked copies of
other repositories; editing them does not update their real upstream source and
creates silent divergence.

## Active source topology

The active superproject is `canonical-monorepo`:

- deployable application and service repositories are real Git submodules under
  `canonical-monorepo/apps/`;
- reusable source dependencies such as `canonical-interfaces`,
  `canonical-lib`, and `canonical-clients` are declared through Zed package
  manifests where appropriate;
- `canonical-cli` consumes `canonical-lib` and `canonical-clients` through Zed;
- this legacy mirror intentionally has no `.zpkg.toml` and must not become a
  package.

Current source repositories include:

```text
canonical-monorepo
canonical-api-server.rs
canonical-web-server.rs
canonical-marketing-site.web
canonical-mcp-server.rs
canonical-interfaces
canonical-lib
canonical-clients
canonical-cli
```

Clone and work in those repositories directly. A typical local layout is:

```text
~/codes/canonical-cloud/canonical-monorepo/
~/codes/canonical-cloud/canonical-api-server.rs/
~/codes/canonical-cloud/canonical-web-server.rs/
~/codes/canonical-cloud/canonical-marketing-site.web/
~/codes/canonical-cloud/canonical-mcp-server.rs/
~/codes/canonical-cloud/canonical-interfaces/
~/codes/canonical-cloud/canonical-lib/
~/codes/canonical-cloud/canonical-clients/
~/codes/canonical-cloud/canonical-cli/
~/codes/canonical-cloud/canonical.cloud/   # legacy mirror only
```

## Deployment migration note

Any deployment or infrastructure reference that still checks out
`canonical.cloud` should be migrated to `canonical-monorepo` and its real
`apps/` gitlinks. Until that migration is complete, treat this repository as a
read-only compatibility snapshot.
