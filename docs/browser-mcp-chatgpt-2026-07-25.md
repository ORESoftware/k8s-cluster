# browser-mcp-rs — ChatGPT MCP server verification, 2026-07-25

> Historical verification record. The hostPath/auth/ArgoCD findings below
> describe the pre-hardening deployment. The current image-based multi-cloud
> design, anonymous-domain ceiling, rollout procedure, and residual risks are in
> [`browser-mcp-production-hardening-2026-07-25.md`](browser-mcp-production-hardening-2026-07-25.md).

`remote/deployments/browser-mcp-rs` is the public MCP server that lets **ChatGPT**
(or Claude / any MCP client) drive a real browser through two model-callable
tools — `browser_state` (read-only) and `browser_act` (write-capable) — while
stopping before CAPTCHA / MFA / payment / signatures / final submissions. It
speaks MCP-over-HTTP (JSON-RPC 2.0 at `/mcp`) and proxies validated calls to the
private `dd-web-scraper` `/agent/*` (Playwright), which can bridge to
`dd-selenium-server`.

```
  ChatGPT ── HTTPS, MCP-over-HTTP ──▶ dd-remote-gateway (/browser-mcp, public)
                                        └─▶ dd-browser-mcp-rs :8092  (JSON-RPC /mcp)
                                              └─ X-Server-Auth ─▶ dd-web-scraper :8097 /agent/*
                                                                    └─▶ Playwright (+ selenium bridge)
```

## What was verified (this pass)

Built and ran the server locally, pointed at the deployed `dd-web-scraper` (via
`kubectl port-forward`), and exercised the full path an MCP client uses.

- **Protocol handshake** — `initialize` returns the negotiated `protocolVersion`,
  `capabilities.tools`, and the safety instructions (prompt-injection notice +
  human-gate for CAPTCHA/MFA/payment/signatures). `notifications/initialized` →
  `202`. `tools/list` → exactly `browser_act` + `browser_state`.
- **Full tool call, end-to-end** — `tools/call browser_act` with
  `{intent, actions:[{type:"start"},{type:"goto","url":"https://example.com"}]}`
  returned `isError:false`, `page.title:"Example Domain"`,
  `page.url:"https://example.com/"`, a `session_id`, and `revision:2`. i.e. the
  chain **MCP client → browser-mcp-rs → web-scraper /agent/act → Playwright →
  example.com** works. `readyz` confirms the server gates on the worker's
  `/agent/healthz`.
- **Auth** — with `BROWSER_MCP_REQUIRE_AUTH=true` the `/mcp` surface requires the
  bearer; the worker leg is always `X-Server-Auth`-gated.

## Transport fix for ChatGPT (Streamable HTTP)

ChatGPT's remote-MCP client uses the **Streamable HTTP** transport and, to open
the standalone server→client channel, issues `GET /mcp` with
`Accept: text/event-stream`. The server previously answered that with a **200 +
JSON descriptor**, which is a spec violation (the MCP spec requires **405** when a
server offers no SSE stream) and can hang a strict client.

Fixed in `src/main.rs`: `GET /mcp` now content-negotiates —
`Accept: text/event-stream` → **405** (`Allow: POST`); a plain `GET` still returns
the JSON descriptor. This server sends no server-initiated notifications, so all
traffic correctly flows over `POST /mcp` (which returns `application/json`, a
Streamable-HTTP-valid response ChatGPT accepts). Added the crate's first tests
(6): the `wants_event_stream` decision, the `initialize`/`tools/list` shape,
`browser_act` required args, and the JSON-RPC error envelope.

Verified live after the fix:

| Request | Result |
|---|---|
| `GET /mcp` `Accept: text/event-stream` | **405** (`Allow: POST`) |
| `GET /mcp` `Accept: application/json` | 200 (JSON descriptor) |
| `POST /mcp` `initialize` (`Accept: application/json, text/event-stream`) | 200 |
| `POST /mcp` `notifications/initialized` | 202 |
| `POST /mcp` `tools/list` | `browser_act`, `browser_state` |

## Reproduce

```sh
CTX=dd-ec2-runtime
SECRET=$(kubectl --context $CTX -n default get secret dd-agent-secrets \
          -o jsonpath='{.data.SERVER_AUTH_SECRET}' | base64 -d)
kubectl --context $CTX -n default port-forward svc/dd-web-scraper 18097:8097 &
BROWSER_MCP_WORKER_URL=http://127.0.0.1:18097 SERVER_AUTH_SECRET="$SECRET" \
  HOST=127.0.0.1 PORT=18092 target/debug/dd-browser-mcp-rs &
curl -s -X POST localhost:18092/mcp -H 'content-type: application/json' -d '{
  "jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"browser_act",
  "arguments":{"intent":"smoke","actions":[{"type":"start"},
  {"type":"goto","url":"https://example.com"}]}}}'
```

## Deployment status — SUPERSEDED (was: "not running, 0 replicas")

**That is no longer true.** Verified live against `dd-ec2-runtime`:

```
dd-browser-mcp-rs    1/1   Running   0 restarts   (deployed, healthy)
dd-browser-mcp-rs-secrets   Opaque   1   ExternalSecret Ready=True "secret synced"
```

The Application is synced, the bearer secret is provisioned, the gateway
`/browser-mcp` locations exist, and `dd-web-scraper.networkpolicy.yaml` already
admits `app: dd-browser-mcp-rs`. **The end-to-end browser chain works in
production.** Driven through the public gateway with the bearer:

