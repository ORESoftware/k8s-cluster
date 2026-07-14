# Open architectural security findings

Cross-repo security items surfaced by the **2026-07-13 full-platform audit** that
were **deliberately not fixed** because each needs a design decision spanning
multiple services, clients, and/or the deployment contract — not a low-risk local
patch. Concrete, low-risk defects found in the same audit were fixed and committed
in their respective repos; this document tracks only what remains open.

> Status legend: **Open** = not started · **Design** = needs a design decision
> before implementation · **Mitigated** = a compensating control limits impact today.

---

## 1. Platform-wide shared-secret identity (biggest theme)

**Severity: Medium–High · Status: Design · Mitigated (network isolation)**

A single `FIDUCIA_INTERNAL_SECRET` authorizes **all** internal `/v1` traffic across
the platform. It is verified fail-closed with a constant-time compare (good), but it
conveys **no per-actor identity and no operator-vs-service distinction** — every
holder of the secret is fully trusted for every internal action.

Consequences observed:

| Service | Exposure |
|---|---|
| `fiducia-brain.rs` | Any secret-holding caller (node sidecars, LB, …) can `POST /v1/scale`, `DELETE /v1/nodes/{id}`, `POST /v1/policies` — i.e. drain or rescale the cluster. No per-actor identity or audit trail on operator-grade actions. |
| `fiducia-ai-agent-bridge.rs` + `fiducia-ai-agent-control-plane` | `from` (messages) and `agent_key` (file leases) are caller-asserted under the shared bearer. Any authenticated caller can read any channel it never joined and, in standalone mode, release/steal another agent's file lease. |
| `fiducia-lambda-service.rs` | Any secret holder can invoke **any tenant's** function — there is no tenant scoping in the invoke protocol. |

**Compensating control today:** the secret only rides trusted hops
(edge → LB → node/brain), inbound `x-fiducia-*` identity headers are stripped at the
edge and LB, and the namespace `NetworkPolicy` is default-deny — so the blast radius
is "a compromised in-namespace workload," not the public internet.

**Direction:** introduce per-actor identity with a role split (operator vs service),
e.g. distinct credentials or signed actor tokens, and gate operator-grade endpoints
(`scale`/`nodes`/`policies`) on the operator role. This touches every service, every
client, and the deploy contract, so it must be sequenced as a coordinated migration.

---

## 2. `fiducia-memory.rs` endpoint authentication

**Severity: High (residual) · Status: Open · Mitigated (loopback bind)**

No endpoint authenticates the caller, and `tenant_id` is **supplied in the request
body**. FORCEd row-level security is applied per-transaction on every table path
(verified), so RLS defends against connection-pool bleed — but it binds the GUC to
the *same caller-supplied* `tenant_id`, so it does **not** stop a caller that lies
about its tenant. Additionally, `POST /v1/claims/resolve` — the only path to
authoritative truth in the claim ledger — has no authorization: any caller can
accept or reject any claim.

**Compensating control today:** the service binds `127.0.0.1:8100` and is documented
as trusting its upstream caller (ARCHITECTURE.md §5.7). The "vectors suggest, never
control authoritative state" invariant **does** hold — recall applies tenant/validity
as hard filters before scoring, and vector output never writes back to authoritative
state.

**Direction:** authenticate callers and derive `tenant_id` from a verified identity
(not the request body); add authorization to `claims/resolve`.

---

## 3. Login rate-limiting on the web tiers

**Severity: Low · Status: Open**

Neither `fiducia-customer.rs` `POST /login` nor `fiducia-admin.rs` `POST /login`
rate-limits credential submission at the application tier. Both correctly delegate the
Supabase password grant and verify through `fiducia-auth`, but nothing throttles
brute-force / password-spray if Supabase's own limits are relaxed.

**Direction:** enforce at the edge/LB (dd-remote-gateway) where a trustworthy client
IP is available, layered with Supabase throttling — not in the stateless web tier
(which would need shared state and a trusted client-IP source anyway).

---

## 4. Infrastructure hardening (`fiducia-infra`)

**Severity: Low–Info · Status: Open · report-only (production-impacting)**

Static analysis passed (`kubeconform` 14/14, true default-deny NetworkPolicy, no
committed secrets, non-root + read-only-rootfs workloads, no wildcard RBAC). Residual
items, left as report-only because they are production-impacting or need
environment-specific values:

- **Mutable image tags.** First-party `ghcr.io/fiducia-cloud/*` images
  (`node`, `sidecar`, `brain`, `load-balance`) are pinned by the mutable tag `v0.1.0`,
  not by digest. The third-party collector is already digest-pinned; extend the same
  to first-party images.
- **Kubelet-probe NetworkPolicy scope.** `fiducia-allow-kubelet-probes` opens `:8091`
  (sidecar `/meta` + `/metrics` — node identity + re-exposed node metrics) and `:13133`
  (otel) with **no source selector**, so any in-namespace source, not just the kubelet,
  can reach them. Tightening needs per-CNI node-CIDR knowledge and risks breaking probes.
- **No committed hardened `prod` Terraform env.** Only a disposable `e2e` env is
  committed, and its defaults leave the cluster API open to `0.0.0.0/0`. The
  prod-hardening toggles (private endpoint, `authorized_api_cidrs`, private subnets,
  network-policy enforcement, firewalls) exist as opt-in variables but no committed env
  asserts them — prod must set them explicitly at instantiation.
- **Optional:** enable `tls-roots` on `fiducia-telemetry.rs` OTLP export only if
  exporting across an untrusted network (current in-cluster `otel:4317` is plaintext
  by design).

---

## Notes

- **Not fixed here:** everything above is intentionally deferred; see each service's
  own audit notes for the low-risk fixes that *were* applied on 2026-07-13.
- **Decided:** `fiducia-monorepo` staying public on GitHub is accepted (operator call).
- The complementary flag-based/CI findings live in [`SECURITY-AUDIT.md`](./SECURITY-AUDIT.md).
