# Infra + security test sweep — 2026-07-25

A two-wave unit-test sweep across ten messaging/infra/security services, adding
~303 tests (all pure logic; no live NATS/DB/cluster). The tests were written to
**characterize current behavior + pin genuine invariants**, and to surface bugs
rather than paper over them. Four real defects found this way were fixed (each
with a flipped regression test); the rest are **open findings** listed below for
a decision.

Landed in `main` as `cb32b6c7` (wave 1) and `2083aac4` (wave 2).

## Coverage added

| Service | Kind | Tests (added → total) |
|---|---|---|
| `remote/deployments/thread-operator-go` | k8s operator | +25 → 31 |
| `remote/deployments/thread-fleet-exporter-go` | Prometheus exporter | +18 → 20 |
| `remote/deployments/queue-consumer-rs` | NATS queue consumer | +23 → 30 |
| `remote/deployments/browser-job-runner-rs` | NATS browser-job orchestrator | +35 |
| `remote/nats-bridge` | HTTP→NATS bridge (security) | +23 → 29 |
| `remote/deployments/auth-server-rs` | PIN/TOTP/cookie gateway auth | +42 → 44 |
| `remote/deployments/apostille-services-server-rs` | doc intake/orchestration | +38 → 40 |
| `remote/deployments/webrtc-signaling-rs` | WebRTC signaling | +39 |
| `remote/deployments/bastion-rs` | cluster access broker | +28 → 31 |
| `remote/deployments/wal-gateway-rs` | Postgres→NATS CDC relay | +33 → 37 |

## Fixed this sweep (regression-tested)

| # | Severity | Service | Defect | Fix |
|---|---|---|---|---|
| F1 | HIGH | apostille | `blocked_host` only ran IPv6 predicates for v6 inputs, so an **IPv4-mapped IPv6** literal (`::ffff:169.254.169.254`) bypassed the private-host filter → cloud metadata endpoint (SSRF → IAM creds). | Judge mapped addrs by embedded IPv4 via `to_ipv4_mapped()` (mapped range only, so `::1` stays loopback); also block RFC 6598 CGNAT `100.64.0.0/10`. |
| F2 | MED | auth-server-rs | `safe_return_to` only rejected leading `//`; `/\evil.com` passed and browsers fold `\`→`/` → `https://evil.com` **open redirect** off the auth gate. | Also reject `/\…`. |
| F3 | MED | browser-job-runner-rs | `handle_run` timeout used `requested.clamp(1_000, max_timeout_ms)`, which **panics on every authorized `POST /run`** when `BROWSER_JOB_MAX_TIMEOUT_MS < 1000` (`Ord::clamp` needs `min ≤ max`). | Extracted non-panicking `effective_max_ms()` (ceiling wins below the 1s floor). |
| F4 | MED | nats-bridge | Subject allowlist matched with raw `str::starts_with`, safe only because shipped prefixes end in `.`; a dotless prefix (`vxl`) silently widened to sibling subjects (`vxlmalicious.*`). | Token-boundary-anchored `subject_in_prefix()`; also makes a stray empty prefix no longer permit-all. |

## Open findings (characterized, NOT fixed — decide per item)

Each is pinned by a passing characterization test that will trip if the behavior
changes, so a future fix is safe to make.

### Medium

- **webrtc-signaling-rs — no capacity limits (DoS).** No per-room or global room/peer cap on join (`signal_socket`, `admin_runtime_socket`); per-peer outbound queues are `mpsc::unbounded_channel`, so a stalled receiver's queue grows without bound → memory exhaustion. Empty rooms *are* reclaimed on last leave (verified). Fix: cap rooms/peers and bound the send queues.
- **webrtc-signaling-rs — duplicate peer-id overwrite (routing hijack).** `room.peers.insert(peer_id, …)` replaces a live same-id entry with no uniqueness check; a reconnect reusing a live `?peer=` evicts the first from routing, and either side's close then tears down the other and emits a spurious `peer-left`. (Message `from` is server-stamped, so payload identity can't be spoofed — but the routing slot can be hijacked.) Fix: reject/deconflict duplicate ids on join.
- **auth-server-rs — cookie is a static shared bearer, not signed.** Despite the "signed-cookie" framing there is no HMAC/signature and no server-side expiry: `DD_AUTH_COOKIE_VALUE` is one fixed value for every browser/session, valid until the secret is rotated (`Max-Age` is only a browser hint). Cookie *attributes* are otherwise hardened (HttpOnly/Secure/SameSite=Lax/Path=/). Fix: issue an HMAC-signed, expiring token; verify signature + expiry server-side.

