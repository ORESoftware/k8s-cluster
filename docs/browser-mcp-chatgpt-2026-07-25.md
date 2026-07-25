# browser-mcp-rs — ChatGPT MCP server verification, 2026-07-25

`remote/deployments/browser-mcp-rs` is the public MCP server that lets **ChatGPT**
(or Claude / any MCP client) drive a real browser through two model-callable
tools — `browser_observe` (read-only) and `browser_act` (write-capable) — while
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
  `202`. `tools/list` → exactly `browser_act` + `browser_observe`.
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
| `POST /mcp` `tools/list` | `browser_act`, `browser_observe` |

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

## Deployment status (action needed)

`dd-browser-mcp-rs` is **not running** on the cluster (0 replicas). Its ArgoCD
Application (`remote/argocd/apps/dd-browser-mcp-rs.application.yaml`, tracking
`dev`, path `.../k8s/ec2`, `syncPolicy.automated`) exists in the repo but is not
yet registered/synced in ArgoCD. To make the ChatGPT endpoint live:

1. Register the ArgoCD Application (add it to the app-of-apps / `kubectl apply`).
2. Ensure prerequisites the manifests assume: the `dd-remote-gateway`
   `= /browser-mcp` public location (streaming), the `dd-web-scraper` NetworkPolicy
   allowing the `dd-browser-mcp-rs` pod, and — if `BROWSER_MCP_REQUIRE_AUTH=true`
   — the `BROWSER_MCP_AUTH_SECRET` for the public bearer.
3. Sync; confirm `readyz` is green (it checks the worker) and point ChatGPT at
   `https://<gateway>/browser-mcp`.

The server code + transport are verified working; deployment/gateway/secret
wiring is the remaining operator step.
