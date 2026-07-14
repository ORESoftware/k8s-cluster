# Open architectural security findings

Cross-repo security items surfaced by the **2026-07-13 full-platform audit** (and a
**2026-07-14 re-audit**) that need a design decision spanning multiple services,
clients, and/or the deployment contract — not a low-risk local patch. Concrete,
low-risk defects found in the same audits were fixed and committed in their
respective repos; this document tracks what remains open, plus recent resolutions.

> Status legend: **Open** · **Design** (needs a decision before implementation) ·
> **Mitigated** (a compensating control limits impact today) · **Resolved** (with
> commit + date).

_Last updated: 2026-07-14._

---

## 1. Platform-wide shared-secret identity (biggest theme)

**Severity: High · Status: Open / Design · Mitigated (network isolation)**

A single `FIDUCIA_INTERNAL_SECRET` authorizes **all** internal `/v1` traffic across
the platform. It is verified fail-closed with a constant-time compare (good), but it
conveys **no per-actor identity and no operator-vs-service distinction** — every
holder of the secret is fully trusted for every internal action.

| Service | Exposure |
|---|---|
| `fiducia-brain.rs` | Any secret-holding caller (node sidecars, LB, …) can `POST /v1/scale`, `DELETE /v1/nodes/{id}`, `POST /v1/policies` — i.e. drain or rescale the cluster. No per-actor identity or audit trail on operator-grade actions. |
| `fiducia-ai-agent-bridge.rs` + `fiducia-ai-agent-control-plane` | `from` (messages) and `agent_key` (file leases) are caller-asserted under the shared bearer. Any authenticated caller can read a channel it never joined and, in standalone mode, **steal/release another agent's file lease** — the holders lookup returns each lease's `fencing_token`, which is all that a release requires (`bridge state.rs` release path, `http.rs` lease routes, `types.rs` holder shape). |
| `fiducia-lambda-service.rs` | Any secret holder can invoke **any tenant's** function — no tenant scoping in the invoke protocol. |

**Compensating control today:** the secret only rides trusted hops
(edge → LB → node/brain), inbound `x-fiducia-*` identity headers are stripped at the
edge and LB, the namespace `NetworkPolicy` is default-deny, and the secret is now
delivered via Kubernetes `secretKeyRef` (not env-plaintext) — so the blast radius is
"a compromised in-namespace workload," not the public internet.

**Direction:** per-actor identity with a role split (operator vs service) — distinct
credentials or signed actor tokens — gating operator-grade endpoints
(`scale`/`nodes`/`policies`) and lease ownership on the acting identity rather than a
caller-asserted string. Touches every service, client, and the deploy contract, so it
must be sequenced as a coordinated migration. Includes the "who may `resolve` a
memory claim" question from item 2.

---

## 2. `fiducia-memory.rs` endpoint authentication

**Severity: High (was) · Status: Resolved 2026-07-14 (`44b0933`)**

Previously: no endpoint authenticated the caller and `tenant_id` was supplied in the
request body, so FORCEd RLS defended pool-bleed but not a caller lying about its
tenant; `POST /v1/claims/resolve` had no authorization.

**Resolved:** `src/auth.rs` adds a `require_internal_auth` `route_layer` over all
`/v1` (health public), constant-time-comparing `x-fiducia-internal-auth` against
`FIDUCIA_INTERNAL_SECRET` **fail-closed** (localdev opt-in
`FIDUCIA_ALLOW_INSECURE_INTERNAL=1`). Tenant is now derived from the LB-injected
`x-fiducia-org-id`, and `resolve_tenant()` returns 403 on any body/query `tenant_id`
mismatch — applied to every epistemic + durable handler including
`/v1/claims/resolve`. FORCEd-RLS + `SET LOCAL` remain as defense-in-depth behind the
gate. Covered by 5 unit + 6 middleware e2e tests. The "vectors suggest, never control"
invariant still holds. Residual — *which* authenticated actor may resolve a claim — is
folded into item 1 (per-actor identity).

---

## 3. Login rate-limiting on the web tiers

**Severity: Low · Status: Open**

Neither `fiducia-customer.rs` `POST /login` nor `fiducia-admin.rs` `POST /login`
rate-limits credential submission at the application tier. Both correctly delegate the
Supabase password grant and verify through `fiducia-auth`, but nothing throttles
brute-force / password-spray if Supabase's own limits are relaxed.

