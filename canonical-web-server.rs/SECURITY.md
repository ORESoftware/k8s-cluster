# Security policy

## Reporting

Report suspected vulnerabilities privately — do **not** open a public issue for
anything exploitable. Use GitHub's private "Report a vulnerability" flow on this
repo. Include the affected commit and a minimal reproduction.

## Secrets

Never commit real secrets. Only `.env.example` (placeholder values) is tracked;
everything matching `.env*` is gitignored. If a secret is committed, treat it as
compromised: rotate it first, then scrub history.

## CI supply chain

GitHub Actions are pinned to commit SHAs; workflows run with least-privilege
`permissions: contents: read`. Dependabot tracks the action, package, and crate
dependencies weekly.

CI pins `cargo-audit` and denies both vulnerabilities and informational
warnings, with two reviewed exceptions:

- `RUSTSEC-2023-0071`: `rsa` is recorded only through SQLx's disabled optional
  MySQL support. This server enables PostgreSQL and SQLite; CI separately fails
  if `rsa` ever appears in the active Cargo dependency graph.
- `RUSTSEC-2026-0173`: the unmaintained `proc-macro-error2` crate is a
  compile-time-only transitive dependency of SeaORM's derive macros. Remove the
  exception when SeaORM replaces it.
