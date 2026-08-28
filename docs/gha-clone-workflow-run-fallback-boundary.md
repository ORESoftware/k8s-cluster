# GHA clone failure-fallback event boundary

The independent continuity webhook is a **failure-only `workflow_run` fallback**. It is not a second execution path for ordinary `push`, `pull_request`, issue, release, or deployment events.

After validating the request HMAC, delivery UUID, JSON payload, and exact repository allowlist, `gha-clone-server-rs` handles every event other than `workflow_run` as a bounded no-op:

```text
HTTP 202
accepted=false
reason=only workflow_run events may trigger the failure fallback
```

That response occurs before workflow-source retrieval, planning, delivery-claim retention, or build-server submission. A signed non-`workflow_run` event therefore cannot consume an independent-run identity or execute a fixed profile.

A `workflow_run` event must still pass the existing fail-closed checks:

1. action is `completed`;
2. conclusion is one of the configured failure conclusions;
3. workflow name is outside the recursion-deny list;
4. repository and workflow path exactly match reviewed configuration;
5. `head_sha` is a full immutable 40-hex commit revision;
6. the fetched workflow is independently executable under reviewed fixed profiles; and
7. the delivery UUID has not already claimed a dispatch within the bounded retention window.

## Validation invariant

The regression suite starts the real HTTP process, signs representative `push` and `pull_request` payloads, and proves HTTP 202 with the stable no-op reason, zero GitHub workflow fetches, zero build-server submissions, and zero retained delivery claims. The same suite keeps the legitimate `workflow_run` failure path covered so narrowing the event boundary cannot silently disable the fallback.

Native GitHub Actions or official Actions Runner Controller remains responsible for normal event-triggered workflow semantics.