**Direction:** enforce at the edge/LB (dd-remote-gateway) where a trustworthy client
IP is available, layered with Supabase throttling — not in the stateless web tier.

---

## 4. Infrastructure hardening (`fiducia-infra`)

**Severity: Low–Info · Status: Partially resolved · remainder report-only**

Static analysis passed (`kubeconform` 14/14, true default-deny NetworkPolicy, no
committed secrets, non-root + read-only-rootfs workloads, no wildcard RBAC).

**Resolved since 2026-07-13:**
- `FIDUCIA_INTERNAL_SECRET` is now wired via `secretKeyRef` (`fiducia-secrets`) in the
  node/sidecar/brain/load-balance manifests — no longer env-plaintext.
- A committed `terraform/envs/prod` now exists (previously only a disposable `e2e`).

**Still open:**
- **Mutable image tags.** First-party `ghcr.io/fiducia-cloud/*` images
  (`node`, `sidecar`, `brain`, `load-balance`) are pinned by the mutable tag `v0.1.0`,
  not by digest. The third-party collector is already digest-pinned; extend the same to
  first-party images.
- **Kubelet-probe NetworkPolicy scope.** `fiducia-allow-kubelet-probes` opens `:8091`
  (sidecar `/meta` + `/metrics` — node identity + re-exposed node metrics) and `:13133`
  (otel) with **no source selector**, so any in-namespace source, not just the kubelet,
  can reach them. Tightening needs per-CNI node-CIDR knowledge and risks breaking probes.
- **Cloud control-plane API exposed by default.** The firewall is opt-in
  (`enable_firewall`/`create_default_rules` default off), so the Kubernetes API `:6443`
  and NodePorts are reachable on unfiltered public IPs unless explicitly enabled —
  observed for hetzner, civo, and vultr modules; the `e2e` env still allows `0.0.0.0/0`.
  Prod instantiation must enable the firewall + `firewall_allowed_cidrs`.
- **Optional:** enable `tls-roots` on `fiducia-telemetry.rs` OTLP export only if
  exporting across an untrusted network (current in-cluster `otel:4317` is plaintext by
  design).

---

## 5. 2026-07-14 re-audit — new code findings (not yet fixed)

Deliberately deferred (repos actively owned by other agents at the time); verified to
exist in source, pending owner fix.

- **`fiducia-messaging.rs` (Medium ×4).**
  1. Pool-based `db::inbox_try_insert` is claim-before-effect: a crash between
     claim-commit and effect **silently drops the effect forever** — use the tx-scoped
     `PgInbox` instead (`db.rs`).
  2. Broker effectively-once needs JetStream `duplicate_window >= claim_ttl` (300s); the
     crate never sets it, so a crash-window republish double-delivers (`db.rs`,
     ARCHITECTURE.md).
  3. The compat relay (`transactional.rs`) publishes via core NATS with **no
     `Nats-Msg-Id`** and holds one tx across batch network I/O → whole-batch re-publish,
     zero broker dedup.
  4. `require_fencing_token` / `is_expired` (`envelope.rs`) are **never enforced on any
     consume/publish path** — a stale/expired envelope is accepted (methods exist but
     have no non-test callers). Note `tenant_id` is envelope-only and unauthenticated;
     all tenants share subjects, so NATS account config must enforce isolation.
- **`fiducia-node.rs` (Low, latent).** Snapshot/fencing is otherwise solid (counter
  serialized in `Store`, deterministic replay off `proposed_at_ms`, snapshot-first fsync
  compaction). But `Store` fields are `#[serde(default)]`, so a snapshot missing
  `next_fencing_token` silently zeroes it → **fencing-token reuse**; `restore()` does no
  invariant check and snapshots carry no checksum. Add a monotonic-floor assertion +
  checksum on restore.

---

## Notes

- **Decided:** `fiducia-monorepo` staying public on GitHub is accepted (operator call).
- Complementary flag-based / CI findings live in [`SECURITY-AUDIT.md`](./SECURITY-AUDIT.md).
- Fixes applied on 2026-07-13/14 were committed locally on each repo's `main`; see each
  service's own history for detail.
