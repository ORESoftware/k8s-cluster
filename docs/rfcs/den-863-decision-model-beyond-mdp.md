# DEN-863: Soccer decision model beyond a plain MDP

## Status

Draft implementation RFC for `akrion-soccer-engine-rs`.

Related Linear issues:

- `DEN-863` — design soccer decision model beyond plain MDP.
- `DEN-104` — tactical planner, off-ball skills, and execution behavior.
- `DEN-862` — plateau escape, structured exploration, and reward-shaping experiments.
- `DEN-103` — learning metrics, diagnostics, and experiment tracking.

## Problem

A single-agent, fully observed MDP is useful as a teaching primitive, but it is not expressive enough for soccer player decisions. A soccer tick has many simultaneous actors, local observations, hidden tactical intent, continuous motion, delayed credit, opponent adaptation, and team-level objectives.

The engine should keep MDP/RL vocabulary where it helps, but the core design should model soccer as a hierarchical, multi-agent, partially observable decision system.

## Reward and penalty semantics

The engine should expose one canonical reward contract:

- rewards are scalar values that may be positive, zero, or negative;
- penalties are represented as negative reward components;
- cost-style views are allowed as adapters where `cost = -reward`;
- every aggregate reward should be explainable as a breakdown of named components;
- shaping rewards must not be able to overpower terminal match outcomes without an explicit config decision.

Example component sketch:

```text
score_goal                 +100.0
concede_goal               -100.0
increase_expected_threat     +2.0
lose_possession              -5.0
line_breaking_pass           +3.0
foul                         -8.0
bad_forced_shot              -4.0
transition_defense_success   +2.5
infeasible_action            -6.0
```

The implementation should prefer a structured record over a bare float:

```rust
pub struct RewardBreakdown {
    pub terminal: f32,
    pub possession: f32,
    pub threat_delta: f32,
    pub tactical_shape: f32,
    pub defensive_transition: f32,
    pub legality: f32,
    pub feasibility: f32,
}

impl RewardBreakdown {
    pub fn total(&self) -> f32 {
        self.terminal
            + self.possession
            + self.threat_delta
            + self.tactical_shape
            + self.defensive_transition
            + self.legality
            + self.feasibility
    }
}
```

## Decision model

The practical target is a hierarchy, not one giant MDP:

```text
Team tactic layer
  chooses: press, sit deep, counter, recycle, overload, play direct

Role/player decision layer
  chooses: pass, shoot, carry, receive, support, mark, press, cover, rotate

Low-level controller
  chooses: movement target, acceleration, body orientation, spacing correction
```

The model should support these formal views:

| View | Use in engine |
| --- | --- |
| Contextual action-value model | Fast local pass/shoot/dribble/hold scoring. |
| POMDP | Per-player partial observation and short history/belief state. |
| Markov/stochastic game | Simultaneous interaction among both teams. |
| Dec-POMDP approximation | Shared team reward with local observations. |
| MPC/steering controller | Low-level movement, spacing, and collision avoidance. |
| Offline imitation/value layer | Pretrain on traces before self-play RL. |

## State and observation contract

The engine should distinguish omniscient simulation state from what a player-policy observes.

```rust
pub struct WorldState {
    pub tick: u64,
    pub phase: MatchPhase,
    pub ball: BallState,
    pub players: Vec<PlayerState>,
    pub score: ScoreLine,
}

pub struct PlayerObservation {
    pub self_player: PlayerState,
    pub ball_relative: RelativeBallState,
    pub visible_teammates: Vec<ObservedPlayer>,
    pub visible_opponents: Vec<ObservedPlayer>,
    pub role_context: RoleContext,
    pub recent_memory: ObservationMemory,
}

pub enum TeamTacticAction {
    PressHigh,
    MidBlock,
    LowBlock,
    CounterAttack,
    PossessionRecycle,
    OverloadLeft,
    OverloadRight,
    DirectPlay,
}

pub enum PlayerAction {
    Pass(PassIntent),
    Shoot(ShotIntent),
    Carry(CarryIntent),
    SupportRun(SupportRunIntent),
    Press(PressIntent),
    Mark(MarkIntent),
    Cover(CoverIntent),
    HoldShape,
}

pub struct JointDecision {
    pub team_tactics: [TeamTacticAction; 2],
    pub player_actions: Vec<(PlayerId, PlayerAction)>,
}
```

`WorldState` is allowed inside the referee, physics, replay, and evaluation layers. Learned or heuristic player policies should consume `PlayerObservation` plus bounded memory unless a test explicitly opts into omniscient diagnostics.

## Simultaneous action resolution

The tick resolver should evaluate joint actions, not serialize players as if they act in isolation. A deterministic resolver can still order tie-breaks, but it should record that the conceptual input was a joint decision.

Recommended step shape:

```rust
pub fn step_joint_decision(
    state: &WorldState,
    decisions: &JointDecision,
    config: &DecisionModelConfig,
) -> StepResult {
    // 1. Validate action legality and feasibility.
    // 2. Resolve ball contests and possession transitions.
    // 3. Apply low-level movement/MPC targets.
    // 4. Advance match phase and referee events.
    // 5. Compute reward breakdowns for both teams and individual diagnostics.
    // 6. Emit replay-safe decision trace rows.
    todo!()
}
```

## Encoder direction

For learned policies, prefer entity/relation encoders over flat hand-ordered vectors where feasible:

- players and ball are entities;
- edges carry distance, angle, velocity closing rate, pressure, passing-lane obstruction, and team relation;
- global context carries phase, score, time, tactic, and role assignment;
- output heads score action families first, then parameters.

A graph or transformer-style encoder should remain an implementation detail behind the policy trait so heuristic, tabular, and neural policies can coexist.

## Benchmark fixtures

DEN-863 should add deterministic fixtures before large training changes:

1. pressured pass versus carry versus recycle;
2. support run angle selection;
3. counterattack after turnover;
4. low-block breaking with overloads;
5. high press with cover shadow;
6. transition defense after a failed dribble;
7. hidden-context scenario where memory should beat a memoryless policy.

Each fixture should compare:

- current policy result;
- hierarchical decision result;
- reward breakdown;
- possession/threat delta;
- deterministic replay equivalence.

## Boundaries with related issues

This RFC does not replace the tactical planner in `DEN-104`; it defines the decision model contract that the planner should target.

This RFC does not replace the exploration work in `DEN-862`; it defines the reward and observation abstractions that exploration experiments should use.

This RFC does not replace metrics work in `DEN-103`; it requires reward breakdowns and fixture outcomes to be reported in a way that DEN-103 can track over time.

## Definition of done

- Reward/penalty semantics are versioned and tested.
- `WorldState` and `PlayerObservation` are clearly separated.
- Joint actions are represented explicitly.
- Reward breakdowns are available in decision traces.
- Deterministic fixtures cover on-ball, off-ball, defensive, and hidden-context decisions.
- The current policy stack can be compared against the hierarchical model without deleting the existing MDP/POMDP/MPC primitives.
