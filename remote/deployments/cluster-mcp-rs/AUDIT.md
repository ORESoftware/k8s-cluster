# dd-cluster-mcp-rs — security & correctness audit

Audit + hardening pass on the Rust `dd_cluster` MCP server. Dated 2026-06-09.
Companion to `../gleam-mcp-server/AUDIT.md` (the two servers expose the same
read-only `dd_cluster` tool surface and share a threat model).

## Findings

| # | Severity | Area | Status |
|---|----------|------|--------|
| R1 | **Med** | Unbounded metric-map cardinality keyed by caller-supplied `method`/`tool` | **Fixed** |
| R2 | Med | reqwest clients followed redirects (SSRF amplifier) | **Fixed** |
| R3 | Low | k8s client trusted public webpki roots in addition to the cluster CA | **Fixed** |
| R4 | Low | Missing/invalid SA CA failed silently (opaque TLS failures) | **Fixed** |
| R7 | Hardening | No in-pod auth option (parity gap vs Gleam server) | **Added (opt-in)** |
| R5 | Low | `response.bytes()` reads the full upstream body before clipping | Documented |
| R6 | Rec | NetworkPolicy egress `0.0.0.0/0:443` for build-time crates.io | Documented |

### R1 — Unbounded metric cardinality (Med, fixed)

`Metrics::record_rpc(method)` and `record_tool(tool)` inserted the
request-supplied `method` / `params.name` strings verbatim into
`Mutex<BTreeMap<String,u64>>` maps. Any caller — including the **unauthenticated
in-VPC ingress path** (`172.31.0.0/16 :8091`) — could send millions of distinct
method/tool names and grow those maps without bound: memory exhaustion plus an
ever-growing `/metrics` body (one line per distinct label). Reachable before any
tool dispatch, so even a "method not found" / "unknown tool" path recorded the
attacker string.

**Fix:** `bounded_method_label` / `bounded_tool_label` collapse anything outside
the fixed known sets (`KNOWN_METHODS`, `TOOL_NAMES`) to `"other"` before
recording. Cardinality is now bounded to the static label set. Test:
`metric_labels_bucket_unknown_to_other`. (The Gleam server was already immune —
its metrics are fixed counters.)

### R2 — reqwest followed redirects (Med, fixed)

Neither `build_k8s_client` nor `build_http_client` set a redirect policy, so
reqwest's default (follow up to 10) applied. A 3xx from a backend could retarget
a read to another host. reqwest strips `Authorization` on cross-origin redirects,
so the SA token would not leak, but the server would still issue the redirected
request (SSRF). **Fix:** `.redirect(reqwest::redirect::Policy::none())` on both
clients.

### R3 — k8s client trusted public roots (Low, fixed)

`build_k8s_client` added the cluster CA via `add_root_certificate` but left the
built-in webpki roots enabled, so a publicly-trusted cert would also be accepted
for the token-bearing k8s call. No public CA issues for `kubernetes.default.svc`,
so real risk is low, but **fix:** `.tls_built_in_root_certs(false)` pins trust to
the cluster CA only. (The observability `http` client keeps built-in roots so an
HTTPS OTLP endpoint still works.)

### R4 — Silent CA load failure (Low, fixed)

If the SA CA file was unreadable or unparseable the client was built without it
and k8s calls failed TLS with no explanation. **Fix:** log a `WARN`
(`cluster_mcp.k8s.ca_read_failed` / `…ca_parse_failed`). Still fails closed.

### R7 — Optional in-pod bearer gate (added)

The Gleam server already had `MCP_REQUIRE_AUTH`; the Rust server had none, so the
unauthenticated in-VPC path could only be closed at the NetworkPolicy (which must
stay open for host-network pool workers). **Added** a matching opt-in gate: with
`MCP_REQUIRE_AUTH=true` + `MCP_AUTH_SECRET`, every `POST /mcp` must present
`Authorization: Bearer <secret>` or `X-Server-Auth: <secret>`, else `401`.
**Default off** (manifest sets `MCP_REQUIRE_AUTH=false`); fails closed if required
but unset. Test: `header_secret_gate_accepts_only_matching_credentials`.

### R5 — Unbounded response body read (Low, documented)

`kubernetes_get` and `http_result` call `response.bytes().await`, reading the
entire upstream body into memory before clipping the sample. A compromised
in-cluster backend could return a very large body. Mitigated by per-request
timeouts and the k8s API being TLS-pinned + trusted. Left as-is; a bounded
streaming read is the follow-up if the observability plane is ever less trusted.

### R6 — Broad build-time egress (recommendation)

The NetworkPolicy allows egress `0.0.0.0/0:443` so first boot can fetch crates.io
(this service runs `cargo run` from the EC2 host checkout, like the other Rust
deployments). For a token-bearing cluster-recon pod this is a real exfiltration
path if the pod is ever compromised (e.g. a malicious crate). The Gleam server
avoids it via committed offline `build/packages`. **Recommendation:** `cargo
vendor` + offline build, or a prebuilt image, then drop the `0.0.0.0/0` rule (the
manifest comment already flags this).

## Done well (left as-is)

Real serde JSON-RPC parse (not substring routing); `sanitize_rpc_id` echoes only
scalar ids, else `null`; `jsonrpc:"2.0"` + non-empty method enforced; 1 MiB body
cap (axum layer + manual check); `allowed_mcp_base_url` allowlist (loopback /
`*.svc` / `kubernetes.default.svc`) gating `MCP_ALLOW_EXTERNAL_URLS`, applied to
the k8s API URL so the token destination is constrained; recursive JSON redaction
+ line-based text fallback over a comprehensive secret-key set; `clip_string` is
char-boundary safe and `from_utf8_lossy` avoids invalid-UTF-8 output;
`sanitize_url_for_output` strips creds/query/fragment; SA token sent only to the
k8s API and never logged (errors go through `sanitize_text`); hardened
securityContext (non-root, RO rootfs, dropped caps, seccomp); list-only RBAC;
ingress ipBlock already tightened to `172.31.0.0/16`; graceful shutdown.

## Verify

```sh
cd remote/deployments/cluster-mcp-rs
cargo test            # 11 passed (incl. R1 + R7 regression tests)
cargo clippy          # clean
KUBECTL_NO_CONFIRM=1 kubectl kustomize k8s/ec2   # 7 resources
```
