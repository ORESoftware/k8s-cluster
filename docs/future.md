# Future Work

## Neural-Guided MCTS

The live engine can use MCTS, but only in a bounded action-selection role. Full
match-state MCTS is too expensive for realtime play because each branch would
clone and advance 22 players, ball physics, pass/shot state, officials, and MPC
execution checks. The viable path is the one now started in the neural blend:
search over a tiny set of already-legal MDP/POMDP action candidates, guided by
tabular Q, actor priors, retrieval priors, and the neural critic.

Near-term ideas:

- Add telemetry for MCTS candidate count, simulations, selected action, prior
  source, and whether MCTS changed the top neural-blend action.
- Add a runtime/env override for the live MCTS budget, with hard caps preserved
  in `SoccerNeuralBlendConfig`.
- Feed MCTS trace data into the inspector snapshot so a live debugging tool can
  answer why the planner preferred a pass, dribble, shot, reset, or tackle.
- Measure the per-player decision timing delta with MCTS on/off during a live
  match and lock an acceptable budget in tests or runbook checks.

## Learned World-Model Lookahead

The existing world model predicts next neural-feature state from `(s, a)` but is
currently shallow. Future MCTS work should deepen lookahead only when the world
model is demonstrably warm and stable.

Good next steps:

- Gate depth `2+` on world-model training steps and finite recent loss, similar
  to how the value blend readiness keeps cold critics out of live action.
- Score model rollout leaves by re-applying legal action-family channels before
  critic evaluation, so predicted next states are less action-null and less
  off-distribution.
- Add a diagnostic comparing one-step model value against realized next-tick
  reward/value for the same player trajectory.
- Keep model rollouts feature-space only. Do not run full match ticks inside the
  MCTS planner.

## POMDP Belief Sampling

The opponent-press belief and perception-confidence features can become a small
belief ensemble for planning. This should stay cheap and deterministic in live
play.

Possible path:

- Sample two or three belief variants for likely opponent press/drop behavior,
  then average the critic/model value for each root action.
- Use Bayesian opponent-press belief only as a scoring feature or prior, not as a
  hidden mutable world branch in realtime.
- Add tests where the same pass action is downgraded against a high-press belief
  and preserved against a passive-marker belief.

## Action And Target Search

Current MCTS searches action labels. Later work can add target-level branches for
the few action families where target choice dominates outcome.

Candidate expansions:

- Pass-like actions: branch over top visible target players and target points.
- Dribble-like actions: branch over left, center, right, protect, and technical
  move variants when they are legal.
- Shooting actions: branch over target lane/height buckets only if the shot QP
  and goalkeeper model already produce finite estimates.
- Reset actions: explicitly compare short backward reset, lateral recycle, and
  long retreat ball so short 3-5 yard resets can win without hard-banning longer
  recycling.

## Training Uses

MCTS may be more valuable during learning than during live play. Training can use
larger budgets off the frame loop and distill the result back into the actor,
critic, retrieval corpus, or Q table.

Ideas:

- Run high-budget MCTS offline on saved replay moments, then store improved
  policy targets for actor training.
- Use MCTS visit counts as soft action labels rather than only the selected
  action.
- Store high-value searched moments in the retrieval corpus with metadata showing
  which priors and critic values drove the decision.
- Compare learned policy improvement from plain self-play versus MCTS-augmented
  self-play over the same seeds and wall-clock budget.

## Learned Role-Band Average-Y Models

The back four and midfield four should not be fixed line-height heuristics. Each
role band's average `y` position is a dynamic state variable that the value head,
actor, and learned world model can learn from live match outcomes. The current
heuristic remains a warm-start prior, but the neural nets should decide when
holding, stepping, dropping, or temporarily breaking a band actually improves
goals, turnovers, offside traps, pressure, outlets, and clean possession regains.

State inputs now made explicit:

- Ball position, velocity, and acceleration relative to the average defender
  line and midfield band, with the whole-field motion block still carrying the
  full ball vector.
- Opponent player vectors through the existing 22-player motion block, plus
  opponent center-of-mass gap, velocity, and acceleration relative to each role
  band.
- Each role band's own average `y`, forward velocity, and forward acceleration.
- Goalkeeper depth relative to each average band.
- Possession state: own possession, opponent possession, or loose/unknown.
- Current offside-trap viability and whether the next three seconds project to a
  viable trap for the back four.
- Midfield current spread and whether the average midfield band projects back
  within target tolerance over five seconds.

Eventual consistency is intentional. A defender may step out, cover a runner,
press a carrier, or have a wingback overlap for roughly three seconds without
being treated as a failed line. The back four is still the only role group with a
strict line requirement, and that requirement only applies to offside-trap
viability. Midfielders get roughly five seconds to recover their average band and
do not need to form a strict line; their spread is a learning input, not a hard
failure gate. The model should learn the payoff of those exceptions, while
persistent drift is still visible through target-delta, relative-motion,
spread, and viability channels.

Near-term training path:

- Track feature importance and value gradients for the appended back-four line
  and midfield-four band tails so we can see whether the critic is using the
  aggregates or relying only on hand-coded clamps.
- Add offline labels from successful defensive phases: average line `y`,
  midfield-band `y`, conceded xG/goal outcomes, offside wins, line-break
  concessions, keeper interventions, pressure recoveries, outlet availability,
  and regained possession windows.
- Let world-model lookahead predict how the average line/band evolves over one
  to five seconds, but keep execution per-player: LP/IPM owns team shape and
  each player's MPC remains single-actor only.
- Promote any learned line-height controller only behind readiness gates for
  finite loss, sufficient samples, and live fallback to the current defensive
  assignment logic.

## MPC Boundary

MCTS must not become team MPC. Keep the architecture division intact:

- LP/IPM decides team shape and slot allocation.
- MDP/POMDP plus neural policy/value chooses discrete player action.
- MCTS may rerank a bounded set of those discrete action choices.
- Per-player MPC executes one actor's own trajectory toward the chosen target.
- Other players and the ball may enter MPC only as obstacles/references, never as
  co-optimized state.

Future work that wants multi-player coordinated search should first prove it is
not duplicating the formation LP and does not introduce joint-state blowup in the
live loop.

## Guardrails

- Default/base match configs should remain deterministic and inexpensive.
- Live budgets should remain tiny: candidate-capped, simulation-capped, and
  depth-capped.
- Cold or malformed neural/world-model outputs must fall back to the tabular
  policy without changing play.
- Any new planner should have focused tests for budget caps, legality filtering,
  deterministic tie-breaking, and no accidental MPC/team-MPC coupling.
