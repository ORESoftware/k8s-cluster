# Fiducia JWT revocation deployment runbook

This runbook is the production gate for DEN-1119. It covers the two direct raw-JWT verifier boundaries: `fiducia-edge` and `fiducia-load-balance.rs`.

Passing source and hosted-CI tests does not prove the exact production candidate. DEN-1119 remains open until the immutable image, rendered GitOps revision, live propagation/fault evidence, credential rotation, rollback exercise, and independent review are attached.

## Immutable release identity

The authority must run exactly this reviewed release:

```text
source repository: fiducia-cloud/fiducia-auth.rs
source commit:     6984b584e5350c1a82a2e5d5ff0195e124aa4542
Docker target:     revocation-admin
image:             ghcr.io/fiducia-cloud/fiducia-revocation-admin
registry digest:   sha256:b9377ca8bc5f1298b7adf705563e7a80ab97727337a301578dea42b208102d6c
immutable ref:     ghcr.io/fiducia-cloud/fiducia-revocation-admin@sha256:b9377ca8bc5f1298b7adf705563e7a80ab97727337a301578dea42b208102d6c
release ledger:    fiducia-cloud/fiducia-auth.rs#38
```

The release ledger record was emitted by the `main` publishing workflow from the digest returned by `docker/build-push-action`. The manifest must use the exact `image@sha256` reference above. A mutable tag, commit tag without digest, locally resolved digest, or digest from another source revision is not an equivalent deployment identity.

## Merged dependency order

1. Verifier-local cache: `fiducia-auth.rs` `64709f5fc1da02db72835ff7645033370035611c`.
2. Reusable fail-closed gate: `fiducia-auth.rs` `3b5321e82fd74ae130120aa6ca28d74643357ca3`.
3. Deterministic two-verifier model: `fiducia-auth.rs` `06892d230f4e14184ea3dbf2d40aa313597398b3`.
4. Load-balancer verifier: `fiducia-load-balance.rs` `437a7901c4a333dff1e8f9930e011c053cd3cc94`.
5. Edge verifier: `fiducia-edge` `08e2d0d51b7e5a4def676d0b9749d06887f2050f`.
6. Immutable authority target and digest publisher: `fiducia-auth.rs` `6984b584e5350c1a82a2e5d5ff0195e124aa4542`.
7. This GitOps revision, after all hosted checks pass.

Do not begin the shared-package cleanup in DEN-1351 until this rollout and live evidence are complete.

## Capability and runtime credential boundaries

Provision two independently generated values in `dd/remote-dev/fiducia-revocation`:

* `FIDUCIA_REVOCATION_ADMIN_SECRET`;
* `FIDUCIA_REVOCATION_READER_SECRET`.

Both values must contain at least 32 non-whitespace bytes and must not be equal.

* The authority receives both credentials because it serves mutation and reader endpoints.
* The load balancer receives only `reader-secret` and may call only `/v1/revocations/check`.
* Administrative mutation requires `admin-secret`, `x-fiducia-actor`, and `idempotency-key`.
* The edge uses its separately reviewed reader-credential mechanism; it never receives the admin credential.
* General applications, CI, dashboards, support tooling, and browser artifacts must receive neither value.
* Secret values, raw JWTs, tenants, subjects, and JTIs must never enter logs, metrics labels, traces, screenshots, or evidence attachments.

## Registry pull boundary

Treat the GHCR package as private unless a separate reviewed change proves anonymous pull access for the exact digest.

Provision `dd/remote-dev/fiducia-revocation-ghcr-pull` in the cluster secret store with one property:

* `dockerconfigjson`: a valid Docker config JSON document for `ghcr.io`, using a machine-owned credential with package-read access only.

External Secrets materializes it as `fiducia-revocation-ghcr-pull` with type `kubernetes.io/dockerconfigjson`. The Pod references that Secret only through `imagePullSecrets`.

This credential is registry-only:

* never reuse `admin-secret`, `reader-secret`, `FIDUCIA_INTERNAL_SECRET`, the CI GitHub App key, a personal access token, or a human account credential;
* never expose it through container environment variables, mounts, logs, issue comments, Linear, test fixtures, or support artifacts;
* scope it to package reads required by this workload and document its machine owner and rotation procedure;
* if the package is deliberately made public, prove anonymous pull of the exact digest in hosted CI and remove both the ExternalSecret and `imagePullSecrets` in a separate reviewed change. Do not silently rely on public visibility.

## Hosted-CI preflight

Before merge:

1. Require the revocation-specific rendered contract test, secret scan, repository catalog, and broad repository checks.
2. Require current `dev`; do not merge a branch whose unrelated tests reflect stale repository expectations.
3. Configure `K8S_SUBMODULE_APP_ID` and `K8S_SUBMODULE_APP_PRIVATE_KEY` for the private-backend contract job. Missing GitHub App credentials are a repository configuration failure, not permission to skip or weaken the job.
4. Confirm every third-party action remains commit-pinned and no credential is embedded in a URL, manifest, workflow, or test fixture.
5. Confirm the exact source SHA and digest match the release-ledger record for target `revocation-admin`.
6. Confirm the registry ExternalSecret is registry-only, uses `dd-cluster-secrets`, produces `kubernetes.io/dockerconfigjson`, and is not referenced as a runtime Secret.

## Rendered-manifest preflight

Render `remote/argocd/fiducia` and verify:

* the authority Deployment and Service use port 8098;
* the image is the exact GHCR digest in this runbook;
* the release SHA and digest annotations match the image record;
* there is no Rust builder image, shell command, command override, args override, Git clone/fetch, Cargo invocation, compiler cache, source checkout, writable build workspace, hostPath, or build emptyDir;
* the Pod has no service-account token, runs as UID/GID 65532, uses RuntimeDefault seccomp, drops every capability, forbids privilege escalation, and has a read-only root filesystem;
* the Pod has exactly one `imagePullSecrets` entry named `fiducia-revocation-ghcr-pull`;
* the registry ExternalSecret uses `dd-cluster-secrets`, remote key `dd/remote-dev/fiducia-revocation-ghcr-pull`, property `dockerconfigjson`, target key `.dockerconfigjson`, and type `kubernetes.io/dockerconfigjson`;
* the registry Secret is not mounted or injected into the running container and does not reference the revocation admin, reader, or internal KV credentials;
* `admin-secret`, `reader-secret`, and the internal KV credential are required (`optional: false`);
* the load balancer references only `reader-secret` and cannot reference `admin-secret`;
* the authority NetworkPolicy permits only DNS and the load-balancer KV path on 8088 for egress;
* no authority egress rule contains `ipBlock`, `0.0.0.0/0`, public port 80, or public port 443;
* authority ingress and load-balancer egress agree on port 8098;
* both ExternalSecrets are cloud-backed and contain no literal value;
* rendered YAML contains no credential-shaped value.

## Argo CD rollout

1. Reconcile `fiducia-revocation-ghcr-pull` and verify the resulting Secret has type `kubernetes.io/dockerconfigjson` and the `.dockerconfigjson` key without printing, exporting, mounting, or base64-decoding its value.
2. Reconcile the `fiducia-revocation-secrets` ExternalSecret.
3. Verify the target runtime Secret contains `admin-secret` and `reader-secret` keys without printing, exporting, or base64-decoding their values.
4. Confirm the two runtime keys are non-empty and distinct using only length/equality exit status; do not log either value.
5. Sync the immutable authority Deployment.
6. Require a successful registry pull and require the image ID observed on the ready Pod to include `sha256:b9377ca8bc5f1298b7adf705563e7a80ab97727337a301578dea42b208102d6c`.
7. Require `/healthz` to become ready before rolling reader clients.
8. Roll `fiducia-load-balance` with the reader-only patch.
9. Deploy the edge reader configuration.
10. Record Argo application revision, Pod UID, node, image ID, rollout timestamps, and health status without recording protected identifiers.

