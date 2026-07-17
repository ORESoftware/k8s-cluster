# `dd-func-approx-rs`

A **function approximator** over HTTP and NATS — a small, modern **Eureqa**
clone. Given a dataset it discovers a non-linear regression model and returns it
in as **analytic** a form as the method allows, balancing **accuracy against
simplicity**. Slots beside the monte-carlo / evolution / economics servers.

It combines **numeric** and **analytic** methods, and combines **evolutionary
programming with neural nets**:

- `symbolic` *(default)* — **genetic-programming symbolic regression**. Evolves
  expression trees over a configurable set of building blocks (`+ - * / sin cos
  exp log sqrt square tanh abs`) and returns **human-readable equations** along
  an accuracy/complexity **Pareto front**, plus the **symbolic derivatives** of
  the chosen equation. Every candidate is fitted with its optimal affine wrapper
  `a·f(x)+b` **in closed form** (Keijzer *linear scaling*), so an analytic
  least-squares solve rides inside the evolutionary loop; a numeric constant
  hill-climb then polishes the survivors. Fits **raw units**, so equations are in
  your own variables.
- `neural` — a small **MLP trained by backpropagation** (exact analytic
  gradients, Adam, early stopping on a validation fold).
- `evolution` — the **same MLP, weights evolved** by a self-adaptive
  (μ/μ_I, λ) **Evolution Strategy** (gradient-free neuroevolution).
- `hybrid` — neuroevolution **then** a short gradient-descent polish (memetic).
- `linear` — closed-form **ridge polynomial least squares** (exact, analytic;
  returns the polynomial as a string).
- `auto` — runs symbolic + neural + evolution on trimmed budgets and keeps
  whichever **generalises best**, preferring the simpler analytic answer on a
  near-tie. Reports every candidate.

Deterministic: pure-Rust math with a seeded SplitMix64 PRNG, so every fit is
reproducible from `seed`.

## HTTP

- `GET /`, `GET /ui` — interactive browser playground (see **Web UI**)
- `GET /ui/config.json` — UI bootstrap (dd-data-viz URL, whether auth is required)
- `GET /healthz`, `GET /metrics`
- `POST /approximate` (alias `POST /fit`)

```bash
# Rediscover y = 3·sin(x) + 0.5·x²  as an analytic equation
curl -s localhost:8139/approximate -H 'content-type: application/json' -d '{
  "method": "symbolic",
  "samples": [ {"x":[-2.0],"y":-0.728}, {"x":[-1.0],"y":-2.024}, {"x":[0.0],"y":0.0},
               {"x":[1.0],"y":3.024}, {"x":[2.0],"y":4.728}, {"x":[3.0],"y":4.923} ],
  "seed": 7,
  "predictAt": [[5.0]],
  "config": { "generations": 60, "population": 400,
              "operators": ["+","-","*","sin","square"] }
}'
```

Response highlights: `expression` (the chosen equation), `derivatives`
(`∂/∂xᵢ`, symbolic), `complexity`, `paretoFront` (each member with
`complexity`, `trainRmse`, `valRmse`, `valR2`, `expression`), `train` /
`validation` metrics (RMSE, MAE, R² on the original units), `model` (the
analytic spec — equation, MLP layers + scalers, or polynomial coefficients),
optional `predictions` / `predictedValues`, plus `iterations` and `durationMs`.

### Input shapes

Data may be given three ways: `samples: [{x:[..], y}]`, parallel
`inputs: [[..]] + targets: [..]`, or single-feature `x: [..] + y: [..]`.
Name variables with `variableNames` (defaults `x0, x1, …`). Knobs live under
`config`: GP (`population`, `generations`, `maxDepth`, `operators`,
`parsimony`, `tournament`, `constOptIters`); NN/hybrid (`hidden`, `activation`,
`epochs`, `learningRate`, `batchSize`); ES (`esPopulation`, `esParents`,
`esGenerations`, `sigma`); polynomial (`degree`, `ridge`). `valFraction`
(default `0.2`) sizes the held-out fold.

## NATS (subjects in `remote/libs/nats/subject-defs`)

