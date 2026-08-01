# Fiducia KV secret-delivery — audit addendum & next steps — 2026-07-31

Addendum to [`fiducia-kv-security-audit-2026-07-27.md`](fiducia-kv-security-audit-2026-07-27.md).
This pass re-verified the prior audit against the current `main`, then added a source-level review of
the upstream Rust services (`fiducia-node.rs`, `fiducia-auth.rs`, `fiducia-load-balance.rs`) focused on
the question: **how does the k8s cluster inject/retrieve secrets from fiducia-cloud, and do we need a
custom operator?**

## Headline answer: no custom operator needed

Retrieval already uses the idiomatic pattern — External Secrets Operator (0.18.2) with its generic
`webhook` provider reading fiducia's KV HTTP API
([`fiducia-webhook.yaml`](../remote/argocd/secrets/common/fiducia-webhook.yaml)). The ESO webhook's
`$.entry.value` contract exactly matches the node's found-envelope
(`fiducia-node.rs/src/kv.rs:784-790`). For **injection**, the webhook provider is read-only; add a
thin authenticated `kv:write` sync Job — not a controller. Reserve a custom ESO provider only if we
later need dynamic/leased secrets.

## Correction to the interim verbal audit

An earlier interim read of the LB deployment comment suggested KV was "permissive by default"
(`FIDUCIA_AUTH_MODE`). The source disproves this: **`FIDUCIA_AUTH_MODE` does not exist**; the real
control is `FIDUCIA_AUTH_REQUIRED`, and `/v1/kv` routes are **scope-gated independently** and fail
**closed** (403 `insufficient_scope`) even when `FIDUCIA_AUTH_REQUIRED=false`
(`fiducia-load-balance.rs/src/proxy.rs:224-231, 271-272`). `api.fiducia.cloud/v1/kv` therefore always
requires a scoped API key or JWT — it is **not** an open public endpoint. That comment in
[`fiducia-load-balance.deployment.yaml:78`](../remote/argocd/fiducia/fiducia-load-balance.deployment.yaml)
is stale and should be corrected (see FID-SEC-8).

## Re-verification of the 2026-07-27 findings against current `main`

| 2026-07-27 finding | Prior disposition | State on `main` today |
| --- | --- | --- |
| Critical — auth storage identity vs LB trust contract | Fixed upstream | **Merged** (`security(fiducia): make auth startup and KV identity fail closed`) |
| High — cluster-wide ESO reader unconstrained | Fixed | **Merged** — `ValidatingAdmissionPolicy` + namespace condition present ([`fiducia-external-secret-policy.yaml`](../remote/argocd/secrets/common/fiducia-external-secret-policy.yaml), store `conditions.namespaceSelector`) |
| High — required trust material optional/undeclared | Fixed | **Merged** (`require backend runtime credentials`, `render cloud bootstrap`) |
| High — Raft state ephemeral (`emptyDir`) | Staged | **Still open** — node StatefulSet mounts `/var/lib/fiducia` from `emptyDir` (`fiducia-node.statefulset.yaml:302-304`) → FID-SEC-3 |
| Medium — plaintext in-cluster HTTP | Staged | **Still open** — every KV hop is `http://` → FID-SEC-1 |
| Medium — spoofable `dd.dev/fiducia-client` label | Fixed | **Merged** — explicit allowlist now, label demoted to a non-trust comment (`fiducia-load-balance.networkpolicy.yaml:16-47`) |
| Medium — mutable in-pod build source | Staged | **Still open** — pods `git clone --branch main` + `cargo run` at startup → FID-SEC-7 |

## New findings from the source-level review

**N1 — No KV access audit logging (High for a secrets manager).** Node/auth/LB emit only generic
`TraceLayer` HTTP spans; there is **no record of which identity read/wrote/deleted which key**
(`fiducia-node.rs/src/main.rs:152,165`; only a crypto-error line at `kv.rs:964-965`). The delivery doc
references "audit tooling" that does not exist in code. → FID-SEC-2.

**N2 — At-rest encryption is opt-outable by any writer (Medium).** `PUT /v1/kv` accepts
`"plaintext": true` and stores the value verbatim (`kv.rs:725-736, 695-705`). Any `kv:write` key can
persist an unsealed secret; nothing gates this behind admin scope or keyspace. → FID-SEC-5.

**N3 — Key scoping is per-org only; no prefix/path ACLs (Medium).** Within an org, any `kv:read` key
reads *every* KV value (`fiducia-auth.rs` scopes are resource/verb only; `routing.rs:17,53-65` isolates
by org; documented in `docs/fiducia-secret-delivery.md:37-39`). Compensated on the ESO side by the
admission policy's `k8s/<ns>/<workload>/<ENV>` key convention, but not at the fiducia layer for
non-ESO callers. → FID-SEC-4.

**N4 — No managed injection path (Medium).** ESO's webhook provider is read-only; the only real write
path is a raw authenticated `PUT` through the LB (`kv.rs:804-831`; `fiducia-cli.rs` has **no** kv
command despite the runbook saying "prefer the CLI"). Injection today is ad-hoc. → FID-SEC-6.

## Confirmed strengths (no action)

- Values are sealed **on the node, before entering Raft** — AES-256-GCM, random nonce, AAD binds
  key-id + storage key, `fcenc:v2:` envelope; log/snapshots/replicas hold only ciphertext; partial key
  config fails the pod closed (`kv.rs:103-117, 164-271, 336-359, 695-723`). Versioned keyring + Vault
  Transit backends, with rotation.
