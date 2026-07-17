# `dd-quantum-compute-rs`

A **state-vector quantum simulator** over HTTP and NATS. There is **no quantum
hardware** here — it classically simulates a register of qubits as a vector of
`2ⁿ` complex amplitudes and runs circuits plus a few textbook quantum algorithms
end-to-end, returning the **measurement distribution** and each mode's answer.
Slots beside the monte-carlo / evolution / sat-smt / func-approx servers.

Pure-Rust math (its own `Complex`, no `num-complex`), deterministic from a
seeded SplitMix64 PRNG, so every run — measurement sampling and the variational
optimiser — is reproducible from `seed`.

## Modes

- `circuit` *(default)* — apply an arbitrary **gate list** to |0…0⟩ and sample
  measurements. Supported gates: `i h x y z s sdg t tdg rx ry rz p/phase`
  (single-qubit, optionally controlled), `cx/cnot cy cz crx cry crz cphase`
  (two-qubit), `swap`, and `ccx/toffoli`. `barrier`/`measure`/`reset` are no-ops.
- `grover` — **amplitude amplification**. Given a set of marked basis states it
  runs the analytically optimal ⌊(π/4)·√(N/M)⌋ rounds (override with
  `iterations`) and reports the amplified item and total success probability.
- `qaoa` — the **Quantum Approximate Optimization Algorithm** for weighted
  **MaxCut**. A `p`-layer ansatz whose (γ, β) angles are tuned by a gradient-free
  classical optimiser; returns the best sampled cut, the expected cut, the exact
  optimum (brute-forced on small graphs) and the approximation ratio.
- `vqe` — the **Variational Quantum Eigensolver**. A hardware-efficient ansatz
  (alternating RY layers + a linear CX entangler) is optimised to minimise ⟨H⟩
  for a **Pauli-sum Hamiltonian**; returns the variational ground-state energy,
  the most-probable bitstring, and — on small registers — the **exact** ground
  energy from a shifted power iteration, for reference.

If `mode` is omitted (or `auto`), it is inferred from the payload: a
`hamiltonian` ⇒ `vqe`, a `graph` ⇒ `qaoa`, a `marked` list ⇒ `grover`, else
`circuit`.

## HTTP

- `GET /healthz`, `GET /metrics`
- `POST /solve` (alias `POST /simulate`)

```bash
# Bell state (|00> + |11>)/√2
curl -s localhost:8140/solve -H 'content-type: application/json' -d '{
  "mode": "circuit", "qubits": 2, "shots": 1000,
  "gates": [ {"gate":"h","target":0}, {"gate":"cx","control":0,"target":1} ]
}'

# Grover: find |101> among 3 qubits
curl -s localhost:8140/solve -H 'content-type: application/json' -d '{
  "mode": "grover", "qubits": 3, "marked": [5]
}'

# QAOA MaxCut on a triangle (optimal cut = 2)
curl -s localhost:8140/solve -H 'content-type: application/json' -d '{
  "graph": { "edges": [[0,1],[1,2],[0,2]] }, "layers": 2, "seed": 11
}'

# VQE: ground energy of H = Z0·Z1  (= -1)
curl -s localhost:8140/solve -H 'content-type: application/json' -d '{
  "mode": "vqe", "hamiltonian": [ {"coeff": 1.0, "pauli": "ZZ"} ], "layers": 2
}'
```

Response highlights: `topOutcomes` (basis state `bitstring`, `index`,
`probability`, sampled `count`), `amplitudes` (the full state vector, included
only for ≤ 512 amplitudes), `stateNorm`; per mode — Grover's `iterations` /
`successProbability`; QAOA / VQE's `objective` (cut value or energy),
`expectedObjective`, `optimalObjective`, `approximationRatio` /
`exactGroundEnergy`, `bestBitstring`, and the optimised `parameters`; plus
`durationMs` and any `warnings`.

### Input shapes

