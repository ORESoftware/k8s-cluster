# DEN-862: Structured exploration after apparent policy convergence

## Status

Implementation audit and experiment contract for `akrion-soccer-engine-rs`.

Related Linear issues:

- `DEN-862` — structured exploration and plateau escape implementation.
- `DEN-103` — plateau diagnosis, evaluation, and experiment observability.
- `DEN-104` — tactical planning and skill execution.
- `DEN-863` — decision-model architecture beyond a plain single-agent MDP.

## Source question

When an MDP or POMDP repeatedly chooses the same apparently optimal action for a
state, has learning stopped? Should the engine occasionally choose the second-
or third-ranked feasible action in nearby states or state permutations so it can
observe different outcomes?

The practical answer is **yes, under an explicit behavior policy**, with a crucial
qualification: repeated argmax behavior is not proof that an approximate learner
has solved the environment.

## Distinguish four cases

### 1. Exactly solved, stationary, fully known finite MDP

If the transition and reward models are exact, the state is fully observed, the
environment is stationary, and dynamic programming has found the true optimal
policy, additional exploratory interaction has no informational value for that
model. Exploration can be disabled for evaluation and production behavior.

### 2. Approximate learned policy or value function

The engine normally has a current best estimate under finite data, function
approximation, imperfect optimization, sparse or delayed rewards, and a limited
state distribution. A stable argmax may therefore be a local optimum, a blind
spot, a calibration failure, or a consequence of never visiting alternatives.
Controlled exploration remains useful.

### 3. POMDP or belief-state policy

The policy acts on an observation or belief state rather than the hidden true
state. An action can be valuable because it reveals pressure, defender speed,
opponent intent, fatigue, or a passing lane. Such a probing action is not merely
a deliberately bad action; it can be optimal because of its value of
information.

### 4. Non-stationary self-play or opponent-facing environment

When opponents, teammate policies, reward weights, or simulator behavior evolve,
the effective environment changes. A previously strong action can become
exploitable. A small adaptation/exploration budget can remain justified even
after apparent convergence.

## Existing implementation on `main`

The engine already contains a substantial structured-exploration implementation.
This RFC records it instead of creating a second competing selector.

### Deterministic stochastic top-k selector

`src/des/general/soccer/policy_select.rs` provides:

- the default-off `DD_SOCCER_ENABLE_STOCHASTIC_POLICY_TOPK` gate;
- deterministic unit draws derived from match seed, player, tick, and decision
  site, preserving replayability;
- rank-aware sampling among the best three candidates;
- renormalization when only one or two candidates exist;
- score-aware selection entry points;
- behavior-policy probability reporting for learning/importance-weighting;
- unit tests for partition boundaries, degenerate inputs, replay determinism,
  behavior probabilities, and value-weighted sampling.

With the gate off, selection remains deterministic argmax.

### Rank policy: preserve both historical and current profiles

The original requested experiment was:

```text
best         70%
second-best  20%
third-best   10%
```

A later plateau-break change widened the compile-time default in
`PolicySelectionTunables` to:

```text
best         50%
second-best  30%
third-best   20%
```

These are not mutually exclusive ideas. Treat them as two named experiment
profiles under the same selector:

- `rank_70_20_10`: conservative top-three exploration baseline;
- `rank_50_30_20`: broader plateau-break profile currently represented by the
  tunable defaults.

Do not blindly replace one with the other in an experiment report. Compare them
under matched seeds, interaction budgets, opponents, and compute budgets.

### Value-weighted Boltzmann mode

The selector also supports value-weighted softmax sampling when
`policy_selection.boltzmann_temperature > 0`:

```text
P(action) proportional to exp(score / temperature)
```

This is conceptually stronger than rank alone:

- near-tied actions can receive meaningful exploration mass;
- a clearly superior action remains dominant;
- lower-ranked but competitive candidates can be sampled;
- non-finite or ineligible candidates receive no useful mass;
- the chosen behavior probability is reported.

Rank-aware and value-aware exploration should both remain available because they
answer different experimental questions.

### Behavior-policy honesty

The engine must preserve the distinction between the behavior policy that gathers
data and the target policy being learned.

- Off-policy Q-learning-style updates may gather exploratory transitions while
  retaining the appropriate greedy/target-policy bootstrap.
- PPO, actor-critic, MAPPO, and importance-weighted paths must persist the actual
  sampled behavior probability or log-probability.
- Every transition intended for policy-gradient learning should be able to record
  the chosen candidate, rank, score, sampling mode, behavior probability, and
  exploration reason.

The current selector reports the promoted candidate's behavior probability.
There is a documented residual limitation: downstream feasibility or MPC checks
can reject that promoted candidate and execute a later one. The probability then
refers to the promotion rather than the finally executed action. This must be
reconciled before claiming exact on-policy likelihood accounting.

## Semantic merge decision

The combined design retains every distinct useful concept:

