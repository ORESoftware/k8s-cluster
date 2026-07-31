# Formal review procedure: deterministic soccer simulation and control

This document defines when formal evidence is required, which obligations a
change can affect, and what a pull request must record. It is additive: existing
model checkers, proof harnesses, property tests, fuzzers, and implementation
tests remain authoritative at their respective boundaries.

## Boundary

The procedure covers match phase, time, possession, scoring, restarts, and deterministic replay; it intentionally abstracts continuous geometry, neural-network internals, and optimizer numerics.

The executable `formal/review-procedure/model.py` exhaustively explores a deliberately finite abstraction. It is an independent sentinel, not a full proof of the production implementation.

The machine-readable source of truth is
`formal/review-procedure/obligations.json`; CI validates its schema and runs the
bounded sentinel where this PR supplies one.

## Obligations

1. **SIM_PHASE (Safety).** Only legal match-phase and restart transitions are reachable.
2. **SIM_TIME (Safety).** Simulation time is monotonic and never exceeds the bounded horizon.
3. **SIM_POSSESSION (Safety).** Possession is singular: no state represents two simultaneous ball owners.
4. **SIM_SCORE (Safety).** Scores are non-negative and increase only through a goal transition.
5. **SIM_REPLAY (Refinement).** A fixed seed and initial state produce the same abstract trace and result.

Safety and liveness are reviewed separately. A liveness claim must name its
fairness, delivery, resource, and eventual-synchrony assumptions instead of
presenting progress as unconditional.

## When to update formal evidence

Update this procedure, the obligation register, and the strongest applicable
model when a PR changes any registered trigger path in a way that can alter:

- state variables, guards, ordering, retries, expiry, cancellation, or recovery;
- deterministic normalization or serialization;
- identity, ownership, threshold, quorum, or provenance decisions;
- persistence/snapshot fields that carry safety-relevant history; or
- an implementation function named by an existing refinement test.

A refactor may state “no abstract transition change” only when the PR explains
why and names deterministic tests that demonstrate observational equivalence.

## Required change sequence

1. **State the semantic delta.** Write the old and new transition, affected
   state, guard, and postcondition before implementation review.
2. **Select obligations.** List every obligation ID affected. Do not use a broad
   “formal methods passed” statement in place of specific claims.
3. **Update the model/register.** Add the smallest transition or obligation that
   captures the behavior. Bounds may not be weakened merely to remove a
   counterexample.
4. **Add production refinement tests.** Reproduce the abstract transition using
   real production code, deterministic scheduling/time, and explicit failure
   injection where applicable.
5. **Run and record evidence.** Include commands, results, bounds, assumptions,
   and any intentionally unproved surface in the PR.

## Baseline commands

```sh
python3 formal/review-procedure/check.py
python3 formal/review-procedure/model.py
```

Repository-specific refinement evidence includes:

- `cargo test --lib` deterministic seed and transition tests
- scenario tests for goal/restart/half/full-time boundaries
- model-generated traces replayed against the narrowest Rust state-machine layer

## PR evidence block

```text
Formal surface:
Affected obligation IDs:
Old → new transition:
State/guard/postcondition:
Model or proof artifact:
Finite bound and assumptions:
Production refinement tests:
Commands and results:
Counterexample trace (when fixed):
Known unproved surface:
```

## Reviewer stop conditions

Block approval when an obligation is affected but absent from the evidence
block; a timeout/transport loss is treated as proof of failure; a bound is
weakened without justification; model and implementation tests disagree; a
state migration drops safety-relevant history; or a deterministic claim is
supported only by a probabilistic run.
