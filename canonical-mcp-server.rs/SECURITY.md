# Security policy

## Reporting

Report suspected vulnerabilities privately — do **not** open a public issue for
anything exploitable. Use GitHub's private "Report a vulnerability" flow on this
repo. Include the affected commit and a minimal reproduction.

## Scope

This is local developer/ops tooling served over stdio; it binds no ports and
stores nothing. It makes outbound HTTPS requests to `api.github.com`,
`raw.githubusercontent.com`, and the `base_url` given to `service_health`.

## Secrets

Never commit real secrets. The only credential this server touches is an
optional GitHub token read from `GITHUB_TOKEN`/`GH_TOKEN` at runtime; it is
sent only to `api.github.com` and is never logged or echoed into tool output.

## CI supply chain

GitHub Actions are pinned to commit SHAs; workflows run with least-privilege
`permissions: contents: read`. Dependabot tracks the action and crate
dependencies weekly. CI pins `cargo-audit` and denies both vulnerabilities and
informational warnings.