## Reader-path smoke tests

Run from an allowed load-balancer test Pod without echoing the credential:
This runbook is the production gate for DEN-1119. It covers the two direct raw-JWT verifiers: `fiducia-edge` and `fiducia-load-balance`.

## Dependency and rollout order

1. Confirm the merged revocation authority/cache/gate/fault-model stack in `fiducia-auth.rs`:
   - cache foundation `64709f5fc1da02db72835ff7645033370035611c`;
   - reusable gate `3b5321e82fd74ae130120aa6ca28d74643357ca3`;
   - two-verifier model `06892d230f4e14184ea3dbf2d40aa313597398b3`.
2. Confirm the merged direct-verifier implementations:
   - load balancer `437a7901c4a333dff1e8f9930e011c053cd3cc94`;
   - edge Worker `08e2d0d51b7e5a4def676d0b9749d06887f2050f`.
3. Provision two cryptographically random credentials in `dd/remote-dev/fiducia-revocation`:
   - `FIDUCIA_REVOCATION_ADMIN_SECRET`
   - `FIDUCIA_REVOCATION_READER_SECRET`
4. Confirm both values contain at least 32 non-whitespace bytes and are not equal.
5. Sync the `fiducia-revocation-secrets` ExternalSecret and verify the target Secret exists without printing either value.
6. Deploy `fiducia-revocation-admin`; require `/healthz` to become ready.
7. Roll the load balancer with the reader URL and reader-only Secret reference from this GitOps change.
8. Deploy the edge Worker with its separate reader credential mechanism.
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
  - the Rust builder image is pinned by digest;
  - the interface and authority source checkouts are pinned to exact 40-character commits and verified with `rev-parse`;
  - the authority has no service-account token and runs non-root with a read-only root filesystem;
  - the authority ingress and load-balancer egress policies agree on port 8098.
- Verify the authority can reach the load balancer storage path on port 8088 and DNS, but cannot reach unrelated private workloads.
- Verify the load balancer cannot read `admin-secret` through its Pod specification or mounted environment.

## Reader-path smoke tests

Run these from an allowed load-balancer test pod without echoing secrets:

1. `GET /healthz` returns 200.
2. `POST /v1/revocations/check` without the reader header returns 401.
3. The same request with an incorrect reader value returns 401.
4. A correctly authenticated, well-formed non-revoked claim set returns a no-store decision with `revoked: false` and no matched target, generation, or expiry.
5. Unknown fields, inconsistent decisions, oversized bodies, invalid claims, timeouts, and non-success responses are rejected by the verifier and cannot produce an allow.
6. A reader credential cannot call `/revoke` or `/lift`.

## Two-verifier propagation exercise

Use a short-lived synthetic JWT with a unique tenant, subject, and JTI.

1. Confirm edge and load balancer accept it before revocation.
2. Revoke the exact token through the reviewed break-glass admin path with a unique actor and idempotency key.
3. Poll both verifiers until both deny. Record latency and generic outcome only; do not record the token, tenant, subject, JTI, or credential.
4. Repeat with a subject-wide revocation and a second token for that subject.
5. Confirm an equal JTI in a different tenant remains independent.
6. Lift the synthetic revocation with a new idempotency key and verify convergence only after the configured freshness windows.
7. Confirm duplicate mutation requests are idempotent and conflicting reuse of an idempotency key is rejected.

Acceptance: both verifiers deny within the declared freshness budget plus measured network/processing allowance, and neither serves a stale allow after the decision becomes stale.

## Authority-loss, malformed-response, and concurrency exercise

