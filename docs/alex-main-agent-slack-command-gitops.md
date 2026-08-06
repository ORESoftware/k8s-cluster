# alex-main-agent Slack command GitOps runbook

Linear: `DEN-1298`  
Slack app: `alex-main-agent`  
App ID: `A0BMBAMM5NJ`  
Workspace ID: `T01B3C83PMK`  
Runtime source: `ORESoftware/ai-agent-bridge.rs@0ba7d2d4eb8c44583503d4567af2de1f73bc5598`

## Purpose

This runbook activates the reviewed `/ores-*`, `/x-*`, and `/my-*` commands without treating the Slack app settings page, a laptop-running `slack run`, or a pasted credential as production infrastructure.

The GitOps bundle is deliberately staged in signed dry-run mode. It accepts only Slack-signed requests from the expected app and workspace, resolves the request through the thirteen reviewed channel bindings, reads bounded channel context, and posts a metadata-only dry-run result. It does not create bridge workflows, coordinator jobs, Linear records, GitHub branches, or pull requests until a separate reviewed change sets `SLACK_COMMAND_DRY_RUN=false`.

## Reviewed command surface

The installed app manifest must contain exactly these six command names:

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

The Kubernetes Ingress exposes those three paths as exact matches. It does not publish `/healthz`, `/readyz`, a generic `/slack` prefix, or the bridge/coordinator APIs.

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
```

The first public edge is the ingress-nginx deployment used by the `api.fiducia.cloud` Ingress. The AWS cluster intentionally does not claim `ingressClassName: nginx`; its direct gateway remains unaffected. One healthy public command endpoint is sufficient for Slack, so do not expose a second active endpoint until failover ownership and duplicate-delivery behavior are reviewed.

The deployment builds only `fiducia-slack-command` from the immutable reviewed source commit. It verifies the fetched Git revision before compiling or starting. The source registry at that revision contains thirteen unique, single-repository bindings and authorizes only the pilot operator.

## Secret prerequisites

The manifest commits references only. It must never contain token values.

Provision or verify this approved external object:

```text
dd/remote-dev/alex-main-agent-slack
```

Required properties:

```text
SLACK_BOT_TOKEN
SLACK_SIGNING_SECRET
```

The ExternalSecret also projects two existing service-scoped credentials:

```text
dd/remote-dev/ai-agent-bridge-secrets.inbox_token
  -> SLACK_BRIDGE_BEARER

dd/remote-dev/ai-agent-coordinator-secrets.COORDINATOR_API_TOKEN
  -> SLACK_COORDINATOR_BEARER
```

The resulting Kubernetes Secret is `default/dd-slack-command-secrets`. A missing property must prevent the pod from starting. Do not mark any secret reference optional, copy values into Argo parameters, or use an app configuration token as a runtime credential.

An app configuration token is needed only for the short-lived administrative operation that reconciles the remote Slack manifest. It does not belong in AWS Secrets Manager, Kubernetes, GitHub Actions, Linear, or runtime environment variables.

## Remote Slack app reconciliation

Repository configuration does not update the installed Slack app automatically.

1. Open app `A0BMBAMM5NJ` in the Slack app administration UI.
2. Export or copy the current remote manifest.
3. Reconcile the complete remote manifest with `ORESoftware/ai-agent-bridge.rs/slack-app/manifest.yaml` at the pinned source revision.
4. Validate and save the merged manifest.
5. Reinstall the app to workspace `T01B3C83PMK` after command, feature, or scope changes.
6. Refresh the Slack client and type `/ores-` in the `#oresoftware` channel composer.
7. Confirm all six names appear in autocomplete before testing dispatch.

The app requires `commands`, `chat:write`, `channels:history`, and `groups:history`. Add `usergroups:read` only when a reviewed channel binding actually authorizes a Slack user group.

## GitOps rollout

The deployment target is the `k8s-cluster` `dev` branch because the remote Argo applications sync that branch.

Before merge, require the `alex-main-agent Slack command GitOps` workflow to prove:

- the workflow and action dependencies are immutable;
- the source repository checks out at exactly `0ba7d2d4eb8c44583503d4567af2de1f73bc5598`;
- the command binary still exists and compiles;
- the source app manifest has six commands and the expected URLs;
- the source routing registry has exactly thirteen unique bindings;
- all four runtime credentials are required secret references;
- the deployment enforces app ID, workspace ID, bounded context, concurrency, and dry-run;
- the Ingress has only three exact paths;
- the NetworkPolicy permits only ingress-nginx, DNS, the bridge, the coordinator, telemetry, and public TLS;
- `kubectl kustomize remote/argocd/dd-next-runtime` renders the complete bundle;
- no credential-shaped material appears in manifests, tests, docs, rendered evidence, or audit output.