| Call | Result |
|---|---|
| `tools/list` | `browser_act`, `browser_state` |
| `browser_act` → `goto https://www.irs.gov/` | `isError:false`, title *"Internal Revenue Service \| An official website…"* |
| `browser_act` → `goto https://example.com/` | `"host example.com is not on the allowlist"` (policy working) |

So MCP → browser-mcp-rs → dd-web-scraper → Playwright → public web is **fully
functional**. What remains are three ChatGPT-facing config problems.

## The three blockers for ChatGPT (verified 2026-07-25)

### 1. Auth model is incompatible — ChatGPT cannot present a static bearer

`BROWSER_MCP_REQUIRE_AUTH=true` gates `/mcp` on a 64-char static bearer. Verified
against the public endpoint:

```
POST /browser-mcp  initialize   (no Authorization)  -> 401 {"code":-32001,"unauthorized"}
POST /browser-mcp  initialize   (Bearer <secret>)   -> 200 + capabilities
```

Per OpenAI's MCP docs, ChatGPT custom connectors authenticate with **OAuth 2.1**
(public-client or `private_key_jwt` token exchange, CIMD or dynamic client
registration) or **no auth**. A user-supplied static API key/bearer is *not*
something the ChatGPT connector presents. So today ChatGPT gets a 401 on every
call and can never complete `initialize`.

Note the OpenAI **Responses API** `mcp` tool is different — it accepts arbitrary
`headers`, so a static bearer works there. "ChatGPT agents" (the app) and the API
are not the same integration surface.

Options, in ascending order of work:
- **(a)** `BROWSER_MCP_REQUIRE_AUTH=false` → a no-auth ChatGPT connector works
  immediately. But `browser_act` is write-capable and the gateway `/browser-mcp`
  location has no `$dd_mcp_auth_ok` gate, so this puts a public browser driver on
  the internet. The allowlist becomes the *only* control.
- **(b)** Implement OAuth 2.1 in `browser-mcp-rs` (the correct fix for a public
  write-capable endpoint).
- **(c)** Keep the bearer and drive it from the Responses API instead of the app.

### 2. The running binary is STALE — the SSE fix is not live

`d809bde6` made `GET /mcp` + `Accept: text/event-stream` return **405**. In
production it still returns **200 + JSON descriptor**, the pre-fix behavior, both
through the gateway and port-forwarded directly to the pod.

Root cause: the pod does not run a built image. It runs
`cargo run --release` over a **hostPath** mount of
`/home/ec2-user/codes/dd/dd-next-1`, so the *manifest* comes from ArgoCD/git but
the *source* comes from the node's checkout — and that checkout is at:

```
551ba7dc  Merge pull request #36 from ORESoftware/fix/hetzner-browser-test
```

which predates `d809bde6`, `ad8f6422`, and `dfd9089b`. This is why the env vars
look current (ArgoCD synced them) while the behavior is old (the binary is not).

**Decision: do NOT `git pull` that checkout.** Measured on the live cluster,
**93 deployments** mount `/home/ec2-user/codes/dd/dd-next-1` — essentially the
whole in-pod-build fleet (`dd-web-scraper`, `dd-remote-rest-api`, `dd-runtime-config`,
every `dd-thread-*`, …). Pulling it stages new source for all 93, each of which
silently rebuilds whatever is on disk at its *next* restart. That is an
unbounded, delayed-action change to the entire runtime in order to fix one
service's SSE negotiation. Not an acceptable trade.

Two surgical alternatives, preferred order:

1. **Give this one service a real image.** `browser-mcp-rs/Dockerfile` already
   exists. Building and pinning an image removes it from the hostPath fleet
   entirely, makes it independently deployable, and means its binary matches its
   manifest — which is the actual root cause here (manifest from ArgoCD, binary
   from a node checkout, no relationship between them). This is the durable fix.
2. **Update only its subdirectory** on the node
   (`remote/deployments/browser-mcp-rs`) and restart just that deployment. Fast,
   but leaves the manifest/binary split in place to bite again.

### 3. The allowlist permits only filing sites

`BROWSER_MCP_ALLOWED_DOMAINS=sos.state.co.us,irs.gov,dnb.com` — verified live.
This was deliberate (`dfd9089b` "filing sites only", Gmail explicitly dropped), so
credit-program and conference-CFP sites are blocked by design. Widening it is a
policy decision, not a bug. Widen deliberately, host by host.

### Also worth knowing: search/fetch are surface-specific

OpenAI's current docs no longer require every custom MCP app to expose `search`
and `fetch`. This server's `browser_act`/`browser_state` pair is sufficient
for a Developer-Mode custom app. Company knowledge and deep research use only
read/fetch capabilities, so this write-oriented app is not intended for those
surfaces.

## Reproduce the live verification

```sh
CTX=dd-ec2-runtime
BEARER=$(kubectl --context $CTX -n default get secret dd-browser-mcp-rs-secrets \
          -o jsonpath='{.data.BROWSER_MCP_AUTH_SECRET}' | base64 -d)
curl -sk -X POST https://98.90.186.114/browser-mcp \
  -H "Authorization: Bearer $BEARER" -H 'Content-Type: application/json' \
  -H 'Accept: application/json, text/event-stream' \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"browser_act",
       "arguments":{"intent":"smoke","actions":[{"type":"start"},
       {"type":"goto","url":"https://www.irs.gov/"}]}}}'
```
