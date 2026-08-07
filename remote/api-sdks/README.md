# Fleet API SDKs

These packages are generated from the exact OpenAPI 3.1 artifacts indexed by
`remote/deployments/generated-api-docs-index.json`. Never edit generated package files by hand.

## Packages

Eight packages are produced: public and internal variants for TypeScript, Rust, Dart, and Gleam.
Public packages contain only the fail-closed runtime contract. Internal packages contain every
available operation and are intended for trusted service-to-service callers.

All request builders reject unknown parameters, require declared path/query values, enforce request
body presence, and percent-encode path and query values. The root `sdk-lock.json` records the
SHA-256 digest of every source OpenAPI document, both scope catalogs, every package manifest, and the
generator itself.

Current generated coverage:

- public: 44 services / 279 operations
- internal: 44 services / 943 operations
- public: 43 services / 274 operations
- internal: 43 services / 914 operations
- temporarily unavailable deployment gitlinks: 5

## Temporary gitlink exclusions

The generator fails for missing normal deployment artifacts. It may skip only an uninitialized Git
gitlink, and the exact upstream repository is recorded below. These services must be migrated in their
source repositories and then their parent gitlinks must be bumped.

| Service | Language | Source repository | Reason |
|---|---|---|---|
| `billing-server-rs` | `rust` | `git@github.com:quaestor-ledger/billing-server.rs.git` | uninitialized-deployment-gitlink |
| `dart-server` | `dart` | `git@github.com:sagitta-stack/dart-server.git` | uninitialized-deployment-gitlink |
| `fiducia-customer.rs` | `rust` | `https://github.com/fiducia-cloud/fiducia-customer.rs.git` | uninitialized-deployment-gitlink |
| `gleam-lambda-runner` | `gleam` | `git@github.com:scintilla-run/gleam-lambda-runner.git` | uninitialized-deployment-gitlink |
| `usacc-rest-api-backend-rs` | `rust` | `git@github.com:usa-acc/rest-api-backend.rs.git` | uninitialized-deployment-gitlink |

## Commands

```bash
node remote/tools/generate-api-sdks.mjs
node remote/tools/generate-api-sdks.mjs --check
node remote/tools/validate-api-sdks.mjs
```

The generated SDKs currently provide strongly synchronized operation catalogs and dependency-light
request builders. Request and response models become richer automatically as each server migrates
from the compatibility scanner to its native typed OpenAPI adapter.
