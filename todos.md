# Todos

Open hardening / follow-up items captured from agent sessions. Append-only;
remove an entry only when its concrete change is merged to `dev` and reconciled
by ArgoCD.

## Security hardening — bigger items not yet applied

Captured 2026-05-23 from the AWS-credential-leak audit. The narrow fixes (dev-server
`sanitizeEventText` AWS coverage, container-pool nerdctl scrubber, gitleaks CI
workflow + `.github/gitleaks.toml`, locked-in tests) shipped in the same session.
The items below each have meaningful blast radius and need an explicit go-ahead
before being applied.

### 1) `dd-build-server`: switch from static AWS keys in `dd-agent-secrets` to instance-role / IRSA credential chain

This is the only kube-secret-backed AWS key path in the repo and the readme
already calls for it. Concrete shape:

- Drop `AWS_ACCESS_KEY_ID` / `AWS_SECRET_ACCESS_KEY` / `AWS_SESSION_TOKEN` env
  entries from `remote/argocd/dd-next-runtime/dd-build-server.deployment.yaml`.
- Replace `aws_credentials_from_env()` in
  `remote/deployments/build-server-rs/src/main.rs:803-810` with the AWS SDK
  default credential chain (or an explicit ECR token request via the SDK). The
  deployment already runs on EC2 with a node instance role; on EKS, attach an
  IAM role to the `dd-build-server` ServiceAccount.
- Remove the corresponding three keys from the `dd/remote-dev/agent-secrets`
  AWS Secret.

### 2) Block IMDS for host-network warm workers

Today the warm Node.js/Claude worker containers run with `--network host` and
can hit `169.254.169.254` to grab the node's instance-role credentials.
Mitigations, in order of preference:

- On the EC2 instance metadata options, set `HttpPutResponseHopLimit=1` so
  containers running under a bridge can't fetch IMDS through a hop. Doesn't
  help host-network containers.
- Add an `iptables` / `nftables` rule on the EC2 host that drops outbound
  `dst 169.254.169.254` from any UID/PID outside the kubelet / EC2 metadata
  client. This is the only fix that catches host-network containers.
- Or move warm workers off host-network onto a CNI-managed bridge so the
  existing thread-pod-style NetworkPolicy applies.

### 3) Transcript-redaction sweep

Add a sweep that runs `sanitizeEventText` over `agent-transcripts/*/<id>.jsonl`
and `tmp/convos/thread.log` on a schedule (or before any export), so a pasted
secret doesn't sit unredacted in those files. Today only the live event stream
is sanitized; the persisted artifacts are not.

### 4) Local pre-commit hook mirroring the gitleaks rules

So devs catch keys before they hit CI. `.husky/pre-commit` calling
`gitleaks protect --staged --config .github/gitleaks.toml`.

## Decision pending

Want to do (1) the build-server credential-chain swap, (2) the IMDS netfilter,
(3) transcript sweep, and/or (4) the local pre-commit hook? Each is a separate,
contained change. (1) and (3) are the highest-value follow-ups given the leak
that triggered this audit.

## Reminder — credential revocation

The STS credentials pasted into chat on 2026-05-23 are still hot until the
revocation steps are run. The redaction work shipped in the same session only
stops future leaks; it does not revoke the access key id (literal value
omitted from this file: `<redacted-aws-access-key>` — secret-scan CI gate
would fail if the raw `ASIA…` shape were committed here). The issuing
principal must be rotated through CloudTrail lookup → IAM key rotation /
`AWSRevokeOlderSessions` policy, depending on whether the session was issued
from a long-lived IAM user key, an `AssumeRole` chain, or IAM Identity Center
SSO.

## GCS 40k/50k loadtest pipeline hardening — captured 2026-06-09

From the agent session that drove the GCS WSS 40k/50k campaign on `dd-remote-k8s-1`
(`i-0cc2461a55d491af6`, single-node r7i.4xlarge). The first ~5 dispatches failed
*before any load* on host/pipeline state, not on gcs. Each item below is a
separate, contained follow-up; items are append-only per the file convention.

### A) Host / cluster operational fragility (blocked the loadtest before load)

1. **Single-node host went `impaired` / SSM `ConnectionLost` twice**, driven by
   ~100% CPU (post-reboot rebuild herd of build-on-startup services + co-located
   ws-loadtest loaders). `aws ec2 reboot-instances` recovered it each time, but
   recovery is manual and racey (SSM is reachable ~7 min post-boot, then CPU can
   re-saturate). Fixes, in order of value: (a) keep loaders at `replicas=0`
   between runs — the loadtest `cleanup()` EXIT trap scales them down, but
   `restore_loadtest_apps` can re-raise them from Argo desired state, so confirm
   the Argo desired replica count for `dd-*-ws-loadtest-*` is 0; (b) put CPU
   limits on loader pods; (c) stop co-locating loaders on the gcs node (add a
   node / nodeSelector) so a loadtest can't starve the control plane + SSM agent;
   (d) raise gcs liveness/readiness `timeoutSeconds` so CPU spikes don't trigger
   pod restarts mid-load.

