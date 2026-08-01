# Formal-methods procedure: deterministic match lifecycle

Akrion combines a discrete-event match engine, real-time sessions, MPC, POMDP/RL decisions, and replayable learning artifacts. Before proving tactical quality, the engine needs a small stable lifecycle contract: time and scores are monotonic, goals produce stoppages and opponent restarts, possession is singular, finished matches are immutable, and identical traces replay deterministically.

## Implemented refinement boundary

`formal/match_model.py` remains the finite specification. Production Rust now exposes the same lifecycle through `SoccerMatch`, `SoccerRealtimeSession`, `MatchAction`, and the enum-and-integer-only `MatchProjection` in `src/des/general/soccer/lifecycle.rs`. `src/bin/fm_match_adapter.rs` is a protocol-only JSON-lines process that constructs the formal fixture, applies canonical events to the production transition path, and emits the initial and post-event projections.

`formal/compare_match_replay.py` performs the bounded refinement check:

1. explore the model to the manifest depth while retaining one shortest witness for each reachable state;
2. exercise every outgoing action from every retained bounded state;
3. replay all resulting event sequences through the model and production Rust;
4. compare complete canonical traces, not only final states;
5. replay Rust twice in debug mode to expose nondeterminism;
6. compare debug and release results exactly.

The claim is therefore **bounded production refinement for this lifecycle projection**. It is stronger than a separate reference implementation, but it is not an unbounded proof and does not cover continuous simulation state.

## Abstract lifecycle

| Concept | Abstract values |
|---|---|
| phase | `pre_kickoff`, `in_play`, `stoppage`, `finished` |
| possession | `none`, `home`, `away` |
| scoring | bounded non-negative home/away counters |
| clock | bounded monotonic tick |
| restart | side entitled to put the ball back in play |

## Required invariants

1. Tick and scores never decrease.
2. Only a goal increments a score, and exactly one side increments by one.
3. A goal clears possession, enters stoppage, and gives the restart to the conceding side.
4. At most one side possesses the ball; stoppage and finished phases have none.
5. Finished is absorbing.
6. Identical canonical event sequences produce identical state traces.
7. Model, Rust debug, repeated Rust debug, and Rust release observations are identical within the declared bounds.

## Change procedure

1. Keep lifecycle events separate from policy decisions; the trace records what happened, while POMDP/RL layers explain why.
2. Keep `MatchProjection` stable and semantic. Never expose pointers, wall-clock timestamps, hash-map order, incidental IDs, or unrounded floats through the formal seam.
3. Update the model, production transition, adapter, and comparator together when adding halftime, extra time, shootout, abandonment, review, or persistence.
4. If stochastic components eventually feed a lifecycle action, record the seed, model version, configuration digest, and chosen action; do not put stochastic internals into this projection.
5. Run:

   ```bash
   python3 formal/match_model.py
   printf '%s\n' '{"op":"replay","events":["start","home_goal","restart","tick","finish"]}' \
     | cargo run --quiet --bin fm_match_adapter
   cargo test --lib des::general::soccer::lifecycle::tests
   python3 formal/compare_match_replay.py
   cargo check --all-targets
   ```

6. Preserve the smallest event sequence and both observations from every mismatch as a checked-in regression artifact.
7. Never make the adapter massage production output merely to match the model. Resolve the semantic disagreement in the model or production transition and document which contract changed.
8. Revisit the bounds whenever a new lifecycle state or action is added; a green check with unreachable new behavior is not acceptable evidence.

## Explicitly out of scope

This profile does not prove continuous physics, soccer-rule completeness, offside, fouls, substitutions, extra time, floating-point solver equivalence, MPC/RL optimality, fairness, persistence/restart recovery, liveness, or training convergence. Those need separate models and explicit composition arguments with this lifecycle envelope.
