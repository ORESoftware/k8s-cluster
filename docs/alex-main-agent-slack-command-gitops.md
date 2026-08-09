# alex-main-agent Slack command GitOps runbook

Linear ownership: `DEN-845`, `DEN-1041`, `DEN-1298`, `DEN-391`, `DEN-847`  
GitHub rollout ledger: `ORESoftware/k8s-cluster#1111`  
Slack app: `alex-main-agent`  
App ID: `A0BMBAMM5NJ`  
Workspace ID: `T01B3C83PMK`

## Current reviewed release

- Source: `ORESoftware/ai-agent-bridge.rs@c3e54e6cd0c6d56e3d2ed32902228d974e550a3f`
- Trusted container workflow: `31235992249`, attempt `1`
- Bridge: `ghcr.io/oresoftware/fiducia-ai-agent-bridge@sha256:6b7e447a9989fa127ad4b0b3edc51fcd37a6b94a96bcf61b42c22d2641bf0ea8`
- Slack command ingress: `ghcr.io/oresoftware/fiducia-slack-command@sha256:01f80fbd4d3ba5226b4abdb7f5e603538924edb48e79e72b0af43246624900cb`
- Provider runner: `ghcr.io/oresoftware/fiducia-ai-agent-runner@sha256:90a919fb28fb2bc2795a0a3735ab08993d245c3eaa2afcd5f42be9b1a4982702`
- Rollout merge revision on `dev`: `c5f868b4598433d7ec5b3b96a853466ec89a9b49`
- Bridge and Slack rollout: PR `#1120`
- Held-zero provider runner rollout: PR `#1119`

The Kubernetes workloads consume exact immutable image digests. They do not clone source, compile Rust, mount a source `hostPath`, consume a GitHub PAT, or retain Cargo/build caches at runtime.

The Slack command service remains deliberately fail-closed with:

```text
SLACK_COMMAND_DRY_RUN=true
```

The provider runner remains deliberately disabled with:

```text
replicas: 0
```

A merged manifest is not proof of a live deployment. Do not represent the bridge or Slack commands as usable until the live-evidence checklist in this runbook and `#1111` is satisfied.

## Purpose

This runbook activates the reviewed `/ores-*`, `/x-*`, and `/my-*` commands without treating a Slack settings page, a laptop-running `slack run`, a mutable image tag, or a pasted credential as production infrastructure.

The current GitOps bundle accepts only Slack-signed requests from the expected app and workspace, resolves requests through fourteen reviewed channel bindings, reads bounded channel context, and stores durable run-idempotency state. In dry-run mode it posts a metadata-only result and performs no coordinator, bridge, provider, Linear, GitHub branch, or pull-request write.

## Reviewed command surface

The installed app manifest contains exactly six command names:

```text
/ores-claude
/ores-chatgpt
/x-claude
/x-chatgpt
/my-claude
/my-chatgpt
```

The aliases share two canonical command endpoints:

```text
https://api.fiducia.cloud/slack/commands/ores-claude
https://api.fiducia.cloud/slack/commands/ores-chatgpt
```

Interactive modal submissions use:

```text
https://api.fiducia.cloud/slack/interactions
```

The Kubernetes Ingress exposes those three paths as exact matches. It does not intentionally publish the Slack service `/healthz` or `/readyz` routes, a generic `/slack` prefix, the AI-agent bridge API, the provider runner, or the separately built Slack Events API binary.

Unsigned POSTs to either command endpoint and the interaction endpoint must fail with HTTP `401`. A public probe may use that fail-closed behavior without possessing the Slack signing secret. It must not manufacture a valid signature or send a real command body.

## Runtime topology

