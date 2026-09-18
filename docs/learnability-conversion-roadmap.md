# Learnability conversion roadmap

A grounded audit of hardcoded decision surfaces that can be converted to learning,
with the **exact live call sites** and a scoped conversion for each, modeled on
the back-four line model (`back_four_line.rs` + `docs/back-four-line-model.md`):
a feature struct → normalized vector, a `FeedForwardNetwork` head, a gated seam
that is byte-identical when off, a deterministic analytic seed, and a named
training signal.

Call sites verified against `main` @ `415d42b`.

Legend — **State**: `DORMANT` (net/seam built, not consumed live) · `HARDCODED`
(closed-form, no seam) · `PARTIAL` (gated seam landed, learning not live).

---

## Priority 1 — On-ball action policy (the actual decision)

**State: DORMANT (highest ROI).** This is *what to do* — pass/shoot/dribble/carry
and their parameters.

- **Live call site:** `WorldSnapshot::learned_action_for_player_with_context`
  feeds a `learned_plan` into `players[actor].run_time_step_with_context(...)`
  ([world.rs:5869](../src/des/general/soccer/world.rs#L5869)); the plan is produced
  by `exploration_action_for_player` → `neural_blended_action` →
  `best_action_for_state_observation_with_prior` → `mpc_reconciled_learned_plan_for_policy`
  ([world.rs:3135-3164](../src/des/general/soccer/world.rs#L3135)).
- **The gate:** `learned_policy_inference_enabled()` =
  `config.learning_enabled || team_policies.is_some() || learned_policy.is_some()`
  ([world.rs:1968](../src/des/general/soccer/world.rs#L1968)). **Off in a vanilla
  sim** — the heuristic action ordering runs unless a policy is loaded or learning
  is on.
- **What's still hardcoded:** the **action space** the policy ranks is the
  heuristic family set, and Pass/Shoot *parameters* are still a continuous
  power × ~72-site heuristic scan. `DiscretizedKickAction` /
  `lower_discretized_kick_release` are built and unit-tested but constructed
  **only in tests** ([soccer.rs:48097](../src/des/general/soccer.rs#L48097),
  [soccer.rs:52076](../src/des/general/soccer.rs#L52076)) — never in live candidate
  generation.

**Conversion (mostly wiring, low build):**
1. Construct `DiscretizedKickAction` in live candidate generation so kick
   power/direction/curve become a *discrete learned* choice, not a heuristic
   sweep — the policy's output space then matches the trainable action set.
2. Ensure `learned_policy_inference_enabled()` is true in the deployed
   learner/live-server (load the active gen — the live server already can, see
   [[soccer-live-server-pg-bridge-and-policy-bloat]]).
3. Strengthen `neural_blend` readiness so a trained head actually steers (today it
   blends toward heuristic at low readiness).
- **Training signal:** already present — actor-critic + GAE on `SoccerPolicyHead`,
  PPO-clip ([[soccer-real-ppo-multi-epoch]]); reward = existing local events +
  pitch-value×xT (Priority 2).
- **Parity:** gated by the existing inference flag; off = today's heuristic.

---

## Priority 2 — Learned xT / pitch-value surface (the reward everything trains on)

**State: HARDCODED seed.** Converting this lifts *every* positioning/▶action net
at once, because it is (or should be) their reward and critic basis.

- **Live call sites:** `expected_threat` / `team_expected_threat` /
  `territorial_advantage` ([pitch_value.rs](../src/des/general/soccer/pitch_value.rs))
  have **no live consumers** today — they feed only `pitch_value_reward_delta`,
  which is **gated off** (`DD_SOCCER_ENABLE_PITCH_VALUE_REWARD`). `pitch_control_home`
  is a closed-form time-to-arrive logistic ([pitch_value.rs:181](../src/des/general/soccer/pitch_value.rs#L181)).
- **What's hardcoded:** `expected_threat` is a closed-form forward^3 × shooting-zone
  ramp; the module doc itself calls it *"a seed the learners may later replace with
  a learned Markov xT grid."*

**Conversion:**
1. Learn a **Markov xT grid** from self-play possession chains (value of having the
   ball in each cell = P(score before turnover) via the empirical transition
   matrix) — replace `expected_threat` with a grid lookup, keeping the closed-form
   as the cold-start prior.
2. Optionally learn `pitch_control_home` from *who actually won the next loose
   ball* given the arrival-race features.
- **Training signal:** possession-chain outcomes (goal / shot / turnover) already
  flowing through the learner; no new instrumentation needed for the grid.
- **Parity:** swap is behind the existing pitch-value gate; the grid defaults to
  the analytic prior until enough chains accumulate.

---

## Priority 3 — Midfield line + press line depth (replicate the back four)

**State: HARDCODED, but the seam already exists.** Cheap, parity-safe wins —
literally the back-four pattern applied to the next two lines.

- **Live call sites:** the midfield line **already has a band function**,
  `midfield_line_band_adjusted_target` /
  `midfield_line_band_adjusted_intent`
  ([world.rs:29189](../src/des/general/soccer/world.rs#L29189),
  [world.rs:29294](../src/des/general/soccer/world.rs#L29294)) — the exact analogue
  of `defender_line_band_average_adjusted_y`. The press line is
  `defensive_midfielder_press_focus` ([world.rs:20925](../src/des/general/soccer/world.rs#L20925)),
  consumed in `defensive_shape_for` ([world.rs:33278](../src/des/general/soccer/world.rs#L33278)).
- **What's hardcoded:** `defensive_shape_for` is a wall of constants —
  `role_line_bias` (0.70/0.48/0.28/0.10), `width_factor` clamps (0.56–0.92), per-role
  press distances (6.0/3.8/8.5), lateral-pull coefficients
  ([world.rs:33198](../src/des/general/soccer/world.rs#L33198)).

**Conversion:** generalize `back_four_line.rs` into a `LineDepthModel` reused for
the midfield band (and the press trigger line), with the same
`BackFourLineInputs`-style feature vector + head + analytic seed + blend +
band-clamp. The midfield band fn is the drop-in seam; press focus is a second
small head.
- **Training signal:** same pitch-value×xT territorial reward as the back four.
- **Parity:** one gate per line, off = identical (the back-four template guarantees
  this by construction).

---

## Tier 2 — Hardcoded probability / value models (clean swaps, sharpen the sim)

| # | Surface | Live call site | Hardcoded as | Conversion / training signal |
|---|---------|----------------|--------------|------------------------------|
| 4 | **Pass completion** (DORMANT) | head on the sim `pass_completion_head` ([world.rs:277](../src/des/general/soccer/world.rs#L277)); features via `pass_completion_learn_features` ([world.rs:32343](../src/des/general/soccer/world.rs#L32343)) | head is **trained but not consumed live** — `.predict()` never called in the decision path; the analytic completion estimate still drives passes | **promote** `SoccerPassCompletionHead::predict` into the pass ranker behind a readiness gate; corpus already collected |
| 5 | **Pass velocity/power** (PARTIAL) | `dd_soccer_disable_learnable_pass_velocity()` live at [world.rs:10033](../src/des/general/soccer/world.rs#L10033), [world.rs:23446](../src/des/general/soccer/world.rs#L23446) | gated seam **landed on main**, but the chosen speed is a heuristic bucket sweep, not a learned head | feed the power buckets to a small head trained on completion×xT-gain at speed ([[soccer-learnable-pass-velocity]]) |
| 6 | **GK save probability** (HARDCODED) | `goalkeeper_save_probability_from_traits` live at [world.rs:18866](../src/des/general/soccer/world.rs#L18866), [world.rs:34577](../src/des/general/soccer/world.rs#L34577) | closed-form trait/physics formula | learned save model on (shot geometry, pace, keeper traits) → saved?; label is the shot outcome already logged |
| 7 | **Tackle / duel success** (HARDCODED) | `tackle_success_probability` / `slide_tackle_success_probability` / `slide_tackle_foul_probability` ([soccer.rs:43874](../src/des/general/soccer.rs#L43874)) consumed at [world.rs:10512](../src/des/general/soccer/world.rs#L10512), [world.rs:10658](../src/des/general/soccer/world.rs#L10658) | skill-profile formula × `SLIDE_TACKLE_SUCCESS_BOOST` clamp | learned duel model on (closing speed, angle, from-behind, skills) → won/lost/foul; label is the duel outcome |

---

## Tier 3 — Other positioning surfaces (back-four pattern again)

| # | Surface | Live call site | Note |
|---|---------|----------------|------|
| 8 | **Whole `defensive_shape_for`** | [world.rs:33198](../src/des/general/soccer/world.rs#L33198), called from [player.rs:9391](../src/des/general/soccer/player.rs#L9391) &c. | the fore-aft axis is Priority 3; the **lateral** width/compactness constants are a second learnable axis |
| 9 | **GK positioning** | `goalkeeper_ball_goal_tracking_target` ([world.rs:20689](../src/des/general/soccer/world.rs#L20689)), `goalkeeper_buildup_lane_target_for` ([world.rs:19845](../src/des/general/soccer/world.rs#L19845)) | sweep heuristic; learn the start position from shots-faced / chances-conceded |
| 10 | **Off-ball support runs** | `open_space_for` ([world.rs:30791](../src/des/general/soccer/world.rs#L30791)) | heuristic-scored run targets; learn from pitch-value×xT gain of the run |

---

## Tier 4 — Tactical "manager" (the meta-policy above the players)

- **Live call site:** `tactical_directive_for_team` ([soccer.rs:15133](../src/des/general/soccer.rs#L15133))
  sets `home_directive` / `away_directive` ([team.rs:5141](../src/des/general/soccer/team.rs#L5141)).
  It is a **rule selector** (the flaky-on-main
  `tactical_directive_rule_selector_can_invoke_every_attack_strategy` test exercises it).
- **What's hardcoded:** formation, press intensity, line height, width, and attack
  strategy are chosen by rules.
- **Conversion:** a learned higher-level policy choosing the directive from
  (score, time, fatigue, territorial state) — a hierarchical RL agent sitting
  *above* the per-player policies. Larger build; do after Priorities 1–3 so the
  lower policies it commands are already learned.

---

## Suggested order

1. **Priority 1** — wire the discretized action + enable policy inference (low
   build, transformative; it's the actual decision).
2. **Priority 2** — learned xT grid (lifts every net's reward, including the back
   four).
3. **Priority 3** — midfield/press line via the back-four template (cheap,
   parity-safe).
4. Tier 2 promotions (#4 pass-completion is nearly free — just consume the
   already-trained head).
5. Tier 3 / Tier 4 as larger follow-ups.

Every item keeps the back-four contract: **gated, off = byte-identical, analytic
seed as cold-start, the net earns its blend.**
