# dd-build-server — hardening pass

*Security audit + remediation, 2026-07-21. Four-dimension audit (command
execution, webhook/auth/secret handling, k8s RBAC/pod security, fiducia
coordination). This doc records what was **applied in code** and what still
needs an **operator/architectural decision**.*

dd-build-server is the highest-blast-radius workload in the cluster: it clones
arbitrary repos, builds images against the host containerd/buildkit sockets,
runs `kubectl apply` into `default`, and can seal cluster secrets out to GitHub.
Treat every change here as production-sensitive.

---

## Part 1 — applied in code (this branch)

All changes are in `remote/deployments/build-server-rs/` unless noted. The crate
builds clean and all 12 unit tests pass (`cargo test`), including new regression
tests.

### Command execution / injection

| Fix | Where | What it closes |
|---|---|---|
| Canonicalize + containment-check every repo-relative path | `src/main.rs` `resolve_repo_path` (now async) | In-repo symlink (`ctx -> /`) redirecting build context / Dockerfile / deploy manifest to a host path. Resolved target must stay under the canonicalized repo root. |
| Reject `,` `=` `:` and control chars in path components | `src/main.rs` `validate_relative_path` | `--mount` spec injection: `contextDir=x,src=/home/ec2-user` added a second `src=` field → arbitrary host dir bind-mounted into a profile container (node-root via the mounted socket). |
| `TYPE/NAME`-only rollout resource; reject leading `-` | `src/main.rs` `validate_rollout_resource` | `deploy.rollout=--kubeconfig=… / --server=…` reaching kubectl argv as a flag. |
| Reject leading-dash image | `src/main.rs` `validate_image` | `-t`/`push` positional flag injection into nerdctl. |
| Allowlists fail **closed** on empty | `src/main.rs` `ensure_allowed_prefix` | A dropped `BUILD_SERVER_ALLOWED_{REPO,IMAGE}_PREFIXES` silently reverting to "build any repo/image". |

### Secret handling / leakage

| Fix | Where | What it closes |
|---|---|---|
| Suppress DSN in DB-connect panic | `src/main.rs` (db connect) | sea-orm/sqlx inline the full connection string (with password) on a parse failure → pod logs. Error is now dropped unprinted. |
| Drop `#[derive(Debug)]` on `AwsCredentials`, `EcrAuthorizationData` | `src/main.rs` | A stray `{:?}`/`dbg!` printing a live AWS secret key or ECR token. |
| GH secret-sync **owner allowlist** (`BUILD_SERVER_GH_SYNC_ALLOWED_OWNERS`), **fromEnv allowlist** (`BUILD_SERVER_GH_SYNC_ALLOWED_ENV` or `GH_SYNC_` prefix), and a hardcoded **crown-jewel denylist** | `src/gh_secrets.rs` `parse_rules` + `SyncPolicy` | The exfil chain: patch the rules ConfigMap → seal `AWS_SECRET_ACCESS_KEY`/`GH_PAT`/`FIDUCIA_API_KEY`/`SERVER_AUTH_SECRET`/`DATABASE_URL` to an attacker repo. These env names can now **never** be synced, and only allowlisted owners can receive anything. Fail-closed when the owner list is empty. |
| Strict `owner/name` repo charset | `src/gh_secrets.rs` | `x/../../gists` dot-segment normalization into a different GitHub endpoint. |

### Webhooks

| Fix | Where | What it closes |
|---|---|---|
| `valid_commit_sha` (7–64 hex) gate + char-slice `substitute_image` | `src/webhooks.rs` | Byte-slice panic on a non-ASCII `after`/`head_sha` from a (signed) webhook, which dropped the connection and orphaned the idempotency lease. |
| Hash registry secret before `ct_eq` | `src/webhooks.rs` `registry_secret_ok` | Length leak: `subtle` short-circuits on length mismatch over the public `/webhooks/` path. |
| Require `X-Delivery-Id` (1–128 chars) | `src/webhooks.rs` `registry_webhook` | Dedupe bypass: the old timestamp fallback made every request unique, so omitting the header defeated dedupe and let a caller flood the image subject. |

### Schema

- `build_jobs_job_kind_chk` now includes `'run-profile'`
  (`remote/libs/pg-defs/schema/databases/dd_build_server/schema.sql`). Previously
  every profile job's persist silently failed the check constraint — i.e. **no
  audit row for exactly the jobs that execute third-party repo code**.
  **Action required:** converge with `scripts/dpm.sh` (declarative, never at
  boot). Generated pg-defs do not embed check constraints, so no regeneration.

### Manifest guardrails (safe subset only)

`remote/argocd/dd-next-runtime/dd-build-server.deployment.yaml`:
- `requests.ephemeral-storage: 4Gi`, `limits.ephemeral-storage: 24Gi`
- `tmp` emptyDir `sizeLimit: 24Gi`

Caps pod-local build scratch so a runaway build can't fill the node disk and
evict neighbours. (Image layers land in the node containerd store and are
bounded separately by image GC — see Part 2 F4.)

---

## Part 2 — needs your decision (NOT applied)

These are architectural or cluster-policy changes with real blast radius. Each
has a concrete diff/command ready; none were applied unilaterally.

### A. CRITICAL — the deployment compiles `origin/dev` at runtime (audit F13)

