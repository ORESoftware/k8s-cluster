# Browser automation runners on the remote clusters — 2026-07-25

Root cause of the 23-day Hetzner outage, the fix, and what is still fragile.

## State

Both clusters serve traffic as of this writing; an in-cluster probe of each
Service returns 200 on Hetzner and on AWS.

| Cluster | Workload | State |
|---|---|---|
| AWS EC2 | `dd-browser-test-server` | Running 1/1 |
| AWS EC2 | `dd-selenium-server` | Running 2/2 |
| Hetzner | `dd-selenium-server` | Running 2/2, 0 restarts — **fixed**, was 1/2 with ~6 650 restarts over 23 days |
| Hetzner | `dd-browser-test-server` | Running 2/2 on a fragile node-local snapshot; a newer ReplicaSet is crashlooping — see below |

AWS was verified live, not just by pod status: a bounded scenario was driven
through the dedicated Selenium Grid against a cluster-internal URL and returned
`ok: true` in ~700–900 ms, and unauthenticated `POST /run` was correctly
refused with 401.

The three drivers on `dd-browser-test-server` (AWS) also each executed a real
in-cluster scenario: playwright 1.56.0 (~180 ms), puppeteer 24.43.1 (~160 ms),
selenium 4.44.0 (~775 ms). Selenium's extra latency is expected — it opens a
fresh `RemoteWebDriver` session against the Grid sidecar per run.

## Root cause on Hetzner

The `selenium` Grid sidecar was **healthy** throughout and creating Chrome
sessions normally. The failing container was `selenium-api`, and its last log
line was the whole story:

```
/bin/bash: line 2: cd: /opt/dd-next-1/remote/deployments/selenium-server: No such file or directory
```

Both services mount their source from a hostPath:

```yaml
volumes:
  - name: repo
    hostPath:
      path: /home/ec2-user/codes/dd/dd-next-1
      type: Directory
```

That path is **specific to the EC2 node**. Hetzner syncs the *same*
`remote/argocd/dd-next-runtime` path — the cluster app list in
`remote/argocd/clusters/hetzner/applications.yaml` points at it directly, with
no overlay — so it gets manifests that assume a host layout it does not have.
The container starts, cannot find its source, and exits 1 forever.

This is not a Hetzner-specific bug so much as a portability gap: every
source-mounted service in `dd-next-runtime` has the same exposure. It has been
failing silently for 23 days because a crashlooping pod still has a Service and
a ClusterIP — nothing notices until something actually needs the runner.

`dd-rust-vapi-phone` does not have this problem, because it already solves it.

## The fix — `dd-selenium-server`

An `initContainers: [fetch-source]` that shallow-clones the public superproject
into an `emptyDir` mounted at `/opt/dd-next-1`, replacing the hostPath volume
entirely. The application containers are untouched: they still `cd
/opt/dd-next-1/...`, the path is simply populated by the pod itself now rather
than by whatever happens to exist on the node.

This is better than threading a clone into the app container's own startup
script (the first approach tried here): the app args stay simple, an init
failure shows up as a distinct pod phase instead of a crashloop, and the clone
completes before the app starts rather than racing it.

The clone is idempotent across init restarts (`if [ ! -e /opt/dd-next-1/.git ]`)
because the emptyDir outlives them, and `git config --global --add
safe.directory '*'` avoids git's dubious-ownership abort.

This landed on `dev` in
[PR #34](https://github.com/ORESoftware/k8s-cluster/pull/34) (`f0565086`,
"run on any cluster (self-contained clone) + fix semconv NoClassDefFound"), and
Hetzner has been healthy since: two pods, 2/2 Ready, zero restarts.

Worth recording how easy it was to misread the situation. `main` did not have
the fix and the Argo app showed `OutOfSync`, which looks exactly like "the
working fix exists only in the cluster and selfHeal is about to revert it".
It wasn't — the fix was on `dev`, which is the branch Argo actually tracks, and
`main` was simply behind. Comparing against the deployed branch rather than the
checked-out one is what settles that question.

## `dd-browser-test-server` is a different, harder problem

The same hostPath applies, but this service **cannot** be fixed by cloning the
public superproject alone. Its `package.json` declares:

```json
"@dd/telemetry": "file:../../libs/telemetry-node"
```

and `remote/libs` is a **private submodule**
(`git@github.com:ORESoftware/k8s-libs-and-shared-defs.git`). A public-only
clone leaves that path empty, and the build dies with:

```
ENOENT: no such file or directory, scandir '/opt/dd-next-1/remote/libs/telemetry-node'
```

which is exactly what a `fetch-source`-equipped pod was observed doing during
this work. So the fix needs credentials — `GH_PAT` or `GH_DEPLOY_KEY` from
`dd-agent-secrets` — to fetch `remote/libs` as well. That is a real design
decision (which credential, mounted how, and whether the init container should
hold repo-write-capable creds at all), not a mechanical edit.

The two currently-Running replicas work only because someone copied a snapshot
to `/home/ec2-user/codes/dd/dd-next-1` on some Hetzner nodes. That directory is
**not a git checkout** (`fatal: not a git repository`), so it can never be
updated, exists on only some of the five nodes, and makes scheduling a lottery.
It should not be relied on.

**Deliberately not edited here.** The same fix was attempted on `dev`
(`8fec84ee`, "self-heal source via per-pod clone") and **reverted**
(`32f9f3a6`) — the private-submodule dependency above is why. A crashlooping
`fetch-source`-equipped pod was still visible in-cluster during this work,
which is that attempt. Re-landing the same change without solving the
credential question would just reproduce the revert.

## Status

| Change | State |
|---|---|
| `dd-selenium-server.deployment.yaml` | **no change needed** — already fixed on `dev` via PR #34 |
| `dd-remote-gateway.configmap.yaml` — `/vxl/vapi/webhook` | the only new change here; safe to ship before Voxletra exists (variable upstream) |
| `dd-browser-test-server.deployment.yaml` | left alone — the attempted fix was reverted on `dev`; needs the credential decision above |

`kubectl kustomize remote/argocd/dd-next-runtime` builds clean, and the
rendered selenium Deployment carries `initContainers: [fetch-source]` with both
volumes as `emptyDir`.

## Worth checking separately

- Hetzner `dd-browser-job-runner` has a large backlog of `Evicted` pods, which
  points at node disk or memory pressure rather than this bug.
- The Grid sidecar on Hetzner was showing a new Chrome session roughly every
  minute even while the API container was dead, so something else in-cluster
  was driving `:4444` directly. Worth identifying.
- Every other source-mounted service in `dd-next-runtime` shares this exposure.
  The hostPath is an EC2 assumption baked into manifests that two clusters
  sync; selenium is fixed, but it was not special.

## Regression coverage

`voxletra-e2e` suite `110-remote-runners` now checks both clusters on every
run: workload readiness on each, and on AWS a live Grid scenario plus the
unauthenticated-`/run` refusal. It went red on the Hetzner crashloop and is
green now that selenium is fixed — which is the point. The outage lasted 23
days precisely because nothing was asserting on it: a crashlooping pod keeps
its Service and its ClusterIP, so the failure is invisible from the outside.

```sh
cd voxletra-e2e && bash run-all.sh 110
```
