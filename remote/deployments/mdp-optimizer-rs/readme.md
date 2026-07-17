# `remote/deployments/mdp-optimizer-rs`

Rust service for asynchronous MDP, POMDP, and RL-style value optimization in the
remote Kubernetes runtime.

It exposes:

- `GET /healthz`
- `GET /metrics`
- `POST /optimize`
- `POST /telemetry/learn`

It also queue-subscribes to `dd.remote.mdp.optimize` with queue group
`dd-mdp-optimizer` and `dd.remote.telemetry.mdp` with queue group
`dd-mdp-telemetry-learner`. It publishes results to `dd.remote.mdp.results` plus
a compact event on `dd.remote.events`.

The solver uses finite checked probabilities, transition normalization with
warnings, bounded discounted value iteration, Q-values, greedy policy extraction,
and optional belief-state projection/posterior calculation for POMDP-style
requests.

## `POST /optimize`

Requests use camelCase JSON:

```json
{
  "requestId": "example-policy-run",
  "kind": "mdp.value-iteration",
  "states": ["start", "done"],
  "actions": ["wait", "go"],
  "transitions": [
    { "state": "start", "action": "wait", "nextState": "start", "probability": 1 },
    { "state": "start", "action": "go", "nextState": "done", "probability": 1 },
    { "state": "done", "action": "wait", "nextState": "done", "probability": 1 },
    { "state": "done", "action": "go", "nextState": "done", "probability": 1 }
  ],
  "rewards": [
    { "state": "start", "action": "wait", "value": 0 },
    { "state": "start", "action": "go", "value": 1 },
    { "state": "done", "action": "wait", "value": 2 },
    { "state": "done", "action": "go", "value": 0 }
  ],
  "gamma": 0.5,
  "tolerance": 1e-8,
  "maxIterations": 10000
}
```

Responses include convergence metadata, `policy`, `values`, `qValues`, any
normalization warnings, and a `generatedAtMs` timestamp. State, action, and
observation labels must be unique so the optimizer cannot silently collapse two
different labels into one index.

## POMDP-style belief projection

Add `belief` to score actions at a partial-observability belief state. Add
`observations`, `observationModel`, and `observed` to compute a Bayesian
posterior. `beliefAction` optionally pins the action used for the posterior; if
omitted, the optimizer uses the highest-value action at the belief point.

Observation probabilities are validated, grouped by `(action, nextState)`, and
normalized with warnings when a row sums to a finite positive value other than
`1`. Transition probabilities receive the same treatment; missing transition
rows are filled with self-loops and reported in `warnings`.

## Telemetry learning

`POST /telemetry/learn` accepts weighted telemetry signals, scores operational
risk, and builds a bounded MDP request in a background worker. The response
includes ranked signal insights, the current risk state, the recommended action,
and the underlying optimization output so cron dashboards can inspect why a
decision was made.

```json
{
  "requestId": "cluster-window-2026-05-16T18:00Z",
  "scope": "infra",
  "windowMs": 300000,
  "signals": [
    {
      "name": "node_cpu_utilization",
      "service": "dd-dev-server-api",
      "layer": "infra",
      "value": 0.92,
      "warning": 0.7,
      "critical": 0.9,
      "weight": 1,
      "actionImpacts": [{ "action": "scale-up", "delta": 0.55, "confidence": 0.9 }]
    },
    {
      "name": "http_5xx_rate",
      "service": "dd-dev-server-api",
      "layer": "app",
      "value": 0.08,
      "warning": 0.02,
      "critical": 0.1
    }
  ],
  "actions": ["hold", "observe", "scale-up", "page-human"]
}
```

Signals can use `warning`/`critical`, `target`, or `baseline` references. Set
`higherIsBetter` for availability or success-rate signals where drops increase
risk. If callers omit `actions`, the optimizer chooses app and infra defaults
such as `scale-up`, `enable-fallback`, `throttle-feature`, `restart`,
`shed-load`, and `page-human`.

Telemetry requests are intentionally bounded: HTTP and NATS payloads are capped
at 256 KiB, each request can include at most 128 signals and 32 actions, and
each signal can include at most 16 action-impact hints. Scope/layer/action
labels are normalized before scoring. Action-impact confidence must be in
`[0, 1]`, impact deltas must be in `[-1, 1]`, and negative custom impacts
override default action priors so observed harmful interventions are not
silently recommended.

The reaper/cron deployment can publish optimization requests to NATS when it
wants policy help for scheduling, worker warm pools, queue backoff, or resource
allocation decisions.

> **ORM policy:** prefer **SeaORM** over sqlx for new database code (MASH stack: maud, axum, SeaORM, supabase, htmx).
