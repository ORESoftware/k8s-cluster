# dd-gleam-mcp-server — security & correctness audit

Audit + hardening pass on the MCP servers under
`k8s-cluster/remote/deployments`. Dated 2026-06-09.

## Scope note: there is exactly one MCP server

Despite the brief ("…mcp servers in rust and gleamlang"), only **one**
deployment actually speaks the MCP JSON-RPC protocol: **`gleam-mcp-server`**
(Gleam/BEAM). Every Rust hit for "mcp" is a *client or config* pointing at it —
`rest-api-rs` (`AGENT_MCP_URL`), `bastion-rs`/`web-home-rs` (gateway routing),
`dev-server`, `runtime-config-rs` (runtime-config callback). `billing-server-rs`'s
"jsonrpc" is a Solana RPC client, not MCP. So this audit targets the Gleam
server. If a Rust MCP server is intended, the regenerated
`dd_nats_subject_defs` Rust constants (`MCP_TOOL_EVENTS_SUBJECT`,
`MCP_CONTROL_SUBJECT`) are already in place to wire it the same way.

## Findings

| # | Severity | Area | Status |
|---|----------|------|--------|
| F1 | **High** | JSON-RPC routed by substring match on the raw body | **Fixed** |
| F2 | **High** | `id` extracted by regex; parse errors echoed `id:"1"` | **Fixed** |
| F3 | Med | No batch/`jsonrpc` validation; everything got a 200 | **Fixed** |
| F4 | Med | Body truncation could split a UTF-8 char → invalid JSON | **Fixed** |
| F5 | Low | Unknown tool returned a protocol error (`-32602`) | Kept (acceptable) |
| F6 | High* | `POST /mcp` has no auth; NetworkPolicy allows all RFC1918 | **Opt-in gate added** |
| F7 | Med | Upstream URLs env-overridable, fetched server-side (SSRF surface) | Documented |
| F10 | Feature | Server had no NATS publish/subscribe | **Implemented** |

\* High for defense-in-depth; the tools are read-only, but cluster-inventory
metadata is sensitive reconnaissance.

### F1 — JSON-RPC routing by substring match (High, fixed)

`method_from_body` / `tool_from_body` in `http_server.gleam` selected the method
and tool by `string.contains` over the **raw request body**, by priority order.
Any request whose *arguments* contained one of those literals would misroute —
e.g. a `tools/list` whose params carried the string `"tools/call"` was executed
as `tools/call`; an argument value `"kubernetes_inventory"` overrode the real
`params.name`. Latent today (tools take no arguments) but exploitable the moment
any tool accepts input, and wrong for nested data regardless.

**Fix:** `gleam_mcp_json.erl` now does a single real JSON parse (OTP `json`,
present since OTP 27 — the deploy image is erlang-alpine OTP 27+) and returns the
**top-level** `method`, `id`, and `params.name`. Routing reads those fields.
Falls back to the old best-effort extraction only if the `json` module is
absent. Locked in by `test/gleam_mcp_server/json_rpc_test.gleam`
(`quoted_method_literal_in_args_does_not_misroute_test`,
`tool_literal_in_args_does_not_misroute_tool_test`).

### F2 — `id` handling violated JSON-RPC (High, fixed)

The old `request_id` regex matched the first `"id":…` anywhere in the body — so
an `id` nested inside `params` could be echoed as the response id — and defaulted
to `"1"` on failure. JSON-RPC requires the response id to be **`null`** when the
request id can't be determined.

**Fix:** the id is now read from the decoded top-level object and re-encoded
verbatim (number, string, or null). Parse/invalid-request errors respond with
`id: null`. Requests without an id are treated as notifications (HTTP 202, no
body) per spec, instead of being answered with a fabricated id.

### F3 — No envelope validation / batch handling (Med, fixed)

Every readable body produced an HTTP 200. A JSON-RPC batch (top-level array) was
substring-matched into a single bogus response.

**Fix:** the parser classifies each body as `request | notification |
invalid_request | parse_error`. MCP 2025-11-25 removed JSON-RPC batching, so a
top-level array → `invalid_request` (`-32600`, id null). Invalid JSON →
`parse_error` (`-32700`, id null).

### F4 — UTF-8-unsafe body truncation (Med, fixed)

