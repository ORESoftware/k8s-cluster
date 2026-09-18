# Dynamic, learnable back-four line model

## What this is

The forward (y-axis) position of the back four is **not** a fixed formation
slot. At every tick the line has a *centre* — the average y of the defenders
still holding the line — and that centre should be a **dynamic function of the
match state**, ideally one a neural net learns to set optimally over millions of
self-play ticks.

This document specifies that model: its inputs, its output, the eventual-
consistency contract, where it plugs into the engine, and how the learner is
meant to take it over from the analytic seed.

The line *centre* is the controlled quantity. Individual defenders are pulled
onto a flat line around that centre by the existing offside-flatness machinery
(`BACK_FOUR_OFFSIDE_LEVEL_BAND_YARDS`), so setting the centre sets the
collective average-y the user asked for, while the per-defender flatten/cover
logic is left untouched.

## The function

```
line_centre_fwd  =  f( ball, opponents, self, opp_centre_of_mass,
                       possession, own_gk, offside_trap_state )
```

All quantities are expressed in the **defending team's attacking frame** (`fwd =
coord * attack_dir`, lateral = x centred on the pitch spine) so the model is
mirror-invariant: flip the pitch and swap the team and the features — and the
output — are identical. This is the same orientation convention `pitch_value.rs`
uses, and it halves the state space the net must cover.

### Inputs (the 7 groups requested)

| # | Group | Features (normalized, forward frame) |
|---|-------|--------------------------------------|
| 1 | **Ball** position / velocity / acceleration | gap of ball ahead of our own goal (÷L); ball lateral (÷½W); ball vel fwd & lat (÷v_max); ball accel fwd & lat (÷a_max) |
| 2 | **Opponent vector** position / velocity / acceleration | foremost (deepest) attacker's gap toward our goal; opponent **centroid** fwd & lat; opponent **mean** velocity fwd & lat; opponent mean acceleration fwd & lat |
| 3 | **Self** — the back four's own kinematics | line-centre gap ahead of own goal; line-centre lateral; line-centre velocity fwd & lat; line-centre acceleration fwd & lat |
| 4 | **Opponent centre of mass** | folded into group 2 (`opp_centroid_*`) — the centroid *is* the centre of mass of the outfield opponents |
| 5 | **Possession** | `we_control`, `they_control` (both 0 ⇒ contested / loose) |
| 6 | **Own goalkeeper** | GK gap ahead of own goal; GK lateral |
| 7 | **Offside trap state — now and the next ~3 s** | `offside_in_force` (restart suspensions lifted); `trap_active_or_imminent` (offside live **and** ball in a mid/high block so a flat trap is meaningful). The *next-3 s* horizon is supplied implicitly through the ball/opponent **velocity and acceleration** features and through using the **predicted** ball position — the net is meant to learn the lead from them rather than us hard-coding a 3 s extrapolation. |
| 8 | **Existing determinants (retained)** | `heuristic_centre_fwd_from_own_goal` — where the engine's *current* heuristic already puts the line centre (it folds in the directive line target, the ball blend, the role bias, the press focus, and the legal band). The model learns **relative to** this, so the well-tuned existing line is never thrown away — only refined. |

`BACK_FOUR_LINE_FEATURE_DIM = 26`. The exact ordering is the single source of
truth in `back_four_line.rs::BackFourLineInputs::to_features`.

### Retaining existing determinants (group 8)

The model does **not** replace the engine's line wholesale. The existing
heuristic centre is fed in as a feature *and* the final centre is a blend:

```
centre = (1 − blend)·heuristic_centre  +  blend·model_centre        // then re-clamped to the band
```

with `blend = BACK_FOUR_LINE_MODEL_BLEND` (0.5 for the analytic seed — the
existing line still dominates). Every other determinant already in the chokepoint
is preserved unchanged: the predicted-ball lookahead, the 10–30 yd band rails, the
own-goal-zone relaxation, the restart drop-off, the presser-stepped-out / pass-
in-flight / breakthrough-cover exceptions, and the flat ≤2 yd offside band. The
model only chooses **where, inside all of that, the flat line's depth sits** — and
a promoted head can earn a larger `blend` as it proves out.

### Output

A single scalar `gap_fraction ∈ [0, 1]` (sigmoid head):

```
0.0  → line centre sits LEVEL with the (predicted) ball   — highest line / trap
1.0  → line centre sits the full max-gap BEHIND it        — deepest block
line_centre_fwd = predicted_ball_fwd − gap_fraction · max_gap
```

`max_gap` is the existing safety rail (`DEFENSIVE_LINE_MAX_BEHIND_BALL_YARDS`,
capped by room to our own goal). The model chooses *where inside the legal band*
the line sits; it never chooses an illegal line, because the caller re-clamps the
output to `[deepest_fwd, shallowest_fwd]`. The net therefore cannot push the line
offside-ahead of the ball or deeper than the goal — the rails stay analytic and
hard; only the *depth within them* is learned.

## Eventual consistency (the 3-second straying contract)

The model sets a **target only**. It is read inside
`defender_line_band_average_adjusted_y`, which feeds `defensive_shape_for`'s
movement target; players then *ease* onto it over the existing ~3 s movement
grace (`DEFENSIVE_LINE_GRACE_JOG_YARDS`, the offside-recovery clocks). So:

- a defender caught upfield jogs back onto a freshly-dropped line over ~3 s, it
  does not teleport;
- the line is allowed to be transiently wrong — covering a runner, stepping to
  press, recovering shape — without the model fighting it every tick;
- we deliberately do **not** keep the line perfectly optimal each instant; the
  band is a soft attractor, and the exceptions (presser stepped out, breakthrough
  cover, pass-in-flight lane steps) are preserved exactly as today.

Nothing in this model shortens or removes that grace. It only changes *what the
line eases toward*.

## Where it plugs in

`WorldSnapshot::defender_line_band_average_adjusted_y` (in `world.rs`) is the
sole live chokepoint for the line. It already computes the legal band
(`deepest_fwd … shallowest_fwd`) and a heuristic line centre (the clamped
average of the holders). The seam:

```
if let Some(centre) = self.back_four_line_model_centre_fwd(
        me, predicted_fwd, deepest_fwd, shallowest_fwd, max_gap) {
    line_centre_fwd = centre;   // learned / analytic centre, already clamped to band
}
```

When the gate is **off** (default) the method returns `None` and the function is
**byte-identical** to before — the heuristic centre stands. When **on**, the
centre comes from the model (analytic seed today, learned head once trained), and
the surrounding flatten/cover/cap logic is unchanged.

Gate: `DD_SOCCER_ENABLE_BACK_FOUR_LINE_MODEL` (env, off ⇒ parity).

## How the neural net takes over

The seam is deliberately a *function returning a gap fraction*. Today that
fraction comes from `analytic_line_centre_gap_fraction` — a documented,
deterministic, RNG-free policy that encodes the obvious intent (drop deeper when
the ball is driving at our goal / they have it deep / no trap is on; hold a high
line when we press, trap, or control). That seed is the **fallback and the
bootstrap target**. The path to "solve it optimally":

1. **Head.** `BackFourLineHead` is a `FeedForwardNetwork` regression head
   (25 → hidden → 1, sigmoid), mirroring `SoccerPassCompletionHead` /
   `SoccerPolicyHead`: live net is not serde, it round-trips through the existing
   `SoccerNeuralNetworkSnapshot` Postgres path. Construction + `predict` + `train`
   are in place; it is wired but not yet consumed live (same staging as the
   pass-completion head).
2. **Features.** `BackFourLineInputs::to_features` is the captured state vector.
   Logged per tick, it is the training corpus's `x`.
3. **Label / reward.** Two viable signals, both already available in-engine:
   - *Supervised seed*: regress toward `analytic_line_centre_gap_fraction` to
     bootstrap a sane line, then
   - *RL*: credit the line depth with the spatial **pitch-value × xT** reward
     (`pitch_value.rs`) plus the conceded-shot / offside-trap-sprung outcomes, so
     the net learns the depth that maximises net territorial threat while
     minimising balls played in behind. The gap fraction is the action; the
     reward is the change in net dangerous-space control over the next window.
4. **Promotion.** Once `predict` beats the analytic seed on held-out matches,
   flip `back_four_line_model_centre_fwd` to read the head instead of the
   analytic fraction. The seam does not change; only the source of the fraction
   does.

## Parity & safety summary

- Off by default ⇒ byte-identical (the method returns `None` before building any
  features).
- Output is always re-clamped to the legal `[deepest_fwd, shallowest_fwd]` band:
  the net can never set an offside-ahead or goal-side-illegal line.
- The presser-stepped-out, breakthrough-cover, pass-in-flight, own-goal-zone, and
  restart-drop-off exceptions are untouched — the model only replaces the
  *centre*, inside the same guards.
- Pure / RNG-free feature build and analytic fraction ⇒ deterministic and
  testable without enabling the live seam.

---

# Midfield line model (same machinery, second line)

The line-depth model is **generalized**: the feature struct (`BackFourLineInputs`),
the head (`BackFourLineHead`), and the role-parameterized builder
(`build_line_depth_inputs(team, role)`) are line-agnostic. The midfield line reuses
all of it — only which players are "self" (`Midfielder` vs `Defender`) and the
output mapping differ.

## What's the same

- The **same 26-d feature vector**, in the same attacking frame — now describing
  the *midfield* line's own kinematics in the `line_*` slots (the builder is called
  with `PlayerRole::Midfielder`).
- The **same analytic-seed → head** path; `analytic_midfield_gap_fraction` is the
  midfield seed, `BackFourLineHead` the (shared) trainable head.
- The **same eventual-consistency contract**: the model sets a target; players ease
  onto it. The grace is **5 seconds** for the midfield (vs ~3 s for the back four) —
  the midfield is allowed to be looser.

## What's different

1. **Reference & output.** The back four's depth is a gap *behind the ball*; the
   midfield's is a gap *ahead of the back four*. `midfield_line_model_ideal_gap_fraction`
   returns a fraction in `[0, 1]`; the seam maps it onto the existing
   `MID_AHEAD_OF_DEF_{MIN=5, MAX=20}` band (so `0` = compressed onto the defenders,
   `1` = a 20 yd stagger). It replaces the fixed `MID_AHEAD_OF_DEF_IDEAL_YARDS = 10`.
2. **No offside semantics.** The midfield does not set offside, so its flatten is
   **symmetric** — it simply levels into a line (`MIDFIELD_LINE_LEVEL_BAND_YARDS`,
   ±1.5 yd), with none of the back four's asymmetric "never step ahead of the line"
   trap clamp.
3. **It forms an actual flat line.** While defending (model on), each midfielder is
   pulled onto the line **centre** — the back four's average plus the dynamic ideal
   gap — within the level band. In possession it falls through to the looser
   push-on band (the midfield stretches to support the attack).

## Where it plugs in

`WorldSnapshot::midfield_line_band_adjusted_target` (the midfield band chokepoint,
analogous to `defender_line_band_average_adjusted_y`). When the model is on it (a)
makes the ideal gap dynamic, (b) switches to the 5 s window, and (c) flattens the
line while defending. Gate: `DD_SOCCER_ENABLE_MIDFIELD_LINE_MODEL`. **Off ⇒
`ideal_gap` = the fixed constant, the 3 s window, and no flatten = byte-identical**
to the legacy band.

## Training signal

Same as the back four — the pitch-value × xT territorial reward, conditioned on the
midfield's own depth. The shared head can carry a line-id input later if a single
net is preferred over one head per line; today each line trains its own head.