1. deterministic greedy behavior as a control;
2. configurable second-/third-ranked action sampling;
3. both 70/20/10 and 50/30/20 rank profiles as comparable experiments;
4. Boltzmann value-weighted sampling for score-gap awareness;
5. deterministic seeds and replay manifests;
6. explicit behavior probabilities for learning correctness;
7. feasibility masks and executed-action reconciliation;
8. future uncertainty- and information-gain-directed exploration.

No existing implementation should be replaced wholesale merely because another
branch names the same idea differently. A change is a true superset only when it
preserves these distinct behaviors and their tests.

## Required experiment profiles

At minimum, DEN-862 should expose and compare:

| Profile | Selection behavior | Purpose |
| --- | --- | --- |
| `greedy` | Always choose the best feasible candidate | No-exploration control |
| `rank_70_20_10` | Sample among top three by fixed rank weights | Conservative baseline |
| `rank_50_30_20` | Broader top-three rank exploration | Plateau-break baseline |
| `boltzmann_cold` | Low-temperature score-weighted sampling | Explore near ties only |
| `boltzmann_hot` | Higher-temperature score-weighted sampling | Broader value-aware search |
| `uncertainty_directed` | Value plus calibrated uncertainty bonus | Future UCB/ensemble baseline |
| `posterior_sampled` | Thompson/posterior model sample then greedy | Future model-uncertainty baseline |

Fixed rank profiles must not become unexplained permanent defaults. Training,
adaptation, validation, and production/evaluation settings must be independently
configurable and reported.

## Nearby-state and permutation generalization

Trying a different action is only part of the problem. The learner should not
memorize every nearly identical soccer state independently.

Add deterministic held-out neighborhoods covering:

- small position, velocity, acceleration, orientation, and timing perturbations;
- valid left/right field mirroring;
- equivalent player-role or identity permutations where identity should not
  affect the decision;
- observation noise and hidden-state ambiguity;
- nearby latent or retrieval-neighbor states not seen exactly during training.

Measure:

- action-rank stability;
- policy continuity;
- value and uncertainty calibration;
- outcome robustness;
- symmetry violations;
- regret caused by exploratory and greedy choices.

Function approximation, symmetry-aware augmentation, state abstraction, and
nearest-neighbor/latent retrieval can all contribute. Exploration alone does not
solve poor state representation.

## POMDP information-gathering fixtures

At least one deterministic fixture should demonstrate an action whose immediate
reward is lower but whose observation improves a later decision. Soccer examples
include:

- recycle laterally to reveal whether the opponent follows or holds shape;
- delay a pass to observe a defender's acceleration and body orientation;
- probe a passing lane before committing to a through ball;
- scan or reposition to reduce uncertainty about an off-screen runner.

The report should compare immediate-value-only ranking with belief-aware
value-of-information ranking.

## Experiment manifest

Every stochastic run should persist at least:

```text
repository commit
engine/environment version
seed set
opponent/checkpoint set
selection profile
rank weights
temperature
uncertainty/novelty coefficients
annealing schedule
candidate feasibility mask
selected and executed action
selected and executed action rank
behavior probability/log-probability
exploration reason
state-neighborhood fixture id
wall-clock and interaction budget
```

A run without enough information to replay the behavior policy is not a valid
comparison.

## Evaluation contract

Compare profiles under matched budgets and multiple seeds. Report confidence
intervals rather than a single best run.

Core diagnostics include:

- held-out win rate and exploitability;
- goals, expected goals/threat, shot quality, possession value, and turnover
  risk;
- action entropy and state/action coverage;
- top-1/top-2/top-3 value gaps;
- selected-action rank and action regret;
- uncertainty calibration;
- information gain for probing actions;
- value error and learned-model rollout error;
- rules, physics, and real-time-budget regressions.

Exploration is useful only when it creates information or discovers behavior that
improves held-out outcomes. More variance by itself is not progress.

## Remaining implementation gaps

The current top-k/Boltzmann selector satisfies an important part of DEN-862, but
the broader issue remains in progress. The next implementation slices are:

1. reconcile promoted-candidate and finally executed-action probabilities after
   feasibility/MPC vetoes;
2. name and serialize exploration profiles rather than relying on implicit
   process-wide defaults;
3. add annealing/adaptation schedules to the run manifest;
4. add uncertainty-directed and information-gain baselines;
5. add nearby-state, mirror, and valid role-permutation fixtures;
6. separate training/adaptation exploration from validation and production
   evaluation;
7. connect selector telemetry to DEN-103 plateau dashboards and confidence-
   interval reports.

## Definition of done for this RFC slice

- Existing selector behavior is recognized as canonical rather than duplicated.
- Historical 70/20/10 and current 50/30/20 semantics are documented as separate
  experiment profiles.
- Boltzmann selection and behavior-probability requirements are documented.
- The feasibility/probability limitation is explicit.
- Remaining implementation and evaluation work is traceable to DEN-862 and
  DEN-103.
