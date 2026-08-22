# zed-pkg account and registry rollout

## Current state

`dd-zed-account-registry-candidate` is a review-only Argo CD Application. It pins one exact `zed-pkg/zed-infra` commit, has no automated sync, and sets `FailOnSharedResource=true`. The two disposable bootstrap Applications remain active while the production account/registry stack is certified.

The candidate must not be synced while its rendered images contain `replace-with-git-sha`, while required Secrets are absent, or while the bootstrap Applications own overlapping resources.

## Ownership and data planes

The production design separates four responsibilities:

1. **Registry PostgreSQL/RDS** owns users projected from Shared Auth, organizations, memberships and invitations, projects, packages, versions, licenses, embeddings, upload facts, download facts, API tokens, and audit events.
2. **Shared Auth PostgreSQL/RDS** owns customer sessions and Shared Auth state. The registry API never receives this database DSN.
3. **Supabase Auth** is the identity provider used by Shared Auth for signup/login and token verification. The browser receives only the publishable Supabase configuration; service-role material never enters the web pod.
4. **Cloudflare R2** owns immutable package bytes. PostgreSQL remains authoritative for metadata, authorization, hashes, storage keys, and download accounting.

The reviewed SQL contract lives in `ORESoftware/k8s-libs-and-shared-defs/pg-defs/schema/orgs/zed-pkg`. `zed-pkg/zed-lib-core` is the shared Zed package and SeaORM boundary. Long-running API and web processes do not author ad hoc DDL; an Argo `PreSync` migration Job applies the reviewed contract before the API rollout.

## Public HTTP contract

The target canonical hierarchy is:

```text
/api/v1/auth/*                     Shared Auth/Supabase exchange and public auth config
/api/v1/users/me                   signed-in user settings
/api/v1/home                       signed-in home/dashboard aggregate
/api/v1/search                     cross-entity product search
/api/v1/orgs/*                     org dashboards, settings, memberships and invitations
/api/v1/projects/*                 project settings, membership and package creation
/api/v1/registry/*                 registry protocol and package-specific operations
/api/v1/registry/packages/*        package settings, versions, licenses, uploads and downloads
```

The registry is deliberately a subset of the broader product API, not a separate top-level product. During migration, existing `/v1/*` callers may remain as compatibility aliases; new web and SDK code should use `/api/v1`, with package protocol operations under `/api/v1/registry`.

The public hosts are:

- `https://app.zpkg.net` → Maud/HTMX web server;
- `https://api.zpkg.net` → Axum API server.

R2 objects use an immutable, content-addressed key shape:

```text
zed/v1/packages/{org}/{package}/{version}/{sha256}.{extension}
```

A download is authorized and recorded in PostgreSQL before the API returns a short-lived signed R2 redirect. Package metadata never trusts an object-store listing as its source of truth.

## Web surface

The Maud/HTMX application provides:

- signup/login and Shared Auth callback handling;
- home with the user's organizations plus project/package search;
- organization dashboard and settings;
- project settings;
- package settings, license management, upload state, and zip/tarball downloads;
- user settings;
- a static, context-aware header whose create menus are constrained by the current organization/project/package context.

All mutations go through the API service. The web tier receives no migrator credential and no R2 write credential.

## Package visibility invariant

A private package may become public only while both conditions remain true:

- its age is no more than 10 days;
- its committed download count is no more than 50.

The boundary values themselves are allowed: exactly 10 days and exactly 50 downloads may still promote. The database trigger is the final enforcement point, including concurrent promotion/download races; the shared ORM pre-check exists only to return a useful conflict response before the trigger fires.

## Argo ordering

The combined runtime uses these waves:

1. `zed-pkg-registry-migrate` — `PreSync`, wave `-10`;
2. `zed-api-server` — wave `0`, `AUTO_MIGRATE=false`;
3. `zed-web-server` — wave `10`.

The migration Job and API Deployment must use the same immutable API image digest. A migration failure blocks both long-running services from advancing.

## Required secret contracts

The rendered manifests reference, but never define, these Secrets in namespace `zed`:

| Secret | Purpose |
| --- | --- |
| `zed-pkg-registry-db` | role-scoped registry/migrator database connection material |
| `zed-pkg-shared-auth` | Shared Auth service URL, public URL, audience/application binding, and service credential |
| `zed-pkg-artifact-storage` | R2/S3 endpoint, region, bucket and scoped object credentials |
| `zed-pkg-web-auth` | Supabase URL and publishable browser key |
| `zed-pkg-web-runtime` | `PUBLIC_BASE_URL`, secure cookie/domain settings and other public runtime configuration |

Production database identities remain distinct: API read/write without broad DDL, web read-only where direct approved reads are retained, and a discrete migrator identity.

## Activation gates

Before any ownership handoff:

1. the exact `zed-lib-core`, API, and web heads are green under formatting, Clippy, tests and contract/e2e lanes;
2. API and web call the same canonical route contract, including project/package reads and invitation acceptance;
3. the zed-infra overlay pins immutable image digests and no candidate tags remain;
4. registry RDS has `pgcrypto` and `vector`, role grants are verified, and the migration identity is separate from runtime identities;
5. Shared Auth RDS, Supabase project configuration, R2 bucket/credentials and all ExternalSecrets are present;
6. `app.zpkg.net` and `api.zpkg.net` resolve to nginx ingress and cert-manager has issued `zed-pkg-public-tls`;
7. signup/login, revocation, CSRF/origin checks, membership isolation, package promotion boundaries, upload integrity and signed downloads pass against the exact release images;
8. the candidate render and both cluster roots pass `Zed bootstrap contract` CI.

## Explicit Argo ownership handoff

Do not resolve shared-resource warnings by disabling `FailOnSharedResource` in isolation. The activation change must be reviewed as a separate operational commit:

1. disable automated prune/self-heal on the two bootstrap Applications;
2. confirm no bootstrap sync is running and take a live resource inventory;
3. orphan the bootstrap Applications without cascading resource deletion;
4. verify the live objects remain healthy, then remove their stale Argo tracking ownership;
5. update the candidate to the exact digest-pinned zed-infra commit;
6. manually sync the candidate and verify migration, API, web, ingress, sessions and R2 behavior;
7. only after successful adoption, remove the obsolete bootstrap Application manifests and enable the reviewed automated policy.

This ordering keeps rollback explicit and prevents a source merge from being mistaken for a live deployment.