2. **Stale API endpoint after instance relaunch.** The `dd-ec2-runtime`
   kubeconfig (and gateway docs) pointed at a dead IP (`54.91.17.58`) while the
   relaunched instance had a fresh ephemeral IP (`3.91.81.98`); no EIP was
   associated to the box for the k8s API (the gateway EIP `98.90.186.114` is a
   separate concern). Fix: associate a stable EIP (or DNS/NLB) for the `:6443`
   API endpoint and update the kubeconfig so relaunches don't strand operators.

3. **Host checkout drift breaks the loadtest repo-sync** (cost 2 dispatches).
   The sync block runs as `ec2-user` and does
   `git merge --ff-only origin/dev` → on failure falls back to
   `git archive origin/dev <operator paths> | tar -x -C "$repo"`, then
   `git submodule update --init --recursive`. Three drift modes broke it:
   - **root-owned files** in the repo tree block the ec2-user tar overlay
     (`tar: … Operation not permitted` → exit 2). Root cause: a root-run process
     wrote into the source tree (host-cron `install.sh` runs under `sudo`).
     Self-heal (guarded, safe to add at the top of the sync block):
     `sudo -n chown -R ec2-user:ec2-user "$repo" 2>/dev/null || true`.
   - **stale moved-submodule registration** `remote/gcs/chat-vibe` (the old path;
     the submodule moved to `remote/deployments/gcs/chat-vibe`) fatals
     `git submodule update` (`No url found for submodule path … in .gitmodules`,
     exit 128). Self-heal:
     `git config --remove-section submodule.remote/gcs/chat-vibe 2>/dev/null || true`
     then `git update-index --force-remove remote/gcs/chat-vibe 2>/dev/null || true`.
   - **dirty tracked files** (e.g. `AGENTS.md`) keep `merge --ff-only` failing,
     forcing the tar fallback every run. Keep the host checkout reconciled to
     `origin/dev`.

### B) GCS capacity / performance for 40-50k

4. **chat.vibe deploy line is stale.** The k8s-cluster submodule tracks
   `master` (`6fc1a418`), but chat.vibe's default branch is `dev4`, and the
   gcs-router/server performance work lives on `dev4` /
   `feature/gcs-hot-path-perf` ("reuse transport / cap active upstreams",
   "Support max active connection cap in gcs-router"). The deployed `master`
   binary lacks it. Decision needed: realign the submodule to `dev4`, or merge
   the perf commits onto `master`. This is the likely real fix for item 5.

5. **~35s p50 / ~66s p99 message latency at 40k even with gcs at 8 CPU.** With
   the CPU bump (item 7) connections hold (`open=full, failed=0`, no restarts),
   but message fan-out backs up ~35s and loaders wobble around the 13335/loader
   sustain threshold — 40k-light is *marginal*, not a reliable pass (run #6: 2/3
   loaders full, rust 13 receive_errors; run #7: all 3 ~1-3% short). Root-cause
   with the CPU pprof the campaign collects (`loadtest_collect_pprof=true`) — gcs
   hot path and broker (rabbitmq/kafka) fan-out. Most likely fixed by item 4.

6. **gcs-router `--max-active-conns=60000` removed** from
   `gcs-router.deployment.yaml` (commit on `dev`) because the deployed master
   binary does not define the flag and crashlooped on it. Re-add the cap once a
   gcs-router build that defines it is deployed (arrives with item 4).

7. **gcs raised to 8 CPU / 12 Gi** (from 4 CPU / 8 Gi) on the 16-core node to
   fix connection-churn/probe-restart under 40k. If staying on `master`, 50k may
   need more headroom; revisit after item 4. (Requests are still 100m/512Mi —
   fine for the single-node scheduler.)

### C) Security (extends the 2026-05-23 items above)

8. **Account ROOT access keys were pasted into chat twice** this session to
   unblock cluster access. Rotate immediately (CloudTrail lookup → delete the
   root access keys); root keys should never be used for CLI/automation.

9. **New IAM user `my-cli-user` created with `AdministratorAccess`**, its access
   key pasted in chat and stored in `~/.aws/credentials` (`[default]`,
   `[dd-codex]`). Scope it to least privilege for this work — roughly
   `ec2:RebootInstances`/`ec2:Describe*`, `ssm:SendCommand`/
   `ssm:GetCommandInvocation`/`ssm:ListCommandInvocations`/
   `ssm:DescribeInstanceInformation`, `cloudwatch:GetMetricStatistics` — and
   rotate the pasted key.