`clip/2` in `gleam_mcp_k8s.erl` and `gleam_mcp_observability.erl` cut upstream
response bodies at a raw byte offset. Cutting mid-multibyte-char yields invalid
UTF-8, which then renders the enclosing JSON string — and so the whole
`structuredContent` envelope — unparseable for the MCP client.

**Fix:** `clip` backs the cut off to the nearest valid UTF-8 boundary
(`utf8_prefix`/`valid_prefix`, ≤3-byte backoff since a code point is ≤4 bytes).

### F5 — Unknown tool returns protocol error (Low, kept)

MCP suggests tool *execution* failures be returned as a result with
`isError:true`. An unknown tool name is genuinely invalid params, so `-32602` is
defensible; left as-is to avoid churn. Revisit if tools gain arguments.

### F6 — No authentication on `/mcp` (High for defense-in-depth, opt-in gate added)

`POST /mcp` is unauthenticated and the NetworkPolicy deliberately allows ingress
on `:8090` from **all** of `10/8`, `172.16/12`, `192.168/16` (to admit
host-network pool workers). Anyone able to reach the pod on the node network can
call every tool, including `kubernetes_inventory` (all namespaces/nodes/
serviceaccounts/events metadata) and `kubernetes_deployments`. The server's own
`human_access_policy` tool advertises "humanAuthRequiredForPublicGateway", which
`/mcp` did not enforce.

**Fix (opt-in):** an optional bearer gate. With `MCP_REQUIRE_AUTH=true` and
`MCP_AUTH_SECRET` set, every `POST /mcp` must present `Authorization: Bearer
<secret>` or `X-Server-Auth: <secret>`; otherwise `401` with a JSON-RPC error.
**Default off** (`MCP_REQUIRE_AUTH=false` in the manifest) so existing
unauthenticated callers are unaffected — flip it to lock down the recon tools
without rewriting the NetworkPolicy. Fails closed if require-auth is on but no
secret is configured.

### F7 — Server-side fetch of env-configured URLs (Med, documented)

All upstream targets (`MCP_KUBERNETES_API_URL`, `MCP_PROMETHEUS_URL`, …) are
env-overridable and fetched server-side. The Kubernetes SA bearer token is sent
to whatever `MCP_KUBERNETES_API_URL` resolves to. Env is operator-controlled
(set by the Deployment), so real risk is low, but: the k8s call already pins
`verify_peer` against the SA CA (good), and the SA token is **not** sent to the
observability endpoints (good — no token leak there). **Recommendation:** if
these URLs ever become dynamically influenced, pin the k8s API to
`kubernetes.default` / https and gate the token on host match.

## NATS integration (F10 — implemented)

The server only scraped NATS *metrics* over HTTP; it had no presence on the bus.
It now joins NATS using the **source-of-truth subject names** from
`remote/libs/nats/subject-defs` (no magic strings) via a new dedicated schema,
`schema/agent-mcp.schema.json`, regenerated into all eight languages:

- **Publish** → `McpToolEvents` (`dd.remote.mcp.tool.events`): a `dd.mcp_event.v1`
  lifecycle event shortly after boot, and one audit event per `tools/call`
  (tool, ok flag, request id).
- **Subscribe** → `McpControl` (`dd.remote.mcp.control`): read-only ops commands;
  `{"command":"ping"}` echoes a `pong` onto `McpToolEvents`. Commands never touch
  the cluster — the MCP service account is list/read only.

Implementation mirrors the proven `gleamlang-presence-server` pattern: the
vendored `dd_nats.erl` client (TCP NATS 1.x + HPUB headers + jittered reconnect)
plus a `nats.gleam` actor, supervised, **gated on `NATS_URL`** so the HTTP
surface is unchanged when messaging isn't configured. Every PUB carries a
`Source-Node` header so the server drops its own echoes.

**Required wiring (done):** the NetworkPolicy egress to `dd-nats` previously
allowed only `:8222`/`:7777` (HTTP monitor/metrics) — a NATS **client** needs
`:4222`. Without it the `dd_nats` gen_server would log connect-failure and
reconnect forever while HTTP kept working (the same silent half-failure as the
runtime-config rule). Added `:4222` egress + `NATS_URL` env + the
subject-defs path-dep copy in the boot script, preflight check, and Dockerfile.

## What changed

