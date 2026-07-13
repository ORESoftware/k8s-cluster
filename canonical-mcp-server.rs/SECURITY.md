# Security policy

## Reporting

Report suspected vulnerabilities privately — do **not** open a public issue for
anything exploitable. Use GitHub's private "Report a vulnerability" flow on this
repo. Include the affected commit and a minimal reproduction.

## Scope

This is local developer/ops tooling served over stdio; it binds no ports and
stores nothing. It makes outbound HTTPS requests to `api.github.com`,
`raw.githubusercontent.com`, `rdap.org` (plus the registry RDAP endpoint it
redirects to), `cloudflare-dns.com`, `api.cloudflare.com`, and the `base_url`
given to `service_health`. `k8s_status` execs a strictly allowlisted
`kubectl get` with your local kubeconfig; every tool is read-only.

## Secrets

Never commit real secrets. The credentials this server touches are an optional
GitHub token (`GITHUB_TOKEN`/`GH_TOKEN`, sent only to `api.github.com`) and an
optional read-only Cloudflare token (`CLOUDFLARE_API_TOKEN`, sent only to
`api.cloudflare.com`); neither is ever logged or echoed into tool output.

## CI supply chain

GitHub Actions are pinned to commit SHAs; workflows run with least-privilege
`permissions: contents: read`. Dependabot tracks the action and crate
dependencies weekly. CI pins `cargo-audit` and denies both vulnerabilities and
informational warnings.
