# DEN-862: deterministic nearby-state and symmetry fixtures

## Purpose

`formal/exploration-neighborhood/model.py` creates a replayable, content-addressed
fixture neighborhood for the plateau/generalization work in DEN-862 and the
metrics work in DEN-103. It is deliberately independent from the production
policy so fixture semantics can be reviewed before a Rust adapter is wired in.

The harness covers:

- bounded coordinate and velocity perturbations;
- left/right field mirroring with directional action remapping;
- equivalent left/right role permutations that preserve entity geometry;
- deterministic seed replay and a SHA-256 fixture digest;
- a reference action ranking used only to prove mirror equivariance and fixture
  integrity.

It does **not** claim that the production soccer policy is symmetric or robust.
The next adapter must pass each generated scenario to the real selector and
record action rank, value gap, behavior probability, outcome, and symmetry
violation. This sentinel establishes the stable input and comparison contract.

## Commands

```bash
python3 formal/exploration-neighborhood/model.py
python3 -m unittest discover -s formal/exploration-neighborhood -p 'test_*.py'
python3 formal/exploration-neighborhood/model.py \
  --seed 20260801 --samples 128 --json out/exploration-neighborhood.json
```

## Required production report

For every base/perturbed/mirrored/permuted fixture, the production adapter should
emit:

- repository and environment version;
- fixture digest and scenario ID;
- selected and executed action/rank;
- score/value gap and calibrated uncertainty;
- behavior probability or an explicit `unknown-after-override` marker;
- mirror/permutation action mapping;
- policy continuity and action-regret metrics;
- deterministic replay result.

This is additive to the existing top-k/Boltzmann selector; it does not introduce
a competing selector.
