# Formal-methods procedure: deterministic match lifecycle

Akrion combines a discrete-event match engine, real-time sessions, MPC, POMDP/RL decisions, and replayable learning artifacts. Before proving tactical quality, the engine needs a small stable lifecycle contract: time and scores are monotonic, goals produce stoppages and opponent restarts, possession is singular, finished matches are immutable, and identical traces replay deterministically.

## Claim boundary

`formal/match_model.py` is a finite specification, not a proof of the production Rust engine. It exhausts the bounded abstract state space in `fm.toml` and establishes a versioned JSON-lines trace contract. Production refinement remains explicitly `planned` until a Rust adapter projects `SoccerMatch` or `SoccerRealtimeSession` into the same canonical state after each event.

The model excludes continuous physics, offside, fouls, substitutions, extra time, stochastic policy choice, floating-point solver behavior, and learning updates. Those belong in separate models and refinement layers.

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

## Change procedure

1. Keep lifecycle events separate from policy decisions; the trace records what happened, while POMDP/RL layers explain why.
2. Add a canonical Rust projection before claiming refinement. Use stable integers/enums, never pointers, wall-clock timestamps, hash-map order, or unrounded floats.
3. Seed stochastic components and record seed, model version, configuration digest, and actions with replay artifacts.
4. Update the model and Rust adapter together when adding halftime, extra time, shootout, abandonment, or review.
5. Run:

   ```bash
   python3 formal/match_model.py
   printf '%s\n' '{"op":"replay","events":["start","home_goal","restart","tick","finish"]}' \
     | python3 formal/match_model.py --json-stdin
   cargo check --all-targets
   ```

6. Preserve the smallest counterexample trace from every failure.
7. Do not hide production nondeterminism by sorting or rounding only in the test adapter.

## Planned production refinement

The next slice is a Rust JSON-lines/ITF adapter that constructs a deterministic match fixture, applies canonical lifecycle events, projects phase/tick/score/possession/restart state, compares it with this model, and runs fixed seeds across debug and release builds. Only then should this profile claim implementation refinement.

## Explicitly out of scope

This specification does not prove soccer-rule completeness, physical realism, floating-point equivalence, MPC/RL optimality, fairness, liveness, or training convergence.