| Env | Default subject | Meaning |
| --- | --- | --- |
| `FUNC_APPROX_FIT_SUBJECT` | `dd.remote.funcapprox.fit.requests` | inbound requests (queue group `dd-func-approx-rs`) |
| `FUNC_APPROX_RESULT_SUBJECT` | `dd.remote.funcapprox.fit.results` | published `funcapprox.fit.v1` results |
| `FUNC_APPROX_EVENT_SUBJECT` | `dd.remote.events` | runtime events |

`PORT` defaults to `8139`. Set `NATS_URL` to enable the request/result lane.

## Web UI

`GET /` (and `/ui`) serves a **self-contained browser playground** — inline
HTML/CSS/JS in [`ui.html`](ui.html), no build step, no external assets or CDNs.
Pick a sample function (or paste your own `samples` JSON), add optional noise,
choose a method and a couple of knobs, and **Fit** posts to same-origin
`/approximate`. It shows the chosen **analytic equation**, its **symbolic
derivatives**, train/validation **metrics**, **warnings**, and the raw response,
plus charts:

- **Fit** — for single-feature data, the scatter of `(x, y)` with the fitted
  model curve overlaid (sampled server-side via a dense `predictAt` grid, so it
  reflects the *actual* model for every method, not just the symbolic ones).
- **Predicted vs actual** — the dimension-agnostic diagnostic (with a `y = x`
  reference) used automatically for multi-feature data.
- **Pareto front** — complexity vs validation RMSE, when a symbolic/auto front
  is present.

Charts render **locally** as dependency-free inline SVG by default. If
`FUNC_APPROX_DATA_VIZ_URL` is set, the UI also offers a **dd-data-viz** renderer
toggle that posts a [dd-data-viz `/render`](../dd-data-viz-rs) spec
(`{mark, x, y, rows}`) to that service and shows the returned SVG, falling back
to local rendering on any error. No secret is exposed to the browser:
`/ui/config.json` reports only the URL and whether `/approximate` requires a
token (the UI then prompts for one and sends it as `x-server-auth`).
The request payload is the same JSON as `POST /approximate`.

## Limits & hardening

**Wall-clock budget.** Every fit has a server-side deadline
(`FUNC_APPROX_MAX_FIT_MS`, default `20000`, clamped `[500, 120000]`). The
genetic-programming, evolution-strategy, and gradient loops all check it
cooperatively — between generations *and* between individual candidate
evaluations — so no single request can pin a core, regardless of the
`population` / `generations` / `epochs` knobs. When it elapses the fit returns
the **best model found so far** with a `warning` (and `auto` gives each method an
equal slice of the budget). Combined with the inflight cap this bounds total CPU.

**Concurrency.** Inflight cap (`FUNC_APPROX_MAX_INFLIGHT`, default 8); HTTP
returns `503` when saturated, NATS applies backpressure (and redelivers).

