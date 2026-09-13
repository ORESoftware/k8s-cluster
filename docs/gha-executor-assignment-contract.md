# GHA executor router request-assignment contract

Linear: DEN-1597

This contract narrows the independent AWS/Hetzner executor router to a single-replica, fail-closed placement boundary. It does not claim cross-replica or restart-durable ownership.

## Canonical request identity

Every accepted request is validated as the fixed `build-server.v1` / `run-profile` schema and normalized to:

- deterministic `requestId`;
- canonical `owner/repository` identity;
- immutable lowercase 40-hex revision;
- fixed profile slug.

The router binds the request ID to that complete immutable tuple. Reusing an ID with changed repository, revision, or profile returns HTTP 409 before an executor request is sent.

## Provider selection and submission

1. Probe reviewed executors in configured order.
2. Select the first ready executor before submission.
3. Insert one bounded in-process assignment containing the canonical request and selected executor.
4. Start exactly one supervised background `POST /builds` so client disconnect cannot cancel the retained assignment outcome and a submission-task panic becomes a retained ambiguous result.
5. Pin status requests to the executor namespace returned by that submission.

No assignment is inserted when no executor is ready, so a later retry may safely perform a new readiness selection before any submission attempt.

## Duplicate and ambiguous outcomes

- Concurrent and sequential identical requests wait for or return the retained outcome; they do not send another upstream POST.
- Explicit validation, authentication, method, conflict, payload-size, media-type, and schema 4xx rejections are retained and returned without selecting another provider.
- Transport failures, submission-task failure, HTTP 408, HTTP 425, HTTP 429, unclassified 4xx, HTTP 5xx, malformed/oversized HTTP 202 responses, and invalid accepted IDs are ambiguous. The assignment remains pinned to the selected executor and every identical retry returns the same bounded reconciliation error.
- Upstream response bodies, credentials, repository payloads, and immutable revisions are not included in conflict or ambiguity responses.

Each HTTP caller waits only for `GHA_EXECUTOR_ROUTER_ASSIGNMENT_WAIT_TIMEOUT_SECONDS`. If the retained assignment is still pending, the router returns HTTP 504 with `submissionAttempted=true`, `retryable=true`, and `automaticFailover=false`; the background submission continues. Retrying the same immutable request ID waits on the same assignment and never sends a second POST.

The in-process assignment map is deliberately bounded and not evicted. When full, the router rejects new requests before submission rather than forgetting an assignment and risking duplicate work.

The inert deployment declares the retention bound (`4096`) and pending wait (`65s`) explicitly. The wait exceeds the reviewed upstream timeout (`60s`) by default while remaining independently configurable for controlled smoke tests.

## Security-baseline dependency

The merge candidate is evaluated against `dev` after `f99c1118a432d55e76d5123240bc6dc8514f68a0`, which removed the redundant inline `GHA_EXECUTOR_ROUTER_SECRET_ROOT` value. The router continues to use the same fail-closed code default and the same mode-0400 projected credential directory; only the direct-child inbound-auth path remains explicit in GitOps.

This assignment change must not reintroduce an inline secret-root value, alter the Secret projection, or make the router deployable. Repository contracts, secret scans, rendering, and the continuity workflow run against the latest merge candidate rather than an earlier branch head.

## Verification boundary

Repository contracts read the split router implementation directly rather than relying on policy prose in the binary shim. They cover the service, assignment, authentication, and upstream modules plus both real-process suites.

The required checks include:

- Rust formatting and strict Clippy across every target;
- sequential and concurrent single-submission behavior;
- immutable-input conflict rejection;
- no-ready retry before submission;
- fixed-rejection and ambiguous-outcome provider pinning;
- bounded pending waits followed by same-assignment retry;
- explicit rejection classification for HTTP 408 and HTTP 425;
- submission-task panic conversion to a sanitized ambiguous outcome;
- duplicate inbound-auth rejection; and
- the static continuity contract that binds those behaviors to the split source paths.

A passing transformer or an earlier branch head is not release evidence. Merge evidence must name the exact product commit after all temporary write workflows and source-text anchors are absent.

## Activation limit

This contract is safe only with one router replica. Production scaling, restart-transparent retry, or cross-provider takeover requires one shared restart-durable Fiducia-fenced assignment plus durable build status and artifact identity. Until that work is reviewed, GitOps must keep execution disabled and replicas at zero or one during controlled smoke.