### Low

- **auth-server-rs — no TOTP replay protection.** A captured 6-digit code validates repeatedly across its ~90s window (no last-counter/one-time tracking).
- **apostille — residual SSRF encodings.** Decimal (`2130706433`) and octal (`0177.0.0.1`) IPv4 encodings parse as hostnames, not IPs, so they slip the filter (impact depends on the downstream resolver). Also DNS-rebinding: validation is pre-resolution, so a public hostname resolving to a private IP isn't caught.
- **apostille — non-constant-time secret compare.** `require_auth`/`require_webhook_auth` use `==` (timing side-channel on the operator/webhook secret).
- **bastion-rs — empty `matchLabels` matches every pod.** `selector_matches_pod` returns true for `matchLabels: {}` (`all()` over empty). Latent: gated by the allowlist + same-namespace lookup, and real Deployments always carry a selector.
- **bastion-rs — `parse_memory_bytes` overflow panic (debug).** `prefix.parse::<i64>() * multiplier` can overflow on absurd values; contained inside a per-namespace spawned task whose `JoinError` is swallowed (degrades that namespace's metrics, doesn't crash the server).
- **wal-gateway-rs — `env_u64` zero→default footgun.** `WAL_GATEWAY_MAX_BATCH=0` (and `POLL_MS`/`PUBLISH_TIMEOUT_S=0`) silently revert to defaults rather than 0/disabled; `max_batch=0` becomes 2000 and skips the documented floor of 1.
- **wal-gateway-rs — no NATS subject sanitization.** `cdc_row_change_subject` is a bare `format!("{prefix}.{schema}.{table}.{op}")` (in the shared `dd-nats-subject-defs` crate); a `.`-containing quoted PG identifier injects extra subject tokens.
- **thread-fleet-exporter-go — counter-suffixed gauges.** Four gauges carry the counter-reserved `_total` suffix (`dd_thread_fleet_total`, `…_replicas_desired_total`, `…_replicas_ready_total`, `…_pvcs_total`) — promlint would flag them. Baked into the Grafana/alert contract, so renaming is breaking; pinned as a contract instead.
- **thread-operator-go — `ShortID` empty-name edge.** An all-dash `threadId ≥12` chars yields an empty `ShortID` (dash-stripping) → `ChildName` degrades to bare `dd-thread-` and such threads collide. Unreachable with real UUIDs; pinned.

### Informational / by-design (documented invariants)

- **wal-gateway-rs — no per-record checksum.** It's a Postgres logical-replication → NATS JetStream CDC relay: a byte-flip that keeps a wal2json line valid JSON is published verbatim. Ordering/at-least-once/integrity are delegated to the PG slot (peek→publish→ack→get) and JetStream, not re-checked in-process. By design.
- **webrtc-signaling-rs / bastion-rs — gateway-trust auth.** Admin WS (`x-dd-admin: 1`) and the XFF-rightmost rate-limit key both rely on the `dd-remote-gateway` injecting/stripping headers; correct behind the gateway, no defense-in-depth if reached directly.
- **queue-consumer-rs / browser-job-runner-rs — `env_bool` semantics.** An explicitly-set unrecognized/empty value yields `false` (only `1/true/yes/on` enable), overriding a `true` fallback — a typo silently disables rather than falling back.
- **thread-operator-go — `IDLE_TIMEOUT_MS` hard-coded `"0"`.** The in-container idle timer is disabled; auto-sleep is enforced solely by the operator's scale-to-0.

## Notes

- All ten services are **vendored** into `k8s-cluster` (regular tracked files, not
  submodules), so the changes are ordinary parent-repo commits.
- Sync caveat encountered: the wave-1 merge of `origin/main` (which had advanced
  17 disjoint commits) left ~29 submodule *pointers* stale in the working tree; a
  blind `git add -A` would have reverted them. Commits staged only the vendored
  files explicitly. 26/29 stale worktrees were then resynced; 3 remain
  (`fiducia-monorepo`, `gleam-lambda-runner` have correct pointers + untracked
  inner WIP; `sonus-auris-monorepo` has a diverged worktree HEAD — left for the
  owner to reconcile).
