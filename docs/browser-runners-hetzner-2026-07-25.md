# Browser automation runners: Hetzner is broken, AWS is healthy — 2026-07-25

Verification of `dd-browser-test-server` and `dd-selenium-server` on both
remote clusters, the root cause of the Hetzner failure, and the fix.

## State

| Cluster | Workload | State |
|---|---|---|
| AWS EC2 | `dd-browser-test-server` | **Running** 1/1 |
| AWS EC2 | `dd-selenium-server` | **Running** 2/2 |
| Hetzner | `dd-browser-test-server` | **CrashLoopBackOff** 0/1, ~6 400 restarts over 23d |
| Hetzner | `dd-selenium-server` | **CrashLoopBackOff** 1/2, ~6 650 restarts over 23d |

AWS was verified live, not just by pod status: a bounded scenario was driven
through the dedicated Selenium Grid against a cluster-internal URL and returned
`ok: true` in ~700–900 ms, and unauthenticated `POST /run` was correctly
refused with 401.

The three drivers on `dd-browser-test-server` (AWS) also each executed a real
in-cluster scenario: playwright 1.56.0 (~180 ms), puppeteer 24.43.1 (~160 ms),
selenium 4.44.0 (~775 ms). Selenium's extra latency is expected — it opens a
fresh `RemoteWebDriver` session against the Grid sidecar per run.

## Root cause on Hetzner

The `selenium` Grid sidecar is **healthy** and creating Chrome sessions
normally. The failing container is `selenium-api`, and its last log line is the
whole story:

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

## The fix

Give both browser deployments the same source-resolution shape
`dd-rust-vapi-phone` uses: prefer a shallow git clone, fall back to the mount.

```bash
source_root=/opt/dd-next-1
if [ -n "${SELENIUM_GIT_URL:-}" ]; then
  clone_root="$(mktemp -d /tmp/dd-selenium-source.XXXXXX)"
  if git clone --depth 1 --branch "${SELENIUM_GIT_REF:-dev}" "${SELENIUM_GIT_URL}" "$clone_root"; then
    source_root="$clone_root"
  else
    echo "[dd-selenium-server] source clone failed; using mounted source" >&2
  fi
fi
cd "$source_root/remote/deployments/selenium-server"
```

with `SELENIUM_GIT_URL` / `SELENIUM_GIT_REF` (and
`BROWSER_TEST_GIT_URL` / `BROWSER_TEST_GIT_REF`) set on the deployment. The
hostPath stays as the fallback, so EC2 behaviour is unchanged if the clone
fails.

Applied to:

- `remote/argocd/dd-next-runtime/dd-selenium-server.deployment.yaml`
- `remote/argocd/dd-next-runtime/dd-browser-test-server.deployment.yaml`

`kubectl kustomize remote/argocd/dd-next-runtime` builds clean (235 documents)
and both containers carry the new env vars.

**Not applied to any cluster.** Syncing this is a deploy decision; on Hetzner
it should replace a crashloop, and on AWS the clone path becomes primary, which
is worth watching on the first rollout.

## Worth checking separately

- Hetzner `dd-browser-job-runner` has a large backlog of `Evicted` pods, which
  points at node disk or memory pressure rather than this bug.
- The Grid sidecar on Hetzner shows a new Chrome session roughly every minute
  even with the API container dead, so something is still driving `:4444`
  in-cluster. Worth identifying — sessions are being created and dropped
  continuously.

## Regression coverage

`voxletra-e2e` suite `110-remote-runners` now checks both clusters on every
run: workload readiness on each, and on AWS a live Grid scenario plus the
unauthenticated-`/run` refusal. It reports the Hetzner crashloop as a failure
today, which is the point — the outage was invisible precisely because nothing
was asserting on it.

```sh
cd voxletra-e2e && bash run-all.sh 110
```