Qubits are **little-endian**: qubit 0 is the least-significant bit, and a
`bitstring` is printed most-significant qubit first (`|q_{n-1}…q_0⟩`). A gate's
qubits may be given as `target`/`targets`, `control`/`controls`, or `qubits`,
and a rotation angle as `theta`/`angle`/`param`/`params[0]`, whichever is
terser. Graph edges accept `[u,v]`, `[u,v,weight]`, or `{u,v,weight}`.
Hamiltonian terms accept a dense Pauli string (`"IZXY"`, position *i* acts on
qubit *i*) or sparse `ops` (`[[0,"Z"],[1,"Z"]]` or `[{"qubit":0,"pauli":"Z"}]`).
`qubits` overrides the inferred register size; `layers` sets the QAOA/VQE depth.

## NATS (subjects in `remote/libs/nats/subject-defs`)

| Env | Default subject | Meaning |
| --- | --- | --- |
| `QUANTUM_SOLVE_SUBJECT` | `dd.remote.quantum.solve.requests` | inbound requests (queue group `dd-quantum-compute-rs`) |
| `QUANTUM_RESULT_SUBJECT` | `dd.remote.quantum.solve.results` | published `quantum.solve.v1` results |
| `QUANTUM_EVENT_SUBJECT` | `dd.remote.events` | runtime events |

`PORT` defaults to `8140`. Set `NATS_URL` to enable the request/result lane.
The request payload is the same JSON as `POST /solve`.

## Limits & hardening

**Wall-clock budget.** The variational modes (QAOA, VQE) run their classical
optimiser under a server-side deadline (`maxSolveMs`, default `20000`, clamped
`[500, 120000]`) *and* a bounded objective-evaluation count (`maxEvals`, default
`2000`, capped `200000`). The optimiser checks both between evaluations, so no
single request can pin a core regardless of `layers`; on the deadline it returns
the best parameters found so far with a `warning`.

**Register size.** State-vector memory is `2ⁿ` × 16 bytes, so qubits are capped
per mode: `circuit`/`grover` ≤ 20 (16 MiB), `qaoa` ≤ 16, `vqe` ≤ 14 (the
optimiser evaluates the circuit thousands of times). The exact reference solves
(QAOA brute-force optimum, VQE power iteration) run only up to 2²⁰ / 2¹²
amplitudes respectively. Circuits are capped at 100 000 gates and `shots` at
1 000 000 (default 1024); the 8 MiB body limit usually binds first.

**Concurrency.** Inflight cap (`QUANTUM_MAX_INFLIGHT`, default 4); HTTP returns
`503` when saturated, NATS applies backpressure (and redelivers).

**Numerical safety.** Amplitudes are renormalised against floating-point drift,
and every probability / amplitude / parameter is sanitised before serialisation
so an unstable run never emits JSON `null` inside a number array. Results larger
than ~900 KiB are not published to NATS (its default `max_payload`).

## Authentication

Optional and **off by default** (matching the sibling compute services). Set
`QUANTUM_AUTH_SECRET` (or the shared `SERVER_AUTH_SECRET`) to require callers of
`/solve` to present a matching `x-server-auth: <secret>` (or `auth: <secret>`)
header; the comparison is constant-time. `/healthz` and `/metrics` stay open for
probes and Prometheus. Rejections return `401` and increment
`dd_quantum_auth_failures_total`. The deployment manifest wires
`QUANTUM_AUTH_SECRET` from `dd-agent-secrets` with `optional: true`.

## Layout

| File | Role |
| --- | --- |
| `src/complex.rs` | minimal complex arithmetic |
| `src/rng.rs` | seeded SplitMix64 PRNG |
| `src/state.rs` | dense state-vector simulator: gate kernels, measurement, expectations |
| `src/gates.rs` | gate vocabulary, JSON gate parsing, circuit application |
| `src/algorithms.rs` | Grover, QAOA MaxCut, VQE, the exact reference solves, the gradient-free optimiser |
| `src/run.rs` | request/response contract, bounds, mode dispatch |
| `src/main.rs` | axum HTTP + NATS server wiring, metrics, auth, runtime-config |

Run the tests with `cargo test` (covers gate correctness, a Bell state, Grover
amplification, QAOA MaxCut on a triangle, and VQE ground energies of `Z`, `X`,
and `Z⊗Z`).

> **ORM policy:** prefer **SeaORM** over sqlx for new database code (MASH stack: maud, axum, SeaORM, supabase, htmx).
