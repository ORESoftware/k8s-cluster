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
