# Browser E2E infrastructure status — 2026-07-25

Verified the shared browser-automation stack on both clusters by driving
`dd-browser-test-server`'s `POST /run` scenario API under all three drivers. AWS
is healthy. **Hetzner has been down for 23+ days** on a manifest portability
defect that no probe reports.

## Summary

| Cluster | Access | `dd-browser-test-server` | `dd-selenium-server` | Drivers verified |
| --- | --- | --- | --- | --- |
| AWS EC2 | ctx `dd-ec2-runtime` | 2/2 Running (50d) | 2/2 Running (50d) | Playwright 1.56.0, Puppeteer 24.43.1, Selenium 4.44.0 — all pass |
| Hetzner HA (5 nodes) | SSH `dd-k8s-*` | **0/2 CrashLoopBackOff** | **0/2 CrashLoopBackOff** | none — service never becomes ready |

## AWS: healthy

All three drivers navigate an in-cluster URL, extract text, and return a
screenshot. Requests routed through the Service reached a *different* pod than
the port-forward target, confirming both replicas serve.

```
playwright  ok=True  68ms
puppeteer   ok=True 122ms
selenium    ok=True 575ms   (Selenium Manager cold start included)
```

A 32-test suite covering all three drivers, the auth rejection paths, scenario
validation, and the `evaluate`-disabled posture passes end to end. See
`anticaptrad/act-e2e` → `tests/cluster/` and its
`docs/cluster-browser-e2e.md` for how to run it.

## Hetzner: root cause

`dd-browser-test-server.deployment.yaml` builds the service in-pod from a
**hostPath mount**:

```yaml
volumes:
  - name: repo
    hostPath:
      path: /home/ec2-user/codes/dd/dd-next-1
      type: Directory
```

and then:

```sh
cd /opt/dd-next-1/remote/deployments/browser-test-server
```

That path is an **AWS-node-specific layout** (`ec2-user`). The Hetzner nodes run
Ubuntu with the repo checked out elsewhere. On every Hetzner node:

```
EXISTS  /home/ec2-user                                  (5 entries)
EXISTS  /home/ec2-user/codes/dd/dd-next-1                (1 entry)
MISSING /home/ec2-user/codes/dd/dd-next-1/remote/deployments/browser-test-server
```

Because the top-level directory happens to exist, `hostPath type: Directory`
mounts **successfully** — the kubelet has nothing to reject. The failure surfaces
one layer later, in the container:

```
/bin/bash: line 5: cd: /opt/dd-next-1/remote/deployments/browser-test-server: No such file or directory
```

Exit code 1, restart, repeat. One pod has accumulated **6371 restarts over 23
days**.

This is the important part: an empty-but-present hostPath is indistinguishable
from a correct one at mount time, so the manifest is silently non-portable
across the two clusters even though ArgoCD syncs it to both.

### dd-selenium-server fails the same way — but only half of it

`dd-selenium-server` is one pod with two containers, and they fail
*independently*, which is worth stating plainly because the summary line
(`1/2 CrashLoopBackOff`) hides it:

| Container | Image | State on Hetzner |
| --- | --- | --- |
| `selenium` | `selenium/standalone-chromium` | **Ready**, 1 restart. The Grid is healthy and has served sessions (Chrome 131.0.6778.204). |
| `selenium-api` | `maven:…-temurin-17` | **6,645 restarts.** Same hostPath error: `cd: /opt/dd-next-1/remote/deployments/selenium-server: No such file or directory`. |

So the Selenium **Grid itself is fine on Hetzner** — it is the authenticated Java
API in front of it that cannot start. Because the Service exposes only `:8105`
(the API) and never the Grid, the capability is unavailable regardless:

```
$ kubectl get endpoints dd-selenium-server dd-browser-test-server -n default
NAME                     ENDPOINTS   AGE
dd-selenium-server                   43d
dd-browser-test-server               43d

# from inside the cluster
selenium-api :8105 -> UNREACHABLE (HTTP 000)
```

Zero endpoints for 43 days on both Services.

For contrast, the same API on AWS drives a real Grid session end to end —
`goto https://example.com` → `waitForSelector h1` → `extractText` →
`extractAttribute`, 770ms, with a 16 KB PNG screenshot.

## Secondary finding: evicted-pod accumulation

Browser workloads on Hetzner have **291 dead pod objects**:

| Status | Count |
| --- | --- |
| `Evicted` | 254 |
| `ContainerStatusUnknown` | 30 |
| `CrashLoopBackOff` | 7 |

The evictions were `ephemeral-storage` DiskPressure:

```
The node was low on resource: ephemeral-storage.
  Threshold quantity: 48335976730, available: 47166528Ki
Pod was rejected: The node had condition: [DiskPressure].
```

No node currently reports pressure (control plane at 74% disk), so the pressure
has passed — but the terminated pod objects persist in etcd and make
`kubectl get pods` unreadable for anyone triaging. They are a symptom of the
crashloop, not a separate cause: a pod that exits immediately and is recreated
forever generates garbage at a high rate.

## Suggested remediation

Not applied — these are mutations to live infrastructure and want an operator
decision.

1. **Make the source mount cluster-portable.** Options, in rough order of
   preference:
   - Bake the service into an image (removes the hostPath entirely, and is the
     only option that also fixes cold-start reproducibility).
   - Parameterise the hostPath per cluster via a kustomize overlay, so AWS and
     Hetzner each get a path that exists.
   - If the hostPath must stay, use `type: DirectoryOrCreate` **plus** a startup
     guard that fails loudly with the resolved path when the expected subtree is
     absent, instead of a bare `cd`. A clear "expected X, not found" beats
     `cd: No such file or directory` on restart 6371.
2. **Reap the dead pods** once the crashloop is fixed:
   `kubectl -n default delete pods --field-selector status.phase=Failed`
   (verify the selector against a `get` first).
3. **Close the probe gap.** The readiness probe only proves Fastify is listening;
   it never launches a browser, which is why a 23-day outage of the *browser*
   capability stayed invisible. `dd-anticaptrad-e2e-browser-suite` covers this —
   it is `suspend: true` today, and enabling it on both clusters would surface a
   recurrence within a day.

## Reproducing

```sh
# AWS
kubectl --context dd-ec2-runtime -n default get deploy dd-browser-test-server dd-selenium-server

# Hetzner
ssh dd-k8s-fsn1 'kubectl get pods -n default | grep -E "browser|selenium" | head'
ssh dd-k8s-fsn1 'kubectl logs -n default deploy/dd-browser-test-server --tail=5'
```
