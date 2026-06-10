# `dd-monte-carlo-server`

A generic **Monte Carlo** estimation engine over HTTP and NATS. Every experiment
returns a point estimate with its **standard error** and a **95% confidence
interval** (Welford accumulation), so results are statistically honest rather
than a single number. Slots beside the economics / trading servers.

Experiments:

- `pi` — estimate π by sampling the unit square / quarter circle
- `option` — European call/put price under geometric Brownian motion, with the
  **Black-Scholes** closed form as an analytic reference
- `queue` — M/M/1 queue simulation (mean wait, time-in-system, utilisation) vs
  the analytic steady-state formulas
- `integrate` — Monte Carlo integral of a built-in function over `[a, b]`

Deterministic via a seeded SplitMix64 PRNG with Box-Muller normals.

## HTTP

- `GET /healthz`, `GET /metrics`
- `POST /simulate`

```bash
curl -s localhost:8134/simulate -H 'content-type: application/json' -d '{
  "experiment": "option", "samples": 500000,
  "spot": 100, "strike": 100, "rate": 0.05, "volatility": 0.2,
  "maturity": 1.0, "optionType": "call"
}'
```

## NATS (subjects in `remote/libs/nats/subject-defs`)

| Env | Default subject | Meaning |
| --- | --- | --- |
| `MONTE_CARLO_SIMULATE_SUBJECT` | `dd.remote.montecarlo.simulate.requests` | inbound requests (queue group `dd-monte-carlo-server`) |
| `MONTE_CARLO_RESULT_SUBJECT` | `dd.remote.montecarlo.simulate.results` | published `montecarlo.simulate.v1` results |
| `MONTE_CARLO_EVENT_SUBJECT` | `dd.remote.events` | runtime events |

`PORT` defaults to `8134`. Set `NATS_URL` to enable the request/result lane.

## Limits & hardening

Inflight-concurrency cap (`MONTE_CARLO_MAX_INFLIGHT`, default 16); HTTP returns `503` when saturated, NATS applies backpressure. `samples` is clamped to 20 000 000 per request. Option inputs are bounded (`spot`/`strike ≤ 1e12`, `volatility ≤ 5`, `maturity ≤ 100`, `|rate| ≤ 1`) so the GBM exponent stays in a range where `exp()` is finite, and any residual non-finite result is reported as `0` with a warning rather than serialised as `null`. The M/M/1 `L`/`Lq` figures are derived from the measured wait via Little's law.