After merge:

1. Confirm the Argo application that owns `remote/argocd/dd-next-runtime` is Healthy and Synced to the merge commit.
2. Confirm `ExternalSecret/dd-slack-command-secrets` reports Ready.
3. Confirm `PersistentVolumeClaim/dd-slack-command-state` is Bound.
4. Confirm `Deployment/dd-slack-command` is Available.
5. Confirm `/healthz` and `/readyz` succeed from inside the cluster.
6. Confirm the public Ingress routes exist without exposing the health routes.
7. Inspect logs only for metadata. Request bodies, command text, Slack tokens, signatures, and channel history must not be emitted.

A cold start compiles the pinned Rust binary and can take several minutes. The startup probe allows up to twenty minutes; a prolonged build failure is not a reason to disable signature validation or switch to a mutable source revision.

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

- the command is visible in autocomplete;
- Slack receives an acknowledgement before its three-second deadline;
- the no-text invocation opens the task modal;
- the request is accepted only for app `A0BMBAMM5NJ`, workspace `T01B3C83PMK`, the reviewed channel, and pilot user;
- the status identifies the correct repository and Linear project;
- the context count is bounded to five by default;
- the run journal prevents a retried delivery from creating a second accepted run;
- the response explicitly says no coordinator, bridge, Linear, or GitHub write occurred;
- logs and evidence contain no command body, captured message text, token, or signature.

Negative canaries must also be recorded:

- invalid signature;
- stale timestamp;
- wrong app ID;
- wrong workspace;
- unmapped channel;
- unauthorized user;
- repository escape attempt;
- command/provider mismatch;
- generic or unknown Slack path.

## Live promotion gate

Live dispatch requires a separate pull request. Do not combine it with initial deployment.

That change may set `SLACK_COMMAND_DRY_RUN=false` only after evidence confirms:

- the remote Slack manifest is reconciled and the app is reinstalled;
- the ExternalSecret and all four properties are Ready;
- the bridge accepts the scoped bearer and can create one single-agent workflow;
- the coordinator accepts the scoped bearer and supports idempotent `slack_agent_run` jobs;
- the AI Agent Run Queue projection is enabled;
- repository writes remain feature-branch and draft-PR only;
- provider, runtime, retry, token, concurrency, and spend ceilings are enforced;
- duplicate Slack delivery does not duplicate a Linear issue, workflow, job, branch, or pull request;
- accepted, running, blocked, PR-opened, CI, review, and merge projections return to the originating Slack thread.

## Rollback

Use the narrowest rollback that stops unsafe work while preserving evidence:

1. Set `SLACK_COMMAND_DRY_RUN=true` in Git and sync.
2. If acknowledgement itself is unsafe, scale `Deployment/dd-slack-command` to zero through a reviewed GitOps revert.
3. Disable the six Slack commands or clear their request URLs in the app configuration.
4. Revoke or rotate the bot token and signing secret after suspected exposure.
5. Rotate the bridge or coordinator bearer only with their callers.
6. Preserve the state PVC and metadata-only run IDs for idempotency analysis.
7. Revert to the last reviewed source revision; never replace the pinned revision with `main`, `dev`, or `latest`.

## Symptom map

| Symptom | Failing layer |
|---|---|
| Commands absent from autocomplete | Remote Slack manifest, missing `commands` scope, reinstall, or client refresh |
| `dispatch_failed` or timeout | DNS, TLS, Ingress, Service, pod readiness, or acknowledgement deadline |
| No-text command does not open modal | Interactivity URL, bot token, `views.open`, or expired trigger ID |
| `403`/unauthorized response | App, workspace, channel, user, repository, or write policy mismatch |
| Pod never starts | Missing ExternalSecret property, PVC, source fetch, compile, or security-context failure |
| Dry-run posts but no work begins | Correct while `SLACK_COMMAND_DRY_RUN=true` |
| Live mode creates workflow but no coordinator job | Coordinator URL, bearer, task contract, or idempotency failure |
| Duplicate work | State PVC, deterministic run ID, coordinator idempotency, or callback reconciliation regression |

## Evidence record

Attach only non-secret evidence to `DEN-1298`:

- Git commit and pull request;
- GitHub Actions run and artifact name;
- Argo application revision and health;
- ExternalSecret Ready condition without values;
- Deployment/PVC/Service/Ingress status;
- redacted `/readyz` response;
- Slack command name, run ID, status code, and timestamps;
- negative-canary outcomes;
- rollback test result.

Never paste a GitHub token, Slack token, signing secret, bearer, raw request body, or captured channel messages into the issue or runbook.