- Trusted-hop gate (`x-fiducia-internal-auth`) fails closed and the release build compiles out the
  insecure escape hatch (`internal_auth.rs:75-78, 107-146`); the LB strips any client-supplied copy
  (`auth.rs:322-340`), so the public gateway hop cannot forge cluster-internal trust.
- ESO is deliberately handed a **read-only** `kv:read` key; writers use a separate `kv:write` key.

## Next-steps backlog (filed in Linear)

Filed under Linear epic **[DEN-1236](https://linear.app/denman/issue/DEN-1236)** — "fiducia-cloud
secret delivery — hardening phase 2 (k8s ↔ KV)" (team Denman, label *Security hardening*, related to
DEN-1164). Priorities relative to a secrets-manager posture; "Area" indicates which repo owns the
change.

| Linear | ID | Title | Priority | Area | Depends on |
| --- | --- | --- | --- | --- | --- |
| DEN-1240 | FID-SEC-1 | TLS on the in-cluster KV path | High | k8s-cluster + fiducia-infra | — |
| DEN-1241 | FID-SEC-2 | KV access audit logging → Loki | High | fiducia-monorepo (node + LB) | — |
| DEN-1242 | FID-SEC-3 | Raft durability: `emptyDir` → PVC | High | k8s-cluster + fiducia-infra | — |
| DEN-1243 | FID-SEC-4 | Prefix/path ACLs on KV keys | Medium | fiducia-monorepo (auth + node) | — |
| DEN-1244 | FID-SEC-5 | Gate/forbid `plaintext:true` writes | Medium | fiducia-monorepo (node) | — |
| DEN-1245 | FID-SEC-6 | Managed injection Job (AWS SM → KV) | Medium | k8s-cluster | FID-SEC-1, FID-SEC-4 |
| DEN-1246 | FID-SEC-7 | Immutable images, deploy by digest | Medium | fiducia-monorepo CI + k8s-cluster | — |
| DEN-1247 | FID-SEC-8 | Fix doc/manifest drift | Low (**done**) | k8s-cluster + docs | — |

### FID-SEC-1 — TLS on the in-cluster KV path (High)
Bearer credentials and unsealed secret values currently cross the pod network in cleartext. The LB
already supports TLS.
**Acceptance:** cluster-trusted cert issued + mounted on the LB; HTTPS service port (`:8443`); ESO
store `caProvider` configured and switched to `https://`; gateway upstream and all in-cluster callers
moved; non-probe plaintext rejected; rollback tested; `fiducia-secret-delivery.test.ts` asserts no
plaintext KV upstream on a public vhost.

### FID-SEC-2 — KV access audit logging (High)
Emit a structured audit event per KV read/write/delete: identity (`org_id`, `key_id`), key, revision,
result, timestamp; export to Loki with redaction of the value. **Acceptance:** every `/v1/kv`
GET/PUT/DELETE produces one audit line; values never logged; queryable in Loki; unit test asserts an
event on each verb.

### FID-SEC-3 — Raft durability: emptyDir → PVC (High)
Pod replacement can erase a replica's Raft state; correlated loss can destroy KV authority.
**Acceptance:** `volumeClaimTemplates` with sized PVC + reclaim policy; documented backup/restore with
a tested restore; quorum-aware rollout (never restart all members together); rollback evidence.

### FID-SEC-4 — Prefix/path ACLs on KV keys (Medium)
Scope keys to key prefixes, not just org, so a leaked `kv:read` key can't read the whole org.
**Acceptance:** key model carries an allowed-prefix set; node/LB enforce it on read+write; ESO reader
keys scoped to their `k8s/<ns>/<workload>/` prefix; tests cover allow + cross-prefix deny.

### FID-SEC-5 — Gate/forbid `plaintext:true` writes (Medium)
**Acceptance:** `plaintext:true` requires `admin:write` (or is removed for secret keyspaces); a
`kv:write` key attempting it gets 403; test covers the reject path.

### FID-SEC-6 — Managed injection Job (Medium)
Thin CronJob/Job (or ESO PushSecret generator) syncing selected AWS SM entries → fiducia KV via a
`kv:write` key, authenticated and audited — the managed counterpart to the read webhook.
**Acceptance:** idempotent PUT with `Idempotency-Key`; least-privilege `kv:write` key scoped by prefix
(needs FID-SEC-4); runs over TLS (needs FID-SEC-1); NetworkPolicy allows only the Job → LB.

### FID-SEC-7 — Immutable images, deploy by digest (Medium)
Replace in-pod `git clone main && cargo run` with CI-built, scanned, signed images pinned by digest.
**Acceptance:** images built+signed in CI with SBOM/provenance; manifests reference digests; startup
no longer clones or compiles; egress narrowed.

### FID-SEC-8 — Fix doc/manifest drift (Low — **done**, DEN-1247)
- `docs/fiducia-secret-delivery.md`: replaced "Prefer the `fiducia` CLI or an authenticated admin UI"
  with the accurate HTTP-API write path (the CLI has no `kv` subcommand; the admin UI has no KV-write
  endpoint). **Applied in this change.**
- The nonexistent-`FIDUCIA_AUTH_MODE` LB comment was **already fixed** by the merged hardening series:
  the deployment now sets `FIDUCIA_AUTH_REQUIRED=true` explicitly with an accurate comment, and
  `/v1/kv` is scope-gated regardless. No further action.