1. Populate a fresh allow decision, then deny verifier-to-authority traffic. It may be used only while fresh; after staleness requests must fail closed.
2. Populate a deny decision, then remove authority access. It remains denied and never converts to allow.
3. Start from a cold cache with authority unavailable. The verifier denies with a generic unavailable response.
4. Delay the authority beyond `FIDUCIA_REVOCATION_TIMEOUT_MILLIS`. The verifier times out and denies.
5. Return malformed JSON, oversized response, HTTP 500, or logically inconsistent decision. Every case denies.
6. Simulate local wall-clock regression below the verifier high-water mark. Cached allow state is not served.
7. Generate at least twelve concurrent cold requests for one opaque token key and verify exactly one authority refresh.
8. Restart each verifier independently during authority availability and outage; behavior must match the deterministic two-verifier model.
9. Partition one verifier from the authority while leaving the other connected and record bounded divergence and convergence.

## Runtime credential rotation

1. Generate separate new admin and reader credentials.
2. Update the backing store without logging values.
3. Trigger and observe ExternalSecret reconciliation rather than waiting for an uncontrolled interval.
4. Because the authority currently accepts one value per capability, use a controlled window:
   * pause administrative mutation;
   * roll the authority after admin rotation;
   * roll reader clients immediately after reader rotation;
   * verify old values are rejected;
   * resume mutation only after health and reader checks pass.
5. Record reconciliation and rollout timestamps, not values.

A future dual-current/next credential scheme may remove this interruption. Do not weaken fail-closed behavior to simulate overlap.

## Registry credential rotation

1. Create a replacement machine-owned package-read credential without changing runtime credentials.
2. Update only `dd/remote-dev/fiducia-revocation-ghcr-pull.dockerconfigjson` and trigger ExternalSecret reconciliation.
3. Verify the Secret resource version changes without reading or logging its value.
4. Force a controlled image pull of the already pinned digest on a new Pod and require success.
5. Revoke the old registry credential and repeat the pull check.
6. Record credential owner, scope, rotation time, Secret resource version, Pod/image ID, and generic success only.

## Rollback

* If the image cannot be pulled, restore or rotate the registry-only pull credential; do not add a broad PAT to the manifest or container environment.
* If the authority cannot become healthy, disable or roll back raw-JWT traffic at the affected verifier. Do not bypass revocation.
* Reverting this GitOps change is safe only after every raw-JWT verifier using this authority is disabled or reverted.
* Preserve the revocation ledger and historical encryption-key material. Rollback must not delete backing ExternalSecret data or discard active revocations.
* API-key introspection and trusted edge-hop paths are separate boundaries; verify their behavior explicitly.
* Capture rollback revision, timings, image-pull outcome, health, and generic denial behavior.

## Completion evidence

DEN-1119 may be completed only after attaching:

* green hosted CI for cache/gate/fault model, edge, load balancer, immutable image publisher, and this current-`dev` GitOps PR;
* the release-ledger record and exact rendered image digest;
* sanitized runtime and registry ExternalSecret, security-context, and NetworkPolicy contract output;
* successful pull of the exact digest through the dedicated registry-only Secret;
* exact-token and subject-wide two-verifier propagation timings;
* tenant/JTI isolation evidence;
* authority-loss, stale-state, timeout, malformed/oversized response, concurrency, clock-regression, restart, and partition results;
* runtime and registry credential-rotation plus rollback exercises;
* confirmation that logs, metrics, traces, screenshots, CI artifacts, and support evidence contain no protected identifiers or secrets;
* independent reviewer acceptance of the exact source SHA, image digest, GitOps commit, environment, assertions, timings, and limitations.
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

- green hosted CI for auth cache/gate/fault-model, edge, load balancer, and this GitOps PR;
- rendered secret/network-policy/image/source-pin contract results;
- two-verifier exact-token and subject-revocation propagation results;
- authority-loss, timeout, malformed-response, concurrency, and clock-regression results;
- a credential-rotation exercise;
- confirmation that logs, traces, metrics, and artifacts contain no protected identifiers or secrets.