**Size bounds.** ≤ 50 000 rows, ≤ 32 features (the 8 MiB body limit binds
first), `predictAt` ≤ 2 000, returned `predictions` ≤ 5 000 (truncated with a
warning). Inputs/targets must be finite with magnitude ≤ `1e150` (so squared
sums can't overflow); `predictAt` rows must be finite; variable names ≤ 64 chars;
`requestId` is truncated to 200. Symbolic search runs over a ≤ 2 000-row sample
of the training fold for responsiveness (metrics are still computed on the full
data); gradient/evolution learners subsample to ≤ 20 000 rows. GP budgets are
clamped (`population ≤ 2000`, `generations ≤ 200`, `maxDepth ≤ 12`); a symbolic
derivative that exceeds 400 nodes after simplification is omitted with a warning.

**Memory.** MLP shape is capped at 4×64 hidden units regardless of the requested
`hidden` (the Evolution Strategy holds λ copies of the weight vector, so an
unbounded net is an OOM vector); ES population is capped at 256. The polynomial
fit streams the normal equations (`XᵀX`/`Xᵀy`) row by row, so its memory is O(m²)
in the basis size, never the full design matrix. Measured peak RSS under eight
concurrent giant-net fits is ~130 MiB against the 1 GiB pod limit.

**Numerical safety.** Protected operators (safe `/`, `log`, `sqrt`, clamped
`exp`) keep evaluation finite; any candidate that still produces a non-finite
prediction is assigned infinite error rather than poisoning the metrics. The
reported metrics clamp each residual so a huge-but-finite prediction can't
overflow RMSE/R² into `null`, and every outgoing prediction array is sanitised so
an unstable model never serialises as JSON `null` inside an `f64[]`. Non-finite
numeric config (`NaN`/`Inf`) is rejected. Scalers are fitted on the **training
fold only** to avoid validation leakage.

**Analytic-formulation engine.** The chosen symbolic equation and its
derivatives are canonicalised by **e-graph equality saturation** (the `egg`
crate — the engine [Herbie](https://herbie.uwplse.org/) uses), turning the raw,
evolution-shaped tree into the simplest analytic form: like-terms combined
(`x+x → 2x`), `x·x → x²`, common factors pulled out, the linear-scaling wrapper's
scattered constants folded (`a·(f−c)+b → a·f+(b−ac)`), and exact identities
applied (`√(a²) → |a|`, `|−a| → |a|`, parity `sin(−x) → −sin(x)`, `cos(−x) →
cos(x)`). The rewrites are **value-preserving under the engine's own protected
arithmetic** (e.g. `a/a → 1` holds because protected division returns `1` at a
near-zero denominator too; rearrangements *across* protected division are
intentionally excluded because they are not), the constant folder reuses the
exact `Unary::eval`/`Binary::eval` semantics, and extraction minimises the **same
complexity weight** the Pareto front reports. The pass is bounded on nodes,
iterations *and* the remaining wall-clock budget, and is total — on any failure
it returns the input unchanged, so it can never break a fit. The result is then
rendered by a **precedence-aware pretty-printer** that drops redundant
parentheses, writes `square` as `^2` and a trailing negative constant as a
subtraction (`((2 * (x0 * x0)) + -3) → 2 * x0^2 - 3`), and **recognises
materialised constants** as clean analytic symbols (`π`, `e`, `√2`, `√3`, `ln 2`,
`φ`, small rationals). Symbolic **derivatives match the protected operators**
they differentiate (away from the clamp/degenerate guards of `exp` and protected
division) — in particular `d/dx √(|x|)` carries the `sign(x)` factor, so it stays
finite and correctly-signed for negative `x` (the earlier form went `NaN`). The
whole pass is deterministic (egg's `deterministic` feature), so a fit is still
fully reproducible from `seed`.

## Authentication

Optional and **off by default** (matching the sibling compute services). Set
`FUNC_APPROX_AUTH_SECRET` (or the shared `SERVER_AUTH_SECRET`) to require callers
of `/approximate` to present a matching `x-server-auth: <secret>` (or
`auth: <secret>`) header; the comparison is constant-time. When unset the
endpoint is open. `/healthz` and `/metrics` are always open (for probes and
Prometheus). Rejections return `401` and increment `*_auth_failures_total`. The
deployment manifest wires `FUNC_APPROX_AUTH_SECRET` from the `dd-agent-secrets`
secret with `optional: true`, so enabling auth is a one-key secret edit.

## Layout

| File | Role |
| --- | --- |
| `src/gp.rs` | symbolic regression: expression trees, linear scaling, Pareto archive, simplify, symbolic diff |
| `src/algebra.rs` | analytic-formulation engine: e-graph (`egg`) canonicalisation + analytic constant recognition |
| `src/nn.rs` | MLP forward / backprop (Adam, early stopping) |
| `src/evo.rs` | self-adaptive (μ/μ_I, λ) Evolution Strategy over MLP weights |
| `src/linalg.rs` | closed-form ridge least squares (normal equations, partial pivoting) |
| `src/data.rs` | dataset, standardisation, seeded split, RMSE/MAE/R² |
| `src/fit.rs` | request/response contract, method dispatch, `auto`, analytic output |
| `src/main.rs` | axum HTTP + NATS server wiring, metrics, auth, runtime-config, UI routes |
| `ui.html` | self-contained browser playground (served at `/`), optional dd-data-viz rendering |

Run the tests with `cargo test` (covers eval/simplify/derivative correctness and
end-to-end recovery of a quadratic, a line, and `sin`).

> **ORM policy:** prefer **SeaORM** over sqlx for new database code (MASH stack: maud, axum, SeaORM, supabase, htmx).
