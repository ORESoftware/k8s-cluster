# Security audit & hardening record

_Date: 2026-07-19. Scope: the full `tor-server.rs` codebase (Rust sources, the
web dashboard, container/Kubernetes manifests, and the dependency tree)._

## Method

The audit ran four independent adversarial reviewers in parallel, each owning one
threat dimension, plus tooling that reading alone can't provide:

1. **Cryptography & protocol** — handshake, AEAD, framing, telescoping, key handling.
2. **DoS / concurrency** — timeouts, semaphores/permits, task lifetime, allocation bounds.
3. **Injection / SSRF / web** — exit policy, dashboard, request parsing, auth.
4. **Deployment / secrets / supply-chain** — container, k8s, key files, dependencies.
5. **Tooling** — `cargo audit` (RustSec CVE scan), plus a panic/`unsafe` surface sweep.

Every finding was independently re-verified against the code before any fix was
applied; fixes were then re-verified by unit tests and end-to-end runs.

## Headline results

- **0 `unsafe`, 0 `panic!`/`unreachable!`**; every non-test `unwrap`/`expect` is
  provably infallible (length-checked slices, fixed-size KDF/HMAC).
- **Default (overlay) build: 0 dependency advisories.** `rsa`/`bincode`/`paste`/`spin`
  are all confined to the optional `arti` feature; the shipped default image
  contains no `rsa` code. RUSTSEC-2023-0071 remains correctly ignored (client-only
  Arti, no RSA private-key operations) — see `.cargo/audit.toml`.
- The onion cryptography core (ntor-like handshake authentication, forward
  secrecy, contributory-DH checks, per-hop AEAD, nonce management, replay/tagging
  resistance, PSK folding) was reviewed in depth and found **sound**.

## Findings and disposition

### Fixed — correctness / availability

