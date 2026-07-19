# Mobile and Flutter release-set CI

Last reviewed: July 18, 2026.

Each Flutter app repository owns its full build matrix and release artifacts.
The monorepo adds a second responsibility: prove that the exact submodule SHAs
forming a release set still work together.

## Workflows

- `integration` initializes every submodule and checks shared interface/proxy
  invariants.
- `pinned Flutter mobile build` initializes only the public mobile app pin,
  runs Android verification, and compiles unsigned iOS on macOS 15.
- `pinned Flutter console build` initializes only the private console pin,
  builds web, and runs both Puppeteer and Playwright.

Full signed Android/iOS releases and Linux/macOS/Windows desktop builds remain in
the app repositories so a monorepo pin cannot unexpectedly publish anything.

## Private-submodule credential

GitHub's repository token cannot read private sibling repositories. Add
`SONUS_SUBMODULE_TOKEN` as a monorepo Actions secret with read-only Contents
access to only the required Sonus repositories. Prefer a GitHub App that mints a
short-lived installation token; a narrowly scoped fine-grained machine-user PAT
is an interim option. Never paste a broad developer CLI token into Actions.

Once the token exists, require `integration` and both pinned Flutter workflows
before merging a release-set change.

## Promotion

- App stores: manually dispatch the app repo's protected signed job, inspect the
  artifact/checksum, then upload to Play internal testing or TestFlight.
- Flutter web: update the exact console revision in the Kubernetes Argo manifests
  and let Argo reconcile.
- Backend: update its cluster pin/manifests and let Argo reconcile.

No monorepo workflow writes directly to the production cluster or store.
