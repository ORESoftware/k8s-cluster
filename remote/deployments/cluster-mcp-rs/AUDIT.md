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

## Pass 3 (2026-06-09)

Audited the surfaces the first two passes didn't: the merged
`dd-runtime-config-client` `/internal/*` router, the vendored NATS wire parser,
and the gateway bearer check. Two fixes here, both small:

- **RC1 (Med, shared lib `runtime-config-client-rs`).** `register_with_control_plane`
  built its reqwest client without a redirect policy. That POST carries the
  `X-Server-Auth` control-plane secret, and reqwest only strips the standard
  `Authorization`/`Cookie` headers on a cross-origin redirect — a **custom**
  header like `X-Server-Auth` is forwarded to the redirect target. A redirecting
  `RUNTIME_CONFIG_REGISTER_URL` would leak the secret. Fixed with
  `.redirect(reqwest::redirect::Policy::none())`. Blast radius: every Rust
  service using this lib (all strictly safer).
- **RC3 (Low, this server).** `main()` applied `DefaultBodyLimit::max(1 MiB)`
  *before* `.merge(runtime_config_client::router())`, so the merged `/internal/*`
  routes fell back to axum's 2 MiB default instead of the intended 1 MiB. Moved
  the layer after the merge. (The routes are `X-Server-Auth`-gated, so this is
  hygiene, not a hole.)

Verified sound, left as-is: the gateway validates the IDE bearer with an nginx
`map` exact-match on `"Bearer <token>"` (default-deny) and strips it before
proxying to the pod — no bypass. The runtime-config auth uses a constant-time
compare. `GET /internal/runtime-config` is unauthenticated by shared-lib design
(non-secret runtime toggles) — fine as long as nothing secret is pushed into the
snapshot. The vendored `dd_nats.erl` parser `binary_to_integer`s wire-supplied
lengths and grows its buffer unbounded, but the broker is a trusted, egress-
restricted in-cluster service and a crash only triggers a supervised reconnect;
left unchanged to stay byte-identical with the `gleamlang-presence-server` source.

`cargo test` 11 passed, clippy clean.

## Pass 4 (2026-06-09)

First supply-chain + build + pod-posture pass.

- **DEP1 (the real find — 4 CVEs).** `cargo audit` flagged `rustls-webpki 0.102.8`
  (transitive via the `=0.23.20` rustls pin): RUSTSEC-2026-0098/0099 (cert
  **name-constraint** validation accepted URI/wildcard names it shouldn't),
  RUSTSEC-2026-0049 (CRL authority matching), RUSTSEC-2026-0104 (reachable panic
  in CRL parsing). The name-constraint bugs matter here — this server validates
  the k8s API server cert while carrying the SA token. **Fixed** by relaxing the
  rustls pin `=0.23.20 → 0.23` and `cargo update -p rustls`, which moved
  rustls `0.23.20 → 0.23.40` and webpki `0.102.8 → 0.103.13` (two packages only).
  `cargo build --release --locked` still passes (so the Dockerfile / `cargo run
  --release --locked` deploy path stays reproducible) and `cargo audit` is clean.
  Remaining: a non-vulnerability "unmaintained" warning for `rustls-pemfile`
  (transitive via reqwest); resolve later by bumping reqwest off the `=0.12.9`
  pin. **Fleet note:** other Rust deployments pin the same old rustls and almost
  certainly carry the same webpki CVEs — worth a fleet-wide `cargo update -p
  rustls`.
- **Build/Dockerfile:** good already — multi-stage (`rust:1.90-bookworm` →
  `debian:bookworm-slim`), `--locked`, `USER 10001:10001`. Only nit: base images
  are tag-pinned, not digest-pinned (left as-is).
- **OTLP exporter (Low, noted):** `finish_span` spawns a detached tokio task per
  request span; under a flood that is request-proportional unbounded background
  concurrency. Bounded by the 800 ms export timeout and a trusted in-cluster
  collector, so left as-is — revisit with a bounded worker/semaphore if it ever
  matters.

The Gleam sibling was brought to non-root in the same pass (it previously ran as
root while this server already ran as uid 1000) — see `../gleam-mcp-server/AUDIT.md`.
