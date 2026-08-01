# Fiducia JWT revocation deployment runbook

This runbook is the production gate for DEN-1119. It covers the two direct raw-JWT verifiers: `fiducia-edge` and `fiducia-load-balance`.

## Dependency and rollout order

1. Merge and deploy the revocation storage/cache foundation from `fiducia-auth.rs#29`.
2. Merge the shared verifier contract and authority binary from `fiducia-auth.rs#32`.
3. Provision two cryptographically random credentials in `dd/remote-dev/fiducia-revocation`:
   - `FIDUCIA_REVOCATION_ADMIN_SECRET`
   - `FIDUCIA_REVOCATION_READER_SECRET`
4. Confirm both values contain at least 32 non-whitespace bytes and are not equal.
5. Sync the `fiducia-revocation-secrets` ExternalSecret and verify the target Secret exists without printing either value.
6. Deploy `fiducia-revocation-admin`; require `/healthz` to become ready.
7. Merge and deploy `fiducia-load-balance.rs#16` with the reader URL and reader-only Secret reference from this GitOps change.
8. Deploy `fiducia-edge#12` with its separate reader credential mechanism.
9. Run the two-verifier propagation and fault tests below before considering DEN-1119 complete.

Do not enable raw JWT authentication on a verifier that lacks its reader credential or cannot reach the authority. Both verifiers are designed to fail closed, so missing authority state causes a controlled denial rather than an implicit allow.

## Capability boundaries

- The load balancer receives only `reader-secret` and may call only `/v1/revocations/check`.
- Administrative mutation requires `admin-secret`, `x-fiducia-actor`, and `idempotency-key`.
- The authority NetworkPolicy accepts traffic only from the load balancer in the baseline deployment.
- Administrative operations must be performed from an explicitly reviewed, temporary break-glass workload or a future dedicated operator service. Do not add the admin credential to the load balancer, edge, general application workloads, or CI logs.
- Secret values, raw JWTs, tenant IDs, subjects, and token IDs must not appear in log fields, metric labels, traces, screenshots, or test artifacts.

## Preflight checks

- Render `remote/argocd/fiducia` and verify:
  - the authority Deployment and Service use port 8098;
  - both authority secret references are `optional: false`;
  - the load balancer references only `reader-secret`;
  - no literal credential material is present;
  - source checkouts are pinned to exact 40-character commits;
  - the authority has no service-account token and runs non-root with a read-only root filesystem;
  - the authority ingress and load-balancer egress policies agree on port 8098.
- Verify the authority can reach the load balancer storage path on port 8088 and DNS, but cannot reach unrelated private workloads.
- Verify the load balancer cannot read `admin-secret` through its Pod specification or mounted environment.

## Reader-path smoke tests

Run these from an allowed load-balancer test pod without echoing secrets:

1. `GET /healthz` returns 200.
2. `POST /v1/revocations/check` without the reader header returns 401.
3. The same request with an incorrect reader value returns 401.
4. A correctly authenticated, well-formed non-revoked claim set returns a no-store decision with `revoked: false` and no matched target/generation/expiry.
5. Unknown fields, inconsistent decisions, oversized bodies, invalid claims, and non-success responses are rejected by the verifier and do not produce an allow.
6. A reader credential cannot call `/revoke` or `/lift`.

## Two-verifier propagation test

Use a short-lived test JWT with a unique tenant, subject, and `jti`.

1. Confirm the JWT is accepted by edge and load balancer before revocation.
2. Revoke the exact token through the break-glass admin path with a unique actor and idempotency key.
3. Poll both verifiers until both deny. Record propagation latency without recording the token, tenant, subject, `jti`, or secrets.
4. Repeat with a subject-wide revocation and a second token for the same subject.
5. Confirm an equal `jti` in a different tenant remains independent.
6. Lift the test revocation with a new idempotency key and verify both verifiers converge after their freshness windows.
7. Confirm duplicate mutation requests are idempotent and conflicting reuse of an idempotency key is rejected.

Acceptance: both verifiers deny within the configured freshness budget plus network/processing allowance; no verifier serves a stale allow after its decision becomes stale.

## Authority-loss and clock-fault tests

1. Populate a fresh allow decision, then deny verifier-to-authority traffic. Requests may use the decision only while it is fresh; after staleness they must fail closed.
2. Populate a deny decision, then remove authority access. The request must remain denied; it must never convert to allow.
3. Start from a cold cache with the authority unavailable. The verifier must deny with a generic unavailable response.
4. Delay the authority beyond `FIDUCIA_REVOCATION_TIMEOUT_MILLIS`. The verifier must time out and deny.
5. Return malformed JSON, an oversized response, HTTP 500, or a logically inconsistent decision. Each case must deny.
6. Simulate local wall-clock regression below the verifier high-water mark. Cached allow state must not be served.
7. Generate twelve concurrent cold requests for one token and verify a single authority refresh occurs.

## Rotation

1. Generate new admin and reader credentials separately; never make them equal.
2. Because the current authority accepts one value for each capability, rotate during a controlled window:
   - update the backing store;
   - wait for ExternalSecret reconciliation;
   - roll the authority;
   - roll reader clients immediately after the reader value changes;
   - verify incorrect old values are rejected.
3. Keep mutation operations disabled during admin-secret rotation unless an emergency requires otherwise.
4. Capture reconciliation, rollout, and smoke-test timestamps without recording values.

A future enhancement should support overlapping current/next reader credentials to make rotation interruption-free. Do not weaken fail-closed behavior to emulate overlap.

## Rollback

- If the authority cannot become healthy, disable raw JWT traffic at the affected verifier or roll back the verifier release. Do not bypass the revocation check.
- Reverting this GitOps change removes the authority workload and reader wiring; it is safe only after raw JWT verification using this authority is disabled or reverted.
- API-key introspection and trusted edge-hop paths are separate; validate their behavior explicitly during rollback.
- Preserve the revocation ledger and historical encryption-key material. Rollback must not delete the backing ExternalSecret data or silently discard active revocations.

## Completion evidence

DEN-1119 may be closed only after attaching:

- green hosted CI for `fiducia-auth.rs#32`, `fiducia-edge#12`, `fiducia-load-balance.rs#16`, and this GitOps PR;
- rendered secret/network-policy contract results;
- two-verifier exact-token and subject-revocation propagation results;
- authority-loss, timeout, malformed-response, concurrency, and clock-regression results;
- a credential-rotation exercise;
- confirmation that logs, traces, metrics, and artifacts contain no protected identifiers or secrets.