```text
Slack
  -> api.fiducia.cloud / three exact signed routes
  -> ingress-nginx
  -> Service/dd-slack-command :8151
  -> Deployment/dd-slack-command
       -> Slack Web API over TLS
       -> Service/dd-ai-agent-bridge :8142
       -> Service/ai-agent-coordinator.ai-agent-coordinator :8080
       -> PVC/dd-slack-command-state

Deployment/dd-ai-agent-runner
  -> replicas: 0 until DEN-391 and DEN-847 authorize activation
  -> Service/dd-ai-agent-bridge :8142 when explicitly scaled
  -> exactly one approved provider over TLS during a bounded canary
```

The first public edge is the ingress-nginx deployment used by the `api.fiducia.cloud` Ingress. One healthy public command endpoint is sufficient for Slack. Do not expose a second active edge until failover ownership and duplicate-delivery behavior are reviewed.

Both runtime images are non-root, read-only-root, capability-dropped, seccomp-constrained images built from one reviewed source tree. The Slack command image includes the validated fourteen-binding channel registry at `/etc/alex-main-agent/alex-main-agent.channels.json`.

## Secret prerequisites

Manifests commit references only. They must never contain values, prefixes, hashes, lengths, encrypted copies, shell-expanded values, or values embedded in URLs.

Provision or verify this approved external object:

```text
dd/remote-dev/alex-main-agent-slack
```

Required properties:

```text
SLACK_BOT_TOKEN
SLACK_SIGNING_SECRET
```

The Slack ExternalSecret also projects two service-scoped credentials:

```text
dd/remote-dev/ai-agent-bridge-secrets.inbox_token
  -> SLACK_BRIDGE_BEARER

dd/remote-dev/ai-agent-coordinator-secrets.COORDINATOR_API_TOKEN
  -> SLACK_COORDINATOR_BEARER
```

The resulting Kubernetes Secret is `default/dd-slack-command-secrets`. A missing property must prevent the pod from starting. Do not mark a secret reference optional, copy values into Argo parameters, or use a Slack app configuration token as a runtime credential.

Provider activation is independently gated by `DEN-391`. The required `default/dd-ai-agent-runner-secrets` bundle must contain only the expected keys for one reviewed provider before any scale-up. The runner Deployment intentionally fails to start if it is scaled without that bundle.

## Remote Slack app reconciliation

Repository configuration does not update the installed Slack app automatically.

1. Open app `A0BMBAMM5NJ` in the Slack app administration UI.
2. Export or copy the current remote manifest.
3. Reconcile the complete remote manifest with `ORESoftware/ai-agent-bridge.rs/slack-app/manifest.yaml` at source `c3e54e6cd0c6d56e3d2ed32902228d974e550a3f`.
4. Validate and save the merged manifest.
5. Reinstall the app to workspace `T01B3C83PMK` after command, feature, or scope changes.
6. Refresh the Slack client and type `/ores-` in the `#oresoftware` channel composer.
7. Confirm all six names appear in autocomplete before testing dispatch.

The app requires `commands`, `chat:write`, `channels:history`, and `groups:history`. Add `usergroups:read` only when a reviewed channel binding actually authorizes a Slack user group.

A Slack app configuration token is needed only for the short-lived administrative manifest operation. It does not belong in AWS Secrets Manager, Kubernetes, GitHub Actions, Linear, runtime environment variables, or this runbook.

## GitOps release verification

The deployment target is the `k8s-cluster` `dev` branch because the remote Argo applications sync that branch.

Before selecting a new release, require one trusted post-merge source workflow to publish machine-readable digest artifacts for every selected binary. Verify that source SHA, workflow run and attempt, image name, build target, manifest digest, and complete image reference agree.

For the current release, focused checks proved:

- bridge, Slack command, and provider runner use exact `image@sha256:<digest>` references from one source workflow;
- the source gitlink equals the selected source commit;
- no runtime Git clone, compiler, Cargo build, PAT, source `hostPath`, or build cache remains;
- all action dependencies in focused workflows are immutable;
- the source app manifest has six commands and the expected URLs;
- the source registry has fourteen unique bounded bindings;
- all four Slack runtime credentials are required secret references;
- the provider secret bundle is required and the runner remains at zero replicas;
- app ID, workspace ID, bounded context, concurrency, durable state, and dry-run are enforced;
- the Ingress has only three exact Slack paths;
- NetworkPolicies limit ingress and egress to reviewed paths;
- `kubectl kustomize remote/argocd/dd-next-runtime` renders the complete bundle;
- Docker and ephemeral-kind bridge smoke pass;
- cluster E2E, OpenAPI, observability, catalog, static, secret-scan, and no-PAT contracts pass.