The pod runs `image: docker.io/library/rust:1.90-bookworm` with an inline
`git clone --depth 1 --branch dev … && cargo run --release` at every start. The
well-built multi-stage `Dockerfile` is **never used**. Consequences: no supply
chain gate (anyone who can push `dev` gets node-root RCE, no review), silent
version drift between pods, full Rust toolchain resident in a node-root pod, and
a fallback to a developer working tree (`/home/ec2-user/codes/dd/dd-next-1`) on
clone failure.

The webhook rule at `dd-build-server.configmap.yaml` already builds this exact
image on push to `dev`, so the ECR artifact exists — the deployment just doesn't
consume it. **Decision:** switch `image:` to the ECR digest and delete the
clone-and-compile script, the `BUILD_SERVER_GIT_URL/_REF` env, the `repo`
hostPath, and the `GH_PAT` clone path. This is the single highest-value change.

### B. CRITICAL — containerd + buildkit host sockets = node root (audit F1)

`hostPath` mounts of `/run/containerd/containerd.sock` +
`/run/buildkit/buildkitd.sock` make the pod node-root; the `cap-drop: ALL` /
`no-new-privileges` securityContext is cosmetic against a socket. Single-node
cluster ⇒ node-root = cluster-root. **Options:** (1) rootless BuildKit in its own
pod over mTLS TCP, drop both sockets; (2) interim: move dd-build-server to a
dedicated tainted node group so node-root ≠ cluster-root. Also builds into the
`k8s.io` containerd namespace (F4) — image-store poisoning of what the kubelet
runs; build into a private `dd-build` namespace + pull back via ECR.

### C. HIGH — fiducia coordination is failing open, unprovisioned (audit F10)

Confirmed in code, not yet on the live cluster (my kubectl context here is a
local `kind` cluster, not EC2). `FIDUCIA_API_KEY` is `optional: true` from a
`dataFrom: extract` bag; if absent, the client sends no bearer token → 401 →
`LockOutcome::Unavailable` → with `BUILD_SERVER_COORDINATION_REQUIRED=false`,
every build logs a warning and runs on the local semaphore. Metrics/logs look
like coordination while providing none. Masked today by `replicas: 1` +
`MAX_CONCURRENT_BUILDS=1`; the day replicas go to 2, concurrent builds race the
same ECR tag and `kubectl apply` with no fencing.

**Verify on the EC2 cluster:**
```bash
aws secretsmanager get-secret-value --secret-id dd/remote-dev/build-server-secrets \
  --query SecretString --output text | jq 'keys'          # is FIDUCIA_API_KEY present?
kubectl --context <ec2> get secret dd-build-server-secrets -n default \
  -o jsonpath='{.data.FIDUCIA_API_KEY}' | head -c 12       # empty = unprovisioned
kubectl --context <ec2> logs deploy/dd-build-server -n default \
  | grep -c 'coordination unavailable'                     # >0 = failing open now
```
**Fix:** mint a `locks:write`-scoped key via fiducia-auth, place it in the AWS SM
bundle build-server actually reads (`dd/remote-dev/build-server-secrets` — note
dd-contract-service was told `dd-agent-secrets`, a *different* bundle; consolidate
to one `dd/remote-dev/fiducia-*` object for all consumers), then set
`optional: false` + `BUILD_SERVER_COORDINATION_REQUIRED=true`. Interim: alert on
the already-exported `lock_failures` counter so the silent path becomes loud.

### D. HIGH — RBAC `create deployments` in an unlabeled namespace (audit F5, F3)

The SA Role is otherwise well-scoped (namespaced, no secrets/pods/rolebindings).
But `create` on `apps/deployments` in `default` — which has **no Pod Security
Admission label** — lets it author a PodSpec with `hostPath: /` / `privileged` /
`serviceAccountName: <any SA in default>` and borrow a more-privileged identity.
**Decision:** (1) give dd-build-server its own `dd-builds` namespace with
`enforce: baseline`, deploy rights there only; cross-namespace via Argo CD.
(2) A ValidatingAdmissionPolicy rejecting hostPath/hostNet/privileged/foreign-SA
from this SA. Labeling the shared `default` namespace was **not** done here — it
would affect every other workload in `default` and is its own rollout.

### E. MEDIUM — egress, credentials, PAT-in-argv

- Egress allows `0.0.0.0/0:443` (IMDS *is* correctly blocked). Front with an
  FQDN-allowlisting proxy (github.com, `*.dkr.ecr.*`, `*.amazonaws.com`).
- Static `AWS_ACCESS_KEY_ID/SECRET` instead of IRSA → durable creds on any escape.
- `GH_PAT` base64 in `git -c http.extraheader=` argv is readable via
  `/proc/*/cmdline`. Dissolves if B/A remove the clone path; else move to a
  0600 file + `include.path`.

### F. LOW — housekeeping

- `webhook_deliveries` has no retention/pruning → unbounded growth.
- Build-arg secret filter is a 5-substring denylist; prefer an allowlist.
- `/healthz` and `/` echo the full allowlist config unauthenticated (in-cluster
  only today) — hands an attacker the prefixes to craft a passing `repoUrl`.

---

## Verified sound (audits confirmed — do not "fix")

git clone transport hardening (`protocol.{ext,file,local}.allow=never` + `--`);
GitHub webhook HMAC verified constant-time before any side effect, fails closed
on missing secret; webhook→build takes repo_url from operator rules, never the
payload (no fork-PR build); sealed-box crypto correct; no secret interpolation in
tracing; `Config` deliberately not `Debug`; ECR password via `--password-stdin`;
compiled-in profile catalog; correct probe design.
