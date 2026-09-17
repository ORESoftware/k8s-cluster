# Clarabel as an LP and MIP-relaxation backend

## Recommendation

Clarabel would be a useful optional LP/interior-point backend for this service.
It should not be described as a native integer-programming or
mixed-integer-programming solver.

In this document:

- **IP** means integer programming.
- **IPM** means interior-point method.

Clarabel solves continuous convex conic problems. Its documented problem format
uses continuous variables in \(\mathbb{R}^n\), and its supported problem classes
include LP, convex QP, SOCP, and SDP. It does not provide an integer-variable
branch-and-bound layer. See the
[Clarabel Rust documentation](https://docs.rs/clarabel/latest/clarabel/).

The practical fit is therefore:

| Workload | Fit | Clarabel's role |
| --- | --- | --- |
| LP | Strong | Solve the complete continuous problem |
| IP | Indirect | Solve continuous relaxations inside this service's branch-and-bound |
| MIP | Indirect | Solve continuous relaxations while this service enforces integrality |
| Convex QP/SOCP | Strong future option | Requires model/API support beyond the current linear schema |

A MIP result using this backend should be identified as something like
`des-bnb+clarabel-relaxations`, rather than implying that Clarabel itself is the
MIP solver.

## Why it complements the current solvers

The native interior-point implementation in DES is dense, dependency-free, and
intended for modest LPs. Clarabel supplies a mature sparse primal-dual
interior-point implementation with presolve, equilibration, regularization, and
infeasibility detection.

This makes Clarabel most attractive for:

- pure LP requests;
- large, sparse, cold root relaxations;
- models on which the native dense IPM is slow or numerically fragile;
- an in-process alternative to invoking an external LP solver.

It is less obviously attractive for every branch-and-bound node. Descendant
nodes are closely related and typically differ by bounds or a few cuts.
Simplex or the existing incremental primal-dual implementation can exploit that
relationship more naturally. A cold IPM solve at every node may spend more time
refactorizing systems than it saves.

## Current repository state

Clarabel is already present through the `des_engine` dependency:

- DES declares `clarabel = "0.11"`.
- DES contains `solve_lp_clarabel` in
  `src/des/general/lp.rs`.
- DES has correctness tests for maximization, equalities, and variable bounds.
- Those two focused Clarabel tests pass in the current checkout.

The solver node does not currently expose that backend:

- `SolveOptions::requested_lp_algorithm` in `src/main.rs` accepts only
  `internal-simplex` and `internal-ipm`.
- `solve_lp_with_options` dispatches only to those two implementations.
- `ConcreteLpRelaxationAlgorithm` in DES does not contain a Clarabel variant.
- The DES worker-side MIP relaxation dispatcher consequently cannot select
  Clarabel for branch-and-bound nodes.

The real change would therefore be to promote and harden the existing DES
adapter, then expose it through the service. It is not necessary to introduce a
completely new solver stack.

## Production-readiness gaps

The existing adapter is a useful starting point but should not be exposed as a
production MIP-relaxation backend without addressing the following points.

### Status and MIP-bound safety

The adapter currently maps Clarabel's `AlmostSolved` status to `LPStatus::Optimal`.
Clarabel's reduced-accuracy tolerances are intentionally looser than its full
accuracy tolerances. Treating a reduced-accuracy objective as an exact node
bound can make branch-and-bound pruning unsafe.

Before use in IP/MIP solves, the integration should distinguish exact and
reduced-accuracy termination, validate primal feasibility and the duality gap,
and apply conservative bound handling.

### Runtime settings and cancellation

The adapter hard-codes `max_iter = 200` and otherwise uses default tolerances. It
does not currently honor the service's LP iteration limit, solve timeout, or
other request-level controls.

Clarabel exposes iteration, time-limit, feasibility, gap, presolve, and linear
solver settings. See
[Clarabel solver settings](https://clarabel.org/stable/api_settings/).

### Result completeness

The adapter returns the primal point and objective but currently discards:

- row duals;
- reduced costs;
- infeasibility certificates;
- unbounded rays;
- solver diagnostics.

An IPM does not naturally produce a simplex basis, so basis fields can remain
unavailable. The other information should be mapped where Clarabel provides it,
especially because the pure-LP API already reports dual information.

### Sparse model construction

The adapter creates dense constraint rows, including one dense row per finite
variable bound, and only then converts them to CSC format. This can consume
quadratic temporary memory in the number of variables and weakens Clarabel's
sparse advantage.

For large sparse models, the integration should assemble CSC entries directly.
The service's own dense `Vec<Vec<f64>>` model representation may also become the
limiting factor before the solver is called.

### Default lower-bound semantics

`LPProblem` documents an absent `lb` vector as the default lower bound of zero.
The generic Clarabel adapter only emits lower-bound rows when `lb` is explicitly
present, which otherwise makes those variables free. This service currently
constructs explicit zero lower bounds, but the generic DES adapter should be
corrected before broader reuse.

### Repeated node solves

The adapter constructs a new Clarabel solver for every solve. Clarabel supports
updating problem data when dimensions and sparsity remain fixed, but updates
cannot be used with presolve or chordal decomposition enabled. See
[Clarabel problem data updates](https://clarabel.org/stable/user_guide_data_updating/).

Branch constraints normally change the model structure. Reuse would require a
fixed structural representation, such as preallocated bound rows with updated
right-hand sides. Even with structural reuse, Clarabel data updates should not
be assumed to provide simplex-style basis warm starts.

## Recommended integration strategy

1. Add an explicit `clarabel-ipm` LP algorithm rather than replacing either
   existing backend.
2. Initially allow it for pure LPs and root MIP relaxations.
3. Continue using incremental primal-dual or simplex methods for most descendant
   nodes.
4. Retain HiGHS as the independent full-MIP verification solver; Clarabel cannot
   replace it for integer verification.
5. Do not make Clarabel the default until representative benchmarks show better
   end-to-end behavior.

The first benchmark matrix should include:

- the soccer-formation LP relaxation;
- the existing 100-by-150 dispatch model;
- sparse and dense synthetic LPs over several sizes;
- binary IP, general IP, and mixed-integer cases;
- infeasible and unbounded relaxations;
- numerically scaled and nearly degenerate models.

For MIP workloads, measure total wall time, nodes per second, LP failures,
incumbent quality, best-bound validity, final gap, and total nodes explored.
Per-LP solve time alone is not sufficient: a relaxation backend can be faster
at the root while producing worse overall branch-and-bound throughput.

## Decision

Add Clarabel as an experimental, opt-in continuous LP/root-relaxation backend.
Do not market it as native IP/MIP support, and do not use it at every MIP node
until status handling, sparse construction, request settings, and representative
end-to-end benchmarks are complete.