| # | Sev | Finding | Fix |
|---|-----|---------|-----|
| C-1 | **High** | **No end-to-end integrity.** `peel_backward` accepted a terminal `Data`/`End` cell at *any* onion layer, so a malicious entry/middle relay (which holds the client's backward key for its own hop) could inject application bytes attributed to the exit. | `circuit.rs`: accept `Data`/`End` only at the exit layer; producing a valid one there needs the exit's key. Regression test added. |
| D-1 | **High** | **Relay pump permit leak.** The backward pump held the circuit-slot permit but read its peer with no timeout, so a silent next-hop/destination parked it forever → `TOR_MAX_CIRCUITS` permanently exhausted. (Regression from an earlier permit-accounting change.) | `relay.rs`: idle timeout on both pumps; `TOR_CIRCUIT_IDLE_TIMEOUT_SECS` now defaults to a finite 600 s and covers the pumps. |
| D-2 | **High** | **Connector splice task leak.** The detached overlay splice task read the circuit with no timeout and outlived its (already-freed) front-end permit → uncounted circuits/FDs accumulate. | `circuit.rs`: idle timeout on the backward splice read (`TOR_CLIENT_IDLE_TIMEOUT_SECS`, default 600 s). |
| D-4 | Med | **Exit DNS resolution had no timeout** — a black-holed nameserver pinned a circuit slot + a blocking-pool thread. | `policy.rs`: `lookup_host` wrapped in a 10 s timeout. |

### Fixed — access control / SSRF

| # | Sev | Finding | Fix |
|---|-----|---------|-----|
| W-1 | Med | **Dashboard auth gap.** `TOR_UI_TOKEN` guarded only `/api/fetch`; `/`, `/api/status`, and `/ws/stats` leaked the relay directory + counters unauthenticated on a non-loopback bind. | `web.rs`: a middleware requires the token for all relay-list/proxy endpoints when set (exempting `/healthz`, `/vendor`, `/docs`, `/proxy.pac`); the WS client forwards the token. |
| S-3 | Low | **SSRF blocklist gaps** — `192.0.0.0/24` (incl. NAT64/DS-Lite) and `192.88.99.0/24` (6to4 relay) were permitted. | `policy.rs`: both ranges added. |
| S-4 | Low | **Exotic IPv6 v4-embeddings** (Teredo `2001:0000::/32`, IPv4-translated `::ffff:0:0/96`) not decoded. | `policy.rs`: decode + block the embedded private v4 (defense-in-depth). |
| W-5 | Low | **Stored XSS surface** — raw HTML in a markdown doc rendered verbatim. | `web.rs`: raw-HTML events escaped to text. |

### Fixed — secrets / deployment hardening

| # | Sev | Finding | Fix |
|---|-----|---------|-----|
| K-1 | Med | **No NetworkPolicy egress** — pods could reach the cloud-metadata endpoint / arbitrary hosts at the network layer. | `k8s/networkpolicy.yaml`: egress lockdown (client → relays + DNS; relays → internet minus RFC1918/metadata + relays + DNS). |
| K-3 | Med | **Relays could co-locate** on one node, collapsing unlinkability. | `k8s/relays.yaml`: required pod anti-affinity on `kubernetes.io/hostname`. |
| K-6 | Low | **`.gitignore` missed natural secret copy-names** (`*-keys.secret.yaml`, `*-secret.secret.yaml`). | Broadened to `k8s/*.secret.yaml` (example templates negated). |
| Cf-4 | Low | **Key-file read followed symlinks** (write path was already `O_EXCL`). | `config.rs`: reject a symlink key file on read. |
| Cr-2 | Low | **Transient secret material not zeroized.** | `config.rs`/`crypto.rs`: `zeroize` the plaintext key buffers, KDF input/output, and auth key. |
| L-1 | Low | **Env-sourced secrets** exposed via `/proc/<pid>/environ`. | `main.rs`: warn when a secret is read from the environment instead of a `_FILE`. |

### Accepted / documented (not changed)

- **Image tag-pinning** (not digest) — recommend digest-pinning `debian:bookworm-slim`
  and the app image before production; digests are environment-specific and not
  fabricated here.
- **No CPU limit** on pods — deliberate (CPU limits throttle a latency-sensitive
  proxy); memory is limited and requests are set. An `ephemeral-storage` limit is a
  reasonable optional add.
- **Per-front-end connection cap is per-listener**, not global — total ≈ cap ×
  (SOCKS + HTTP + #forward routes). Intended; documented here.
- **`?token=` in a URL** can leak via history/`Referer`/logs — prefer the
  `Authorization: Bearer` path for shared/logged environments.
- **Debug logging** records peer/circuit metadata — privacy-sensitive; keep the
  default `info`.
- **No AEAD associated data** — not independently exploitable (distinct per-hop,
  per-direction keys + authenticated cell-type tag); a defense-in-depth-only gap.
- **Unauthenticated directory** — relay authentication is only as strong as the
  integrity of `directory.toml`; a signed/consensus directory is future work (see
  the distributed-directory roadmap item). Protect the file via RBAC + a read-only
  ConfigMap mount.
- **No cell padding** — exact payload sizes are visible to relays/observers
  (traffic analysis); a known limitation vs. production Tor.

## Verification

- **36 unit tests pass** (new: end-to-end integrity rejection; expanded SSRF
  ranges incl. Teredo bit-inversion; dashboard same-origin/token). `cargo clippy`
  clean apart from the repo's explicit-`return` house style.
- **End-to-end:** a live overlay still round-trips SOCKS traffic after the
  crypto/idle-timeout changes (regression check for C-1/D-1/D-2), and the token
  gate returns 401 on `/`, `/api/status`, `/api/fetch`, and `/ws/stats` without the
  token while `/healthz`, `/vendor`, and `/docs` stay open and the authorized flow
  works.

## Residual threat model (unchanged assumptions)

This is a **private-overlay** design, not production Tor. It assumes the local
`directory.toml` is integrity-protected; it provides no traffic-analysis
resistance (no padding/timing defenses); and an exit necessarily sees plaintext
for non-TLS destinations. Use TLS end-to-end, run relays only on infrastructure
you are authorized to use, and prefer real Tor/Arti (`TOR_BACKEND=arti`) when
facing a capable global adversary.
