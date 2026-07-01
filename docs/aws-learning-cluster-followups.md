# AWS single-node learning cluster — open follow-ups

Operational follow-ups for the AWS EC2 k8s node (`i-0cc2461a55d491af6`, the single
14-vCPU control-plane-as-worker box that runs the whole `dd` platform **plus** the
soccer continuous self-play learner). Captured Jun 2026 after un-wedging the learner.

Reach: AWS API is **not** laptop-reachable — use SSM
(`AWS_PROFILE=dd-codex AWS_REGION=us-east-1 aws ssm send-command --instance-ids
i-0cc2461a55d491af6 --document-name AWS-RunShellScript --parameters file://cmds.json`),
on-node `sudo kubectl --kubeconfig=/etc/kubernetes/admin.conf`. argocd self-heals,
so durable changes go through this repo's `dev` branch, not `kubectl`.

Context: the soccer learner runs **only** on AWS (Hetzner nodes are too small — it
peaks ~16 GB RAM; Hetzner runs the tournament farm instead). Both clusters build
from source on each push to `origin/learning` (commit-watcher → rollout-restart).
See `soccer-sim-game-engine.rs/k8s/LEARNING_RUNBOOK.md` for the learning loop.

---

## 1. Loki disk-hoard → recurring `disk-pressure` — ✅ FIXED (Jun 2026, commit on `dev`)

`dd-loki` (namespace `observability`) had **no retention/compactor** and an
**unbounded `emptyDir`**; it grew to **~259 GB** (`/data/chunks`), pushed the node
disk to 90% → `node.kubernetes.io/disk-pressure` taint → stranded the learner (and
destabilized the whole node).

**Fixed** in `remote/argocd/observability/`:
- `loki.configmap.yaml`: added a `compactor` (`retention_enabled: true`,
  `delete_request_store: filesystem`, `compaction_interval: 10m`) + `limits_config.retention_period: 72h`.
- `loki.deployment.yaml`: capped the data `emptyDir` at `sizeLimit: 40Gi` (hard
  backstop — kubelet evicts Loki before it can fill the node root disk again).
- Verified: Loki `1/1 Running`, compactor `ACTIVE`, sizeLimit applied, disk 59% / DiskPressure False.

Tuning note: if 72 h of logs approaches 40 Gi (eviction churn), lower `retention_period`
(e.g. 48h/24h) — the cap is the safety net, retention keeps it graceful.
Interim manual reclaim (if ever needed): `kubectl -n observability delete pod -l app=dd-loki`
(emptyDir = ephemeral). Diagnose disk hogs: `sudo du -sh /var/lib/kubelet/pods/* | sort -rh | head`
then map UID→pod via `kubectl get pods -A -o custom-columns=N:.metadata.name,UID:.metadata.uid`.

## 2. `dd-dart-server` + `dd-dev-server-api` ImagePullBackOff (MED — pre-existing, not capacity)

Both reference images that don't exist: `docker.io/library/dd-dart-server:dev` and
`docker.io/library/dd-dev-server:dev`. `docker.io/library/*` is Docker's **official**
namespace, so these refs are misconfigured (they should point at the project's real
registry). They schedule fine now (post CPU-diet) but can never pull → permanent
`ImagePullBackOff`. **Fix:** correct the `image:` refs to the actual registry and
build/push the images, or delete the deployments if dead. Not a node-resource problem.

## 3. Memory is the next binding constraint (MED)

CPU is now healthy (~66% reserved / ~40% used) but **memory sits ~88% reserved** on
the single node: learner 16 Gi, `dd-des-rs` 4 Gi × 3, tournaments 6 Gi + 4 Gi,
`dd-next-1-js`/`dd-music-rs` 4 Gi each. Not blocking today, but new pods will start
stranding on **memory** (not CPU). **Fix when it bites:** a memory-request right-sizing
pass mirroring the CPU diet (trim over-provisioned `requests.memory` to actual usage;
keep the learner's 16 Gi and DB/queue services honest).

## 4. CPU requests right-sized — DONE (context / guardrail)

Root cause of the chronic "99% CPU" was **requests ~10× actual** (boilerplate
100–250m on services idling at 1–5m) on a single node — the scheduler saw a full node
while CPU was ~10% used. Fixed by a 56-manifest sweep (`requests.cpu >= 50m → 25m`
across `remote/argocd/dd-next-runtime` + formal-methods + gcs-router), with
**`dd-des-rs` kept at a full core** (it self-documents needing one for the live soccer
step loop). Reserved 99% → ~66%.

- Guardrail: idle services at 25m **burst** fine because the node is mostly idle; if the
  node ever runs genuinely CPU-saturated, the 25m floors would throttle under contention
  — revisit sizing then. Don't blanket-trim genuine-CPU services (look for `top pods`
  steady usage, not mass-roll startup spikes).

## 5. Build-from-source churn fills disk (LOW — structural)

The learner and every tournament **cold-build from source (~5 min cargo build)** on each
roll, writing to `/tmp` emptyDirs, and completed jobs linger (`ttlSecondsAfterFinished`
= 2 days). Combined with (1), the node root disk trends full. **Options:** use the
existing (currently unused) Dockerfiles to ship **prebuilt images** instead of
build-on-start; lower the tournament job `ttlSecondsAfterFinished` / history limits; add
a node disk-usage alert at ~80%.

## 6. AWS stays single-node (accepted constraint)

Per decision, no second node. So honest right-sizing has a floor of ~40–66% CPU reserved
(real workloads — learner, des-rs, tournaments, gcs — genuinely need CPU); **25–30%
isn't achievable without under-provisioning real services.** The binding constraints on
this node are **memory and disk**, not CPU. Items 1 and 3 are what keep it stable.

---

_Proof the learner is progressing: generation advanced 291 → 329 over a single session
(resumed from RDS on each roll), neural heads training, self-play writing to the
`soccer-self-play-k8s-overnight` experiment._