```
remote/libs/nats/subject-defs/
  schema/agent-mcp.schema.json          NEW   McpToolEvents + McpControl
  schema/index.json                      EDIT  register the schema
  generated/<8 langs>/…                   GEN   regenerated (drift check clean)

remote/deployments/gleam-mcp-server/
  src/gleam_mcp_json.erl                  REWRITE  real JSON-RPC parse + fallback
  src/gleam_mcp_server/http_server.gleam  REWRITE  parsed routing, id semantics,
                                                   batch reject, auth gate, audit
  src/gleam_mcp_server.gleam              EDIT  supervise NATS when NATS_URL set
  src/gleam_mcp_server/nats.gleam         NEW   NATS actor (pub/sub + lifecycle)
  src/dd_nats.erl                         NEW   vendored NATS client (gen_server)
  src/gleam_mcp_nats.erl                  NEW   self-node + clock FFI
  src/gleam_mcp_k8s.erl                   EDIT  UTF-8-safe clip
  src/gleam_mcp_observability.erl         EDIT  UTF-8-safe clip
  test/gleam_mcp_server/json_rpc_test.gleam NEW 10 parser regression tests
  gleam.toml / manifest.toml             EDIT  + dd_nats_subject_defs path dep
  Dockerfile                              EDIT  copy runtime-config + nats deps
  k8s/ec2/…deployment.yaml                EDIT  NATS_URL, MCP_REQUIRE_AUTH, copies
  k8s/ec2/…networkpolicy.yaml             EDIT  egress NATS :4222
```

## Verify

```bash
cd remote/deployments/gleam-mcp-server
gleam build && gleam test          # 14 passed (10 new JSON-RPC parser tests)

cd ../../libs/nats/subject-defs
node src/generate.mjs --check      # generated outputs up to date
node --test "src/**/*.test.mjs"    # generator unit tests

cd ../../../deployments/gleam-mcp-server/k8s/ec2
KUBECTL_NO_CONFIRM=1 kubectl kustomize .   # 7 resources render
```

The RBAC ClusterRole was reviewed: it grants `list` only, on metadata-level
resources, with no `secrets`/`pods/log`/`pods/exec`/mutation verbs — matching
the inventory the tools actually read. No change needed.

## Pass 2 (2026-06-09)

Re-audit alongside the new Rust sibling `../cluster-mcp-rs` (see its `AUDIT.md`).
The follow-up redaction + URL-allowlist work landed in the interim
(`gleam_mcp_redact.erl`, `safe_base_url` gating `MCP_KUBERNETES_API_URL` so the
SA token destination is constrained, clamped `bounded_int` knobs, per-response
`sample` redaction). Those were reviewed and are sound. The `cacertfile` ssl
option already pins the k8s trust store to the cluster CA only (no public-root
equivalent of the Rust R3 finding), and metrics are fixed counters (no
cardinality DoS). One new fix:

- **httpc followed redirects (fixed).** Both `gleam_mcp_k8s.erl` and
  `gleam_mcp_observability.erl` called `httpc:request` without `{autoredirect,
  false}`, so the default (follow) applied. Erlang `httpc` re-sends request
  headers on a 3xx and does **not** strip them cross-host — so unlike reqwest,
  the k8s **SA bearer token could follow a redirect** off-host. Added
  `{autoredirect, false}` to both call sites (defense-in-depth; the k8s API is
  TLS-pinned and does not redirect in practice, but a token-bearing call must
  not be retargetable). Mirrors the Rust `.redirect(Policy::none())` fix.

Build + 14 tests still green.

## Pass 3 (2026-06-09)

Audited the merged `dd-runtime-config-client` `/internal/*` surface. One fix:

- **RC4 (defect, shared lib `runtime-config-client-gleam`).** `escape_json` in
  `dd_runtime_config_client.gleam` was a no-op (returned its input) despite its
  name, while being used to embed an error `reason` into a JSON error envelope.
  Today the reasons are fixed FFI strings (no quotes), so it was latent, but a
  function named `escape_json` must escape — implemented the standard
  backslash/quote/control-char escaping. Blast radius: every Gleam service using
  this lib (strictly safer).

The Gleam-side equivalents of the Rust pass-3 findings do **not** apply: the
Gleam register path is a raw `gen_tcp` HTTP/1.1 request that ignores 3xx (no
redirect-secret-leak), and `handle_apply` already bounds the body to 1 MiB via
`mist.read_body(req, 1_048_576)`. Auth uses a constant-time compare.

Build + 14 tests still green.
