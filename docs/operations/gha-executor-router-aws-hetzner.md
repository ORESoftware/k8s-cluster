# GHA executor router — AWS/Hetzner activation and rollback

Linear: [DEN-1597](https://linear.app/denman/issue/DEN-1597/ci-continuity-add-fail-closed-awshetzner-independent-executor-routing)

This runbook activates the independent fixed-profile execution lane after the
native GitHub Actions Runner Controller lane has been certified. It does not
replace ARC, GitHub-hosted macOS/Windows runners, environment approvals,
release publication, or arbitrary GitHub Actions semantics.

## Preconditions

Do not scale `dd-gha-executor-router` above zero until all of the following are
recorded with immutable revisions:

1. `gha-executor-router-rs` formatting, tests, and Clippy are green.
2. GitOps and credential-boundary tests are green.
3. The router image is built from an exact commit, scanned, SBOMed, attested,
   published, and pinned by digest. The build-from-source deployment is only a
   bootstrap contract and is not the production image boundary.
4. AWS and Hetzner each expose a reachable `dd-build-server` endpoint that
   accepts only reviewed fixed profiles and trusted repositories.
5. Operator auth, AWS auth, and Hetzner auth are distinct values in
   `dd/remote-dev/gha-executor-router-secrets`.
6. The Hetzner placeholder URL in the ConfigMap has been replaced with the
   reviewed HTTPS endpoint or an explicitly reviewed private-network patch.
7. `gha-clone-server-rs` points to the router Service rather than directly to a
   provider build server.
8. No public-fork workflow, production deployment credential, host socket,
   Kubernetes service-account token, or general cloud credential can reach the
   lane.

## Credential reconciliation

The ExternalSecret reads three properties:

```text
operator_auth
aws_build_server_auth
hetzner_build_server_auth
```

Verify only readiness and metadata. Never print, copy into a command line,
commit, or compare the values in logs.

```bash
kubectl -n default get externalsecret dd-gha-executor-router-secrets
kubectl -n default get secret dd-gha-executor-router-secrets \
  -o jsonpath='{.metadata.name}{"\n"}'
```

The router reads each value from a separate mounted regular file. Inline secret
environment variables are not part of the contract.

## Plan-only startup

1. Replace the source bootstrap image with the reviewed digest-pinned image.
2. Keep `GHA_EXECUTOR_ROUTER_EXECUTION_ENABLED=false`.
3. Scale to one replica.
4. Verify `/healthz`, `/readyz`, `/capabilities`, and `/metrics` through an
   authenticated in-cluster probe.
5. Confirm the health response lists only executor ids and providers, never
   URLs or credentials.
6. Scale back to zero if readiness, NetworkPolicy, or secret-file validation
   fails.

Disabled mode must reject `POST /builds` and must not contact either provider.

## AWS-first acceptance smoke

After enabling execution on one replica:

1. Submit one deterministic fixed-profile request through
   `gha-clone-server-rs`.
2. Require a namespaced response id beginning with `aws~`.
3. Verify the AWS build-server audit contains exactly one matching `requestId`.
4. Verify the Hetzner build-server audit contains no matching submission.
5. Poll the namespaced id to completion and require all polls to remain on AWS.
6. Re-submit the exact same request and require the existing namespaced id with
   no additional provider request.

## Pre-acceptance Hetzner failover smoke

Use a reviewed reversible mechanism to make AWS fail before acceptance—for
example a temporary HTTP 503 response from the AWS test endpoint. Do not kill
or partition an executor after it has returned 202.

1. Submit a new deterministic request id.
2. Require one retryable AWS failure and one Hetzner submission.
3. Require a namespaced response id beginning with `hetzner~`.
4. Restore AWS immediately.
5. Poll only Hetzner to completion.
6. Verify metrics increment `fallback_attempts_total` once and do not increment
   contract rejections.

Repeat with a bounded 429 response. A 4xx contract rejection must stop without
contacting Hetzner.

## Post-acceptance provider-loss drill

This drill proves the non-duplication boundary.

1. Let AWS accept a unique deterministic request and record its `aws~...` id.
2. Temporarily make the AWS status endpoint unavailable.
3. Poll the namespaced id.
4. Require a pinned-executor polling failure.
5. Verify Hetzner received no submission and no poll for that request id.
6. Restore AWS and resume polling the same namespaced id.

The router must never manufacture cross-provider recovery after acceptance.
That requires a separately reviewed shared durable state and Fiducia-fenced
claim.

## Native ARC comparison

Run the same representative Rust, Node, Python, and browser-compatible commits
through:

1. GitHub-hosted Ubuntu;
2. AWS `sonus-ci` ARC;
3. Hetzner `sonus-ci` ARC;
4. the independent fixed-profile router.

Compare exit status, logs, artifact hashes, resource ceilings, and provenance.
Differences must be documented as explicit capability gaps, not silently
normalized.

## Hosted-minute policy

Do not infer exhaustion from a queued, approval-gated, or zero-job workflow.
The current repository has recently received hosted Ubuntu runners, proving
hosted allocation is presently usable. Exact current-month gross and net
Actions usage must come from the dedicated billing-read GitHub App endpoint
owned by DEN-1549:

```text
GET /organizations/{org}/settings/billing/usage/summary?year=YYYY&month=M&product=Actions
```

The capacity broker routes on gross Actions minutes and records net minutes
only as cost telemetry.

## Rollback

Rollback is intentionally mechanical:

1. Set `GHA_EXECUTOR_ROUTER_EXECUTION_ENABLED=false`.
2. Point `gha-clone-server-rs` back to the previously certified single build
   server or disable independent execution.
3. Scale `dd-gha-executor-router` to zero.
4. Leave route and provider audit records intact.
5. Keep native ARC and GitHub-hosted required checks unchanged unless their own
   rollback procedures require otherwise.
6. Rotate any credential whose confidentiality or authority is in doubt.

Never delete route evidence to make a retry possible, and never resubmit an
accepted job to another provider without the future shared-state gate.
