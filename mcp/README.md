# Local IDE access to the dd cluster MCP servers

This directory holds the checked-in config and helper scripts for connecting
local IDEs — **Cursor, VS Code, and Codex** — to the two read-only MCP servers
running on the EC2 Kubernetes runtime:

| Server                                                                    | Language | Gateway URL                       |
| ------------------------------------------------------------------------- | -------- | --------------------------------- |
| [`dd-gleam-mcp-server`](../remote/deployments/gleam-mcp-server/)          | Gleam    | `https://54.91.17.58/mcp`         |
| [`dd-cluster-mcp-rs`](../remote/deployments/cluster-mcp-rs/)              | Rust     | `https://54.91.17.58/cluster-mcp` |

Both expose the same read-only `dd_cluster` tool surface (cluster inventory,
service directory, observability health). They are reached **through the
`dd-remote-gateway`**, never directly.

## Auth model

The MCP server pods have **no app-level auth** — authorization is enforced at
the gateway. Local IDE clients authenticate with a **dedicated, read-only MCP
bearer token**, separate from the master operator cookie value, so an IDE never
holds a credential to the rest of the operator surface (headlamp, grafana,
prometheus, bastion, agents, runtime-config).

The gateway accepts, on `/mcp` and `/cluster-mcp` only, **either**:

- the operator auth (`Auth` header / `dd_auth` cookie) — the human browser path, or
- `Authorization: Bearer <MCP_READONLY_TOKEN>` — the IDE path.

Token plumbing:

- AWS Secrets Manager: `dd/remote-dev/mcp-gateway-token` → JSON key `MCP_READONLY_TOKEN`.
- ExternalSecret → k8s secret `dd-mcp-gateway-token` (see
  [`remote/argocd/secrets/external-secrets.yaml`](../remote/argocd/secrets/external-secrets.yaml)).
- Gateway Deployment reads it as `DD_MCP_READONLY_TOKEN` and validates it in
  [`dd-remote-gateway.configmap.yaml`](../remote/argocd/dd-next-runtime/dd-remote-gateway.configmap.yaml)
  (`$dd_mcp_bearer_ok` / `$dd_mcp_auth_ok`).

**The token is never committed.** It lives in AWS, the cluster secret, and your
local shell / IDE secret storage only.

### One-time: mint and deploy the token (operator)

```sh
# 1. Generate a token and write the AWS secret (JSON key MCP_READONLY_TOKEN).
TOKEN=$(openssl rand -hex 32)
aws secretsmanager create-secret \
  --name dd/remote-dev/mcp-gateway-token \
  --secret-string "{\"MCP_READONLY_TOKEN\":\"$TOKEN\"}"
#   (use `put-secret-value` to rotate an existing secret)

# 2. Let External Secrets sync, then restart the gateway so it loads the env.
kubectl -n default rollout restart deploy/dd-remote-gateway

# 3. Share $TOKEN with operators out-of-band; they set it locally (below).
```

Rotation = write a new AWS version + `rollout restart deploy/dd-remote-gateway`.
Old IDE configs then fail closed with `401`.

## One-time: trust the gateway TLS cert

The gateway serves `https://54.91.17.58` with a self-signed / short-lived IP
cert, so Node-based MCP clients reject it by default. Pin the cert (keeps full
TLS verification — do **not** use `NODE_TLS_REJECT_UNAUTHORIZED=0`):

```sh
./mcp/fetch-gateway-ca.sh        # writes mcp/dd-gateway-ca.pem (gitignored)
```

Then export `NODE_EXTRA_CA_CERTS` pointing at that file:

- **Codex** (stdio bridge): already wired per-server in
  [`codex-config.example.toml`](./codex-config.example.toml) — just set the path.
- **Cursor / VS Code** (native HTTP): these run as GUI apps, so set it in the
  app's launch environment. On macOS:

  ```sh
  launchctl setenv NODE_EXTRA_CA_CERTS "$PWD/mcp/dd-gateway-ca.pem"
  # then fully quit and relaunch Cursor / VS Code
  ```

  (If the gateway is later moved to a publicly-trusted cert, skip this entirely.)

## Per-IDE setup

### Cursor — [`.cursor/mcp.json`](../.cursor/mcp.json) (committed)

Native HTTP. Cursor expands `${env:DD_MCP_TOKEN}` from the environment it was
launched with. Set the token, then relaunch Cursor:

```sh
launchctl setenv DD_MCP_TOKEN "<the token>"   # macOS GUI apps
```

### VS Code — [`.vscode/mcp.json`](../.vscode/mcp.json) (committed)

Native HTTP with a prompted `input`. VS Code asks for the token once on first
use and stores it in its secret storage — nothing to export. Open the file and
click **Start**, or run **MCP: List Servers**.

### Codex — `~/.codex/config.toml`

Codex reaches the servers through `npx mcp-remote`. Copy the blocks from
[`codex-config.example.toml`](./codex-config.example.toml) into
`~/.codex/config.toml`, set `AUTH_HEADER = "Bearer <the token>"` and the
`NODE_EXTRA_CA_CERTS` absolute path. (Your `~/.codex/config.toml` is local-only,
so the literal token there is acceptable.)

## Smoke test (no IDE)

```sh
TOKEN="<the token>"
curl --cacert mcp/dd-gateway-ca.pem \
  -H "Authorization: Bearer $TOKEN" \
  -H 'content-type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/list"}' \
  https://54.91.17.58/cluster-mcp
```

A `200` with a JSON `result.tools` array means the token + CA are good. A `401`
(or a redirect to `/auth`) means the token is wrong or the gateway hasn't been
restarted since the secret landed.

## Security notes / residual findings

- These tools are **read-only** (k8s metadata, service directory, observability
  health). Anyone with the token can enumerate cluster topology, so treat it as
  a real (if low-blast-radius) credential and rotate on offboarding.
- The MCP server pods still accept **unauthenticated in-VPC traffic** on
  `:8090` / `:8091` via the RFC1918 NetworkPolicy ingress rule added for
  host-network warm workers. Local IDEs do not use that path (they go through
  the authenticated gateway), but in-VPC callers bypass the token. Tighten to
  specific pool-node IPs when that rule is revisited.
- Redaction is substring-based on known secret-like keys; bounded samples of
  arbitrary object metadata still flow out. Keep the surface read-only — do not
  add write/secret/log/exec tools without a separate short-lived human grant +
  audit design (see each server's `readme.md`).
