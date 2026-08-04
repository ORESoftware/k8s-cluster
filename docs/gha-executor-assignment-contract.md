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
4. Start exactly one background `POST /builds` so client disconnect cannot cancel the retained assignment outcome.
5. Pin status requests to the executor namespace returned by that submission.

No assignment is inserted when no executor is ready, so a later retry may safely perform a new readiness selection before any submission attempt.

## Duplicate and ambiguous outcomes

- Concurrent and sequential identical requests wait for or return the retained outcome; they do not send another upstream POST.
- A fixed 4xx rejection is retained and returned without selecting another provider.
- Transport failures, timeout, HTTP 429, HTTP 5xx, malformed/oversized HTTP 202 responses, and invalid accepted IDs are ambiguous. The assignment remains pinned to the selected executor and every identical retry returns the same bounded reconciliation error.
- Upstream response bodies, credentials, repository payloads, and immutable revisions are not included in conflict or ambiguity responses.

The in-process assignment map is deliberately bounded and not evicted. When full, the router rejects new requests before submission rather than forgetting an assignment and risking duplicate work.

## Activation limit

This contract is safe only with one router replica. Production scaling, restart-transparent retry, or cross-provider takeover requires one shared restart-durable Fiducia-fenced assignment plus durable build status and artifact identity. Until that work is reviewed, GitOps must keep execution disabled and replicas at zero or one during controlled smoke.
