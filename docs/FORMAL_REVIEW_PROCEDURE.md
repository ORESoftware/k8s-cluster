# Formal review procedure: refresh sessions, revocation, identity, and signing keys

This document defines when formal evidence is required, which obligations a
change can affect, and what a pull request must record. It is additive: existing
model checkers, proof harnesses, property tests, fuzzers, and implementation
tests remain authoritative at their respective boundaries.

## Boundary and canonical evidence

The procedure covers refresh-token issuance and atomic rotation, replay,
logout/revocation, access-token lifetime, signing-key overlap and retirement,
external-identity linking, and JWT claim validation.

The canonical bounded state model is the already-merged
`tests/auth_state_model.rs`. It explores refresh generations, revocation,
access-token expiry, key rotation, overlap, and retirement through the ordinary
Rust test target. This procedure deliberately reuses that stronger model instead
of maintaining a second, narrower Python state machine that could drift from it.

The concurrent PostgreSQL refresh regression in `tests/postgres.rs` remains the
implementation evidence for the transaction race: exactly one contender may
consume a refresh token. Cryptographic primitive correctness, PostgreSQL
isolation guarantees, and provider JWKS transport remain external assumptions
requiring integration tests and audits.

The machine-readable source of truth is
`formal/review-procedure/obligations.json`; CI validates its schema and then runs
the canonical bounded Rust model.

## Obligations

1. **AUTH_ROTATE (Safety).** A successful refresh consumes the presented token
   and installs one fresh current token.
2. **AUTH_REPLAY (Safety).** A consumed or non-current refresh token is rejected
   without changing session state.
3. **AUTH_REVOKE (Safety).** A revoked or expired session accepts neither refresh
   nor access tokens.
4. **AUTH_KEY_ROTATION (Safety).** Rotation creates an explicit overlap window;
   retiring an old key invalidates only tokens signed by that retired key, and
   the active key always remains trusted.
5. **AUTH_IDENTITY (Safety).** External identities are keyed by
   authority/tenant/subject, never email equality alone.
6. **AUTH_CLAIMS (Refinement).** Issued and accepted JWTs preserve issuer,
   audience, algorithm, expiry, not-before, key, and session-provenance
   constraints.

Safety and liveness are reviewed separately. A liveness claim must name its
fairness, delivery, resource, and eventual-synchrony assumptions instead of
presenting progress as unconditional.

## When to update formal evidence

Update this procedure, the obligation register, and the strongest applicable
model when a PR changes any registered trigger path in a way that can alter:

- state variables, guards, ordering, retries, expiry, cancellation, or recovery;
- deterministic normalization or serialization;
- identity, ownership, threshold, quorum, or provenance decisions;
- signing-key activation, overlap, retirement, or trust decisions;
- persistence/snapshot fields that carry safety-relevant history; or
- an implementation function named by an existing refinement test.

A refactor may state “no abstract transition change” only when the PR explains
why and names deterministic tests that demonstrate observational equivalence.

## Required change sequence

1. **State the semantic delta.** Write the old and new transition, affected
   state, guard, and postcondition before implementation review.
2. **Select obligations.** List every obligation ID affected. Do not use a broad
   “formal methods passed” statement in place of specific claims.
3. **Update the canonical model/register.** Extend
   `tests/auth_state_model.rs` when the abstract refresh, revocation, token
   lifetime, or key lifecycle changes. Do not fork those semantics into another
   model. Bounds may not be weakened merely to remove a counterexample.
4. **Add production refinement tests.** Reproduce the abstract transition using
   real production code, deterministic scheduling/time, and explicit failure
   injection where applicable. Transactional refresh changes require the
   PostgreSQL contender test or a stronger replacement.
5. **Run and record evidence.** Include commands, results, bounds, assumptions,
   and any intentionally unproved surface in the PR.

## Baseline commands

```sh
python3 formal/review-procedure/check.py
cargo test --test auth_state_model --locked
cargo test --all-targets --locked
```

Repository-specific evidence also includes transactional concurrent-refresh
tests against disposable PostgreSQL, provider-identity collision tests, and JWT
claim/JWKS boundary tests.

## PR evidence block

```text
Formal surface:
Affected obligation IDs:
Old → new transition:
State/guard/postcondition:
Canonical model or proof artifact:
Finite bound and assumptions:
Production refinement tests:
Commands and results:
Counterexample trace (when fixed):
Known unproved surface:
```

## Reviewer stop conditions

Block approval when an obligation is affected but absent from the evidence
block; a timeout or transport loss is treated as proof of failure; a bound is
weakened without justification; duplicate models encode the same state machine
without a declared refinement relation; model and implementation tests disagree;
a migration drops safety-relevant history; an active signing key can become
untrusted; or a deterministic claim is supported only by a probabilistic run.
