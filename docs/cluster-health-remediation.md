# Hetzner cluster health & remediation

Triage of the Hetzner k8s cluster (2026-07-18), ranked by impact, with the fix
for each. All findings are from read-only inspection. The cluster is
**functioning but running with zero control-plane redundancy and a namespace
that is ~half down**. Nodes: 3 control-plane (fsn1/nbg1/hel1) + 2 workers, all
`Ready`, v1.31.14.

Re-verify current state before acting — restart counts and pod names change.

---

## P0 — CRITICAL: etcd has no quorum (surviving on a single member, nbg1)

**Evidence:**
- `etcd-dd-k8s-nbg1`: `1/1 Running`, 0 restarts — the **only** healthy etcd.
- `etcd-dd-k8s-fsn1` / `hel1`: CrashLoopBackOff, ~4500+ restarts each.
- `kube-apiserver-dd-k8s-nbg1`: `1/1 Running`; `fsn1`/`hel1` apiservers
  crashloop purely because their local etcd is down
  (`dial tcp 127.0.0.1:2379: connect: connection refused` →
  `Error creating leases: context deadline exceeded`, exit 255).
- etcd-fsn1 `--previous` log: `the member has been permanently removed from the
  cluster` → `data-dir used by this member must be removed`. Its `/var/lib/etcd`
  still holds the old 3-member view, so it replays its own removal ConfChange
  on every boot and exits. hel1 is identical.

**Root cause:** fsn1 and hel1 were `member remove`d from etcd, but their
`/var/lib/etcd` was never wiped. The cluster is a **single-member etcd** — one
node failure = total, unrecoverable cluster loss (no quorum). This is the most
urgent item.

**Fix (operator; do fsn1 fully, verify, then hel1):**
```sh
# 0. Confirm from a healthy member which nodes are real:
kubectl -n kube-system exec etcd-dd-k8s-nbg1 -- etcdctl member list \
  --endpoints=https://127.0.0.1:2379 \
  --cacert=/etc/kubernetes/pki/etcd/ca.crt \
  --cert=/etc/kubernetes/pki/etcd/server.crt --key=/etc/kubernetes/pki/etcd/server.key
# On the STALE node (fsn1):
mv /etc/kubernetes/manifests/etcd.yaml /root/etcd.yaml.bak   # stop the static pod
rm -rf /var/lib/etcd                                          # wipe the stale data-dir
# From a healthy node, re-add fsn1 as a member:
etcdctl member add dd-k8s-fsn1 --peer-urls=https://10.20.0.2:2380
# Edit /root/etcd.yaml.bak: set --initial-cluster-state=existing and the full
# 3-peer --initial-cluster list, then restore it:
mv /root/etcd.yaml.bak /etc/kubernetes/manifests/etcd.yaml
# etcd resyncs from nbg1; the local apiserver recovers automatically.
```
Verify `etcdctl endpoint health` shows 2 healthy members before touching hel1.
This is destructive (wipes a data-dir) — do it deliberately, one node at a time.

---

## P1 — DiskPressure eviction storm (concentrated on the etcd node)

**Evidence:** `default` pod phases ≈ 1534 Evicted, 389 ContainerStatusUnknown,
49 CrashLoopBackOff, 39 Running. Sample: `Pod was rejected: The node had
condition: [DiskPressure]`. Eviction distribution: **nbg1 901**, wrk2 630, wrk1
105, fsn1 41. DiskPressure is intermittent (builds up → evicts → recovers).

**Root cause:** the in-pod build pattern (cargo/pnpm/git-clone onto node disk)
plus thousands of crashloop restarts fill node disk → DiskPressure → mass
eviction; tombstones are never GC'd. **Dangerous correlation: nbg1 is both the
sole healthy etcd AND the most-evicted node** — etcd is disk-sensitive, so a bad
episode on nbg1 can take down the whole control plane (P0).

**Fix:**
```sh
kubectl -n default delete pod --field-selector status.phase=Failed   # GC tombstones
# On each node: crictl rmi --prune                                    # prune images
```
Then set kubelet image-GC / `evictionHard` thresholds and add
`ephemeral-storage` requests/limits to build pods. The real reduction comes from
fixing P2 (below).

---

## P2 — ~49 of 103 `default` deployments down: in-pod-build source not provisioned

**Evidence:** identical root cause across services (`--previous` logs):
- `dd-browser-test-server` (2500+ restarts): `cd:
  /opt/dd-next-1/remote/deployments/browser-test-server: No such file or
  directory` (`dd-browser-test-server.deployment.yaml` `cd` line + the hostPath
  mount). Two ReplicaSets crashloop at once because
  `strategy.rollingUpdate.maxUnavailable: 0`.
- `dd-escrow-rs`, `dd-trading-server`, `dd-agent-worker-broker`, … : same
  `cd: /opt/dd-next-1/remote/deployments/<svc>: No such file or directory`.
- Variant — `dd-rust-vapi-phone`: `Cloning into '/tmp/…'` then death (git
  clone never completes — egress/auth), distinct from the missing-path variant.

**Root cause (matches the memory note): in-pod build-script bugs, NOT secrets.**
The node hostPath repo `/home/ec2-user/codes/dd/dd-next-1` is a near-empty
skeleton, so every service that `cd`s into its own `remote/deployments/<svc>`
fails; a second cohort fails on git-clone egress. **athleto works because it
clones fresh into `/tmp` and its clone succeeds — that is the proven-good
pattern.**

**Fix:** either repair whatever syncs the `dd-next-1` checkout onto the nodes
(populate the full `remote/deployments/*` tree), or migrate these services to
the `/tmp`-clone-at-startup pattern that `dd-athleto-app-rs` uses. One fix
recovers most of the 49 deployments and eliminates most of the P1 disk churn.
For `dd-browser-test-server` specifically (blocks the athleto + fiducia browser
E2E CronJobs): switch it to the `/tmp`-clone pattern, or set `replicas: 0` to
immediately stop its crashloop + eviction churn until it's ready.

---

## P4 — athleto is healthy (context)

`dd-athleto-app-rs` revision `ac10a2c`, `1/1 Running`, 0 restarts; migrations
all applied (`_sqlx_migrations` 1-8 `success=t`, `b2b_approved_at` exists — the
checksum-reconciliation fix took). One low-severity note: the migrator hits the
Supabase pooler's ~120s `statement_timeout` on its advisory lock (harmless when
no migrations are pending; noisy). Prefer a direct connection + bounded
`lock_timeout` for the migrator.

---

## P5 — lower / informational

- **ingress-nginx** memory note is STALE: all 3 controllers `1/1 Running`, 0
  restarts. Resolved.
- **hcloud-csi-node** had ~4000 restarts but has stabilized (`3/3 Running`);
  monitor.
- **DNSConfigForming** warnings (16k+ on CP nodes): node `/etc/resolv.conf` has
  >3 nameservers incl. IPv6 → kubelet truncates. Cosmetic; clean up resolv.conf.
- **ArgoCD**: `dd-next-runtime` = `Synced / Degraded` (aggregate of the 49 down
  deployments, all P2). Several other apps Degraded roll up to the same cause; a
  few are `OutOfSync/Missing` or `Unknown`.

---

**Bottom line:** athleto itself is fine. The urgent work is infrastructure —
**restore etcd quorum (P0)** before anything else, relieve **DiskPressure on
nbg1 (P1)**, then fix **in-pod-build source provisioning (P2)**, which clears
~49 deployments and stops the disk churn feeding P1.
