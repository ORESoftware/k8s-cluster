# Local IDE access to the dd cluster MCP servers

This directory holds the checked-in config and helper scripts for connecting
local IDEs — **Cursor, VS Code, and Codex** — to the two read-only MCP servers
running on the EC2 Kubernetes runtime:

| Server                                                            | Language | Gateway URL                        |
| ----------------------------------------------------------------- | -------- | ---------------------------------- |
| [`dd-gleam-mcp-server`](../remote/deployments/gleam-mcp-server/)  | Gleam    | `https://98.90.186.114/mcp`         |
| [`dd-cluster-mcp-rs`](../remote/deployments/cluster-mcp-rs/)      | Rust     | `https://98.90.186.114/cluster-mcp` |

Both expose the same read-only `dd_cluster` tool surface (cluster inventory,
service directory, observability health), reached **through the
`dd-remote-gateway`**, never directly.

> **Gateway IP.** The gateway is the EC2 Elastic IP `98.90.186.114`. If the
> instance is rebuilt and the EIP changes, update the URLs in `.cursor/mcp.json`,
> `.vscode/mcp.json`, this file, and `codex-config.example.toml`. Discover the
> current IP with:
>
> ```sh
> aws ec2 describe-instances --region us-east-1 \
>   --filters Name=tag:Name,Values=dd-remote-k8s-1 Name=instance-state-name,Values=running \
>   --query 'Reservations[].Instances[].PublicIpAddress' --output text
> ```
>
> When the IP changes you must also **reissue the TLS cert for the new IP** (see
> "TLS" below) or clients will fail certificate validation.

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
local shell / IDE secret storage only. The current token is stored locally at
`~/.dd-mcp-token` (mode 600).

### Mint / rotate the token (operator)

```sh
TOKEN=$(openssl rand -hex 32)
aws secretsmanager put-secret-value \
  --secret-id dd/remote-dev/mcp-gateway-token \
  --secret-string "{\"MCP_READONLY_TOKEN\":\"$TOKEN\"}"
kubectl -n default rollout restart deploy/dd-remote-gateway   # picks up new value
```

Rotation invalidates old IDE configs (they fail closed with `401`).

## TLS

The gateway serves a **publicly-trusted Let's Encrypt IP certificate** for
`98.90.186.114` (short-lived, auto-renewed on the EC2 host by
[`remote/ec2/renew-letsencrypt-gateway-cert.sh`](../remote/ec2/renew-letsencrypt-gateway-cert.sh)).
Because it's publicly trusted, **clients need no custom CA** — full TLS
verification works out of the box.

If the EIP changes, the cert must be reissued for the new IP. On the EC2 host
(SSM Session Manager works; no VPN required):

```sh
/home/ec2-user/certbot-venv-312/bin/certbot certonly \
  --config-dir /home/ec2-user/letsencrypt/config \
  --work-dir /home/ec2-user/letsencrypt/work \
  --logs-dir /home/ec2-user/letsencrypt/logs \
  --preferred-profile shortlived --webroot \
  --webroot-path /home/ec2-user/dd-acme-webroot \
  --ip-address <NEW_IP> --agree-tos --register-unsafely-without-email --non-interactive
# then deploy + roll the gateway:
CERT_NAME=<NEW_IP> remote/ec2/renew-letsencrypt-gateway-cert.sh deploy
```

`fetch-gateway-ca.sh` remains only as a fallback for a future self-signed cert;
it is **not needed** with the current LE cert.

## Per-IDE setup

### Cursor — [`.cursor/mcp.json`](../.cursor/mcp.json) (committed)

Native HTTP. Cursor expands `${env:DD_MCP_TOKEN}` from the environment it was
launched with. On macOS, set it for GUI apps and relaunch Cursor:

```sh
launchctl setenv DD_MCP_TOKEN "$(cat ~/.dd-mcp-token)"
```

### VS Code — [`.vscode/mcp.json`](../.vscode/mcp.json) (committed)

Native HTTP with a prompted `input`. VS Code asks for the token once on first
use and stores it in its secret storage — nothing to export. Run **MCP: List
Servers** and start them.

### Codex — `~/.codex/config.toml`

Codex supports native HTTP MCP servers with `http_headers`. Copy the two blocks
from [`codex-config.example.toml`](./codex-config.example.toml) into
`~/.codex/config.toml` and set the bearer token. (Your `~/.codex/config.toml` is
local-only, so a literal token there is fine.)

## Smoke test (no IDE)

```sh
curl -H "Authorization: Bearer $(cat ~/.dd-mcp-token)" \
  -H 'content-type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/list"}' \
  https://98.90.186.114/cluster-mcp
```

A `200` with a JSON `result.tools` array means the token is good. A `401` means
the token is wrong or the gateway hasn't been restarted since a rotation.

## Reachability

The gateway EIP is reachable over the public internet (subject to the instance
security group). If `curl` hangs or returns `000`, you are either off an allowed
network or the EIP has changed — re-run the discovery command above.

## Security notes / residual findings

- These tools are **read-only** (k8s metadata, service directory, observability
  health). Anyone with the token can enumerate cluster topology, so treat it as
  a real (if low-blast-radius) credential and rotate on offboarding.
- The MCP server pods still accept **unauthenticated in-VPC traffic** on
  `:8090` / `:8091` via the RFC1918 NetworkPolicy ingress rule added for
  host-network warm workers. Local IDEs do not use that path (they go through
  the authenticated gateway), but in-VPC callers bypass the token.
- Redaction is substring-based on known secret-like keys; bounded samples of
  arbitrary object metadata still flow out. Keep the surface read-only — do not
  add write/secret/log/exec tools without a separate short-lived human grant +
  audit design (see each server's `readme.md`).
