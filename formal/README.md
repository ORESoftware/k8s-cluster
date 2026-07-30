# Formal verification of machine release

`release_gate.qnt` is an executable specification of the repository's release
policy. It turns the catalog statement “`machineReady` remains false until every
release-blocking gate has retained evidence” into state transitions and
machine-checked invariants.

The finite model represents two plan revisions and these five evidence families:

1. source provenance;
2. machine envelope;
3. process readiness;
4. simulation and quality evidence;
5. human-or-automation handoff.

A preview records whether those gates are clear, but it never authorizes a real
machine run. Authorization is a separate, idempotent action bound to one job and
one immutable revision.

## Counterexample that changed the policy

The first exhaustive run found a 14-step counterexample:

1. validate revision 1;
2. clear all five gates;
3. preview and authorize revision 1;
4. reopen a gate;
5. clear it again;
6. preview and authorize revision 1 a second time.

Every individual gate check was satisfied, but the lifecycle allowed two logical
authorizations for one mutable revision. The corrected contract makes an
authorized revision immutable. A new blocker or changed evidence must advance the
plan revision, which atomically invalidates validation, evidence, preview, and
authorization before review starts again.

That rule is enforced both in the Quint action `reopen_gate` and the Rust
`ReleasePolicy::reopen_gate` method.

## Checked safety properties

The `release_safety` invariant checks:

- revisions, previews, and authorizations always name the current bounded
  revision;
- retained evidence is either absent or belongs to the current revision;
- each cleared gate has current evidence and each blocker has none;
- a safe preview matches the current validated evidence;
- machine readiness implies validation, all five evidence families, no blockers,
  and a current safe preview;
- at most one logical authorization is created per immutable revision.

Simulation also reaches stale-revision and foreign-job rejection, a blocked
authorization attempt, preview without authorization, safe release, duplicate
authorization retry, and release invalidation after revision.

## Run locally

The repository and CI pin Quint 0.32.0.

```sh
QUINT='npx --yes @informalsystems/quint@0.32.0'

$QUINT typecheck formal/release_gate.qnt

$QUINT run formal/release_gate.qnt \
  --max-samples=20000 \
  --max-steps=45 \
  --seed=0x31f899ea7b1d5878 \
  --invariant=release_safety \
  --witnesses \
    stale_evidence_rejection_reached \
    preview_without_authorization_reached \
    blocked_authorization_reached \
    safe_release_reached \
    duplicate_authorization_reached

# Deterministic replay of an authorized revision becoming invalid.
$QUINT run formal/release_gate.qnt \
  --init=init_authorized \
  --step=revision_step \
  --max-samples=1 \
  --max-steps=1 \
  --invariant=release_safety \
  --witnesses revision_invalidation_reached

$QUINT verify formal/release_gate.qnt \
  --backend=tlc \
  --invariant=release_safety
```

After correcting the counterexample, TLC explored 209,481 generated states and
2,680 distinct reachable states to a maximum depth of 22 with no invariant
violation.

## Rust trace replay

[`src/release_policy.rs`](../src/release_policy.rs) is the implementation-facing
policy kernel. Its tests replay the important abstract traces:

- a preview—blocked or clear—does not authorize execution;
- an authorized revision becomes not-ready after revision;
- evidence for the old revision or another job cannot clear a gate;
- duplicate authorization returns the same logical authorization;
- an authorized revision rejects in-place gate reopening.

The kernel is intentionally independent of HTTP, persistence, and machine
controllers. Callers can persist its evidence and authorization records without
reimplementing the state-transition rules.

## Deliberate limits

This is a finite protocol-level safety proof. It does not certify equipment,
operators, generated toolpaths, controller firmware, physical interlocks, sensor
accuracy, or evidence authenticity. It abstracts artifact bodies and signatures,
authorization identities, databases, timestamps, and multiple concurrent jobs.

Future refinements should add signed evidence provenance, explicit
operator-versus-automation authority, concurrent result ingestion, persistence
crash boundaries, and model-based replay through the release HTTP/storage path.