The broad private-backend repository job can stop before tests when owner-scoped GitHub App installation access to unrelated private gitlinks is unavailable. Do not substitute a PAT or weaken fail-closed checks. Record that infrastructure gate separately from the focused bridge evidence.

## Live cluster verification

After a rollout merge:

1. Confirm the Argo application that owns `remote/argocd/dd-next-runtime` is Healthy and Synced to the intended merge revision.
2. Confirm `ExternalSecret/dd-ai-agent-bridge-secrets` and `ExternalSecret/dd-slack-command-secrets` report Ready without printing values.
3. Confirm `PersistentVolumeClaim/dd-slack-command-state` is Bound.
4. Confirm `Deployment/dd-ai-agent-bridge` and `Deployment/dd-slack-command` are Available.
5. Confirm both pods report the exact selected image IDs, not only mutable tag names.
6. Confirm `Deployment/dd-ai-agent-runner` exists at exactly zero replicas.
7. Confirm bridge and Slack `/healthz` and `/readyz` from inside the cluster.
8. Confirm unauthenticated non-health bridge requests fail and the scoped bridge bearer succeeds.
9. Exercise non-sensitive bridge HTTP, SSE, and TCP fixtures against the live cluster.
10. Confirm the three public Slack routes have trusted TLS and reject unsigned POSTs with HTTP `401`.
11. Inspect logs only for metadata. Request bodies, command text, Slack tokens, signatures, channel history, provider prompts, and secret material must not be emitted.
12. Record the previous exact image IDs as the rollback baseline.

Attach value-safe evidence to `DEN-845` and GitHub issue `#1111`. Until this sequence completes, the correct status is “GitOps merged; live deployment not proven.”

## Signed dry-run canary

Keep:

```text
SLACK_COMMAND_DRY_RUN=true
```

Run these from the `#oresoftware` channel composer, not from inside a message thread:

```text
/ores-chatgpt investigate DEN-1298 and report the remaining activation gates
```

```text
/x-claude verify the alex-main-agent dry-run routing contract
```

```text
/my-chatgpt
```

Expected evidence:

- each command is visible in autocomplete;
- Slack receives an acknowledgement before its three-second deadline;
- a no-text invocation opens the task modal;
- the request is accepted only for app `A0BMBAMM5NJ`, workspace `T01B3C83PMK`, the reviewed channel, and an authorized user;
- the status identifies the correct repository and Linear project;
- context is bounded to five messages by default;
- the run journal prevents a retried delivery from creating a second accepted run;
- the response explicitly says no coordinator, bridge, provider, Linear, or GitHub write occurred;
- logs and evidence contain no command body, captured message text, token, signature, or provider credential.

Record negative canaries for:

- missing or invalid signature;
- stale timestamp;
- wrong app ID;
- wrong workspace;
- unmapped channel;
- unauthorized user;
- repository escape attempt;
- command/provider mismatch;
- generic or unknown Slack path.

## Provider and live-dispatch promotion gate

Live dispatch requires a separate reviewed change. Do not combine it with immutable deployment or secret provisioning.

`SLACK_COMMAND_DRY_RUN=false` and `dd-ai-agent-runner.spec.replicas: 1` are authorized only after:

- the remote Slack manifest is reconciled and the app is reinstalled;
- bridge and Slack ExternalSecrets are Ready;
- `DEN-391` proves the required runner secret bundle for exactly one approved, priced provider;
- the bridge and coordinator accept only their scoped bearers;
- bridge, runner, and provider metrics and alerts are in place;
- provider host allowlists, token/runtime/concurrency/retry ceilings, and cost budgets are enforced;
- one separate `DEN-847` PR scales the provider runner from zero to exactly one;
- duplicate Slack delivery cannot duplicate a Linear issue, workflow, job, provider call, branch, or pull request;
- accepted, running, blocked, PR-opened, CI, review, cancellation, failure, and merge projections return to the originating Slack thread;
- one bounded ChatGPT case and one bounded Claude case are explicitly authorized and evidenced;
- the runner is returned to zero after the canary and all claims/leases release or expire safely.

Repository writes must remain feature-branch and draft-PR only until separately reviewed promotion criteria are satisfied.

## Rollback

Use the narrowest digest-only rollback that stops unsafe work while preserving evidence:

1. Keep or restore `SLACK_COMMAND_DRY_RUN=true` in Git and sync.
2. Keep or restore `dd-ai-agent-runner.spec.replicas: 0`.
3. Change the bridge, Slack command, or runner manifest only to a previously recorded exact image digest from the same component.
4. If acknowledgement itself is unsafe, scale `Deployment/dd-slack-command` to zero through a reviewed GitOps change.
5. Disable the six Slack commands or clear their request URLs in the Slack app configuration when the public edge must be stopped immediately.
6. Revoke or rotate the bot token and signing secret after suspected exposure.
7. Rotate a bridge or coordinator bearer together with every authorized caller.
8. Preserve the state PVC and metadata-only run IDs for idempotency analysis.
9. Never restore in-pod compilation, source cloning, a PAT, source mounts, mutable tags, or an unverified branch revision.

## Symptom map

| Symptom | Failing layer |
|---|---|
| Commands absent from autocomplete | Remote Slack manifest, missing `commands` scope, reinstall, or client refresh |
| DNS/TLS failure | Public DNS, Cloudflare/edge routing, certificate, ingress address, or network path |
| Public unsigned POST does not return `401` | Wrong route, wrong workload, ingress drift, edge interception, or signature validation regression |
| `dispatch_failed` or Slack timeout | DNS, TLS, Ingress, Service, pod readiness, or acknowledgement deadline |
| No-text command does not open modal | Interactivity URL, bot token, `views.open`, or expired trigger ID |
| `403`/unauthorized response | App, workspace, channel, user, repository, or write-policy mismatch |
| Pod never starts | Missing ExternalSecret property, PVC, image pull, architecture, or security-context failure |
| Pod reports a different digest | Argo drift, stale rollout, mutable reference, or wrong application ownership |
| Dry-run posts but no work begins | Correct while `SLACK_COMMAND_DRY_RUN=true` |
| Runner has zero pods | Correct until DEN-391 and DEN-847 authorize a bounded canary |
| Live mode creates workflow but no coordinator job | Coordinator URL, bearer, task contract, or idempotency failure |
| Duplicate work | State PVC, deterministic run ID, coordinator idempotency, lease behavior, or callback reconciliation regression |

## Evidence record

Attach only non-secret evidence to `DEN-845`, `DEN-1041`, `DEN-1298`, `DEN-391`, `DEN-847`, and GitHub issue `#1111` as applicable:

- source commit, image workflow run/attempt, exact digest, Git commit, and pull request;
- Argo application revision and health;
- ExternalSecret Ready conditions without values;
- Deployment, PVC, Service, Ingress, and NetworkPolicy status;
- exact pod image IDs;
- redacted health/readiness and authenticated transport results;
- public DNS/TLS status and unsigned-route status codes;
- Slack command name, metadata-only run ID, status code, and timestamps;
- negative-canary outcomes;
- provider canary ID and value-safe usage/cost reconciliation;
- rollback digest and result.

Never paste a GitHub token, Linear token, Cloudflare token, R2 credential, Slack token, signing secret, bridge/coordinator bearer, provider key, raw request body, or captured channel message into GitHub, Linear, Slack, workflow artifacts, command arguments, or this runbook.
