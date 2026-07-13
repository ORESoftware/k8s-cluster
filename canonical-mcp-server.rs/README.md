# canonical-mcp-server.rs

An [MCP](https://modelcontextprotocol.io) (Model Context Protocol) server for
operating the **[canonical.cloud](https://canonical.cloud)** stack (GitHub org
[`canonical-cloud`](https://github.com/canonical-cloud)). It is developer/ops
tooling, not a deployed app: it runs locally over stdio and gives an MCP client
(such as Claude Code) read-only visibility into CI, monorepo submodule pins,
deployment health, and the stack's operational docs.

Built on the official Rust MCP SDK
([`rmcp`](https://github.com/modelcontextprotocol/rust-sdk)) with a tokio
runtime and reqwest (rustls, no OpenSSL).

## Tools

| Tool | Parameters | Purpose |
| --- | --- | --- |
| `stack_ci_status` | `repo` (optional) | Latest five GitHub Actions runs per stack repo: branch, status, conclusion, workflow, run URL, timestamp |
| `submodule_pins` | — | Compare `canonical-monorepo`'s `apps/` submodule pins against each app repo's `main` HEAD: pinned SHA, HEAD SHA, current?, commits behind |
| `service_health` | `base_url` | Probe `{base}/healthz`, `{base}/readyz`, `{base}/api/v1/health` with a short timeout; return status codes and truncated bodies |
| `stack_docs` | `doc`: `deploy` \| `repo-boundaries` | Fetch `docs/deploy.md` or `docs/repo-boundaries.md` from `canonical-monorepo` as raw markdown |
| `domain_status` | `domain` (default `canonical.cloud`) | Registrar-side state via public RDAP (registrar, status codes, registration/expiration events, delegated nameservers — Squarespace exposes no public domains API, so RDAP is the registrar integration) plus live NS/A/AAAA via DNS-over-HTTPS and whether delegation points at Cloudflare |
| `cloudflare_dns` | `domain` (default `canonical.cloud`) | List a Cloudflare zone's DNS records (type, name, content, proxied, TTL). Read-only; needs `CLOUDFLARE_API_TOKEN` |
| `k8s_status` | `resource`: `nodes` \| `pods` \| `deployments` \| `services` \| `ingresses`; `namespace`, `context` (optional) | Read-only cluster state via allowlisted `kubectl get … -o json`, summarized to name/namespace/status/age rows. Never mutates the cluster |

The stack repositories covered by `stack_ci_status`:
`canonical-monorepo`, `canonical-web-server.rs`,
`canonical-marketing-site.web`, `canonical-interfaces`.

## Running

```sh
cargo run
```

The server speaks MCP over stdin/stdout; it is meant to be launched by an MCP
client, not used interactively.

### Register in Claude Code

From a checkout, using the debug build via cargo:

```sh
claude mcp add canonical-mcp -- cargo run \
  --manifest-path /path/to/canonical-mcp-server.rs/Cargo.toml
```

Or build once and register the release binary:

```sh
cargo build --release
claude mcp add canonical-mcp -- \
  /path/to/canonical-mcp-server.rs/target/release/canonical-mcp-server
```

## Environment

| Variable | Required | Purpose |
| --- | --- | --- |
| `GITHUB_TOKEN` (or `GH_TOKEN`) | no | Bearer token for GitHub API calls. Unauthenticated works but is rate-limited to 60 requests/hour per IP. |
| `CLOUDFLARE_API_TOKEN` | for `cloudflare_dns` | Read-only Cloudflare token (Zone.Zone:Read, Zone.DNS:Read). |
| `KUBECONFIG` / kubeconfig | for `k8s_status` | `k8s_status` shells out to `kubectl` on `PATH` and uses your normal kubeconfig/contexts. |

The server makes outbound HTTPS requests only — to `api.github.com`,
`raw.githubusercontent.com`, `rdap.org` (and the registry RDAP endpoint it
redirects to), `cloudflare-dns.com`, `api.cloudflare.com`, and whatever
`base_url` you pass to `service_health`. Every tool is read-only by design;
there are deliberately no write-capable Cloudflare, GitHub, or Kubernetes
tools (matching the read-only MCP contract used across the org's ops repos).

## Layout

- `src/main.rs` — bootstrap only; serves the handler over stdio.
- `src/server.rs` — tool router, parameter schemas, `ServerHandler`.
- `src/tools/github.rs` — GitHub client plus pure JSON summarization
  (CI runs, `.gitmodules` parsing, pin comparison).
- `src/tools/health.rs` — endpoint probing and body truncation.
- `src/tools/docs.rs` — monorepo doc fetching.
- `src/tools/domain.rs` — RDAP + DNS-over-HTTPS summarization and domain
  validation.
- `src/tools/cloudflare.rs` — Cloudflare zone/record listing.
- `src/tools/k8s.rs` — allowlisted `kubectl get` runner and per-resource
  summarizers.

Network access is confined to the thin client/orchestration functions; all
response interpretation is pure functions over fixture-testable JSON.

## Development

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
```

The Nix dev shell mirrors the sibling repos: `./shell` drops you into it
(requires Nix with flakes).
