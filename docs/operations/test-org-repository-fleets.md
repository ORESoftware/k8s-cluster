# Test organization repository fleets

This publisher maintains the bounded test-repository program for the 18 production/test organization pairs listed under `config/test_org_repository_fleets/`. The `r2g` accounts are explicitly excluded.

## Bounded scope

- 18 test organizations.
- 209 focused test repositories.
- 18 public organization `.github` catalog/governance repositories.
- 227 total managed repositories.
- The existing 22-repository `zed-pkg-test` baseline is preserved; 13 complementary fixtures are added.

The configuration is the only repository allowlist. Validation rejects missing or extra organizations, duplicate repositories, unsafe names, unknown source references, an incorrect repository count, and any `r2g` source or target.

## Repository contract

Each focused repository receives, through a pull request:

- `test-fleet.json` with its purpose, language/platform matrix, services, fixtures, and acceptance suites;
- `source-pins.json` with the production repository, role, default branch, and immutable source commit;
- `.gitmodules` plus requested gitlinks for available production sources;
- `.zpkg.toml` dependencies for SDK/library/interface sources that are intended to be consumed through Zed;
- a native package-manager metadata probe;
- a deterministic executable test plan;
- structural and manually dispatched integration workflows;
- local agent, security, synthetic-fixture, and conflict-resolution policy.

GitHub's Trees API can reject gitlinks whose commit objects belong to a different private repository. The publisher first attempts the real gitlinks. If GitHub rejects them, the committed materializer clones the source and checks out the exact SHA in `source-pins.json`; it never substitutes a branch head or a similarly named package.

## Publication behavior

The publisher is dry-run by default and mutates only when `--execute` is supplied by the trusted main-branch workflow. It is idempotent:

1. Resolve and pin available production sources.
2. Create or reuse each allowlisted repository.
3. Preserve the existing default-branch tree.
4. Build one inline Git tree per repository to stay within API limits.
5. Create a deterministic automation branch.
6. Open a pull request.
7. Publish an exact-head `test-fleet/bootstrap-validation` commit status only after the rendered tree passes bounded local validation.
8. Require at least one exact-head commit status/check and require every observed status/check to be successful before squash-merging; otherwise leave the PR open.
9. Merge only the SHA that was validated and gated, then verify the managed digest on the default branch or the existence of the committed open PR.

The generated contract digest includes the renderer version, repository specification, and all source pins. A production source revision change therefore creates a new reviewable bootstrap/update branch rather than being hidden behind an unchanged marker.

## Test domains

The matrix includes native SDK consumers, API/OpenAPI and websocket contracts, browser automation, Flutter emulators, desktop operating systems, PostgreSQL and CockroachDB rollback, distributed consensus and routing faults, NATS/DLQ behavior, offline synchronization, media and clipboard interoperability, accessibility, security, privacy, load, chaos, and package-manager parity.

All identity, message, audio, image, and media fixtures must be synthetic. Live credentials, user conversations, biometrics, customer data, and production exports are prohibited.

## Reruns

Re-run the `Publish test organization repository fleets` workflow from the trusted `main` branch. Completed repositories are recognized by their digest and skipped. Open bootstrap PRs are reused. Failed or rate-limited entries can therefore resume without recreating already verified work.
