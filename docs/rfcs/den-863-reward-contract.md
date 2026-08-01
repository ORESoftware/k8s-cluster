# DEN-863: versioned soccer reward and penalty contract

## Boundary

The committed `formal/reward-contract/reward_contract.json` is a reviewable
version-1 reward schema and safety sentinel. It makes the sign convention
explicit: penalties are negative rewards and cost is exactly `-reward`.

The contract separates:

- terminal match outcomes (`goal_for`, `goal_against`);
- signed shaping deltas such as xG, possession value, progression, and defensive
  integrity;
- event penalties for fouls, offsides, and turnovers;
- a safety penalty for infeasible actions.

## Anti-specification-gaming checks

`verify.py` rejects contracts where:

- component IDs are duplicated or bounds/weights are non-finite;
- a reward can become negative or a penalty can become positive;
- goal-for and goal-against are not symmetric;
- the sum of every non-terminal shaping/penalty extreme can overpower the
  terminal goal reward plus the configured margin;
- an infeasible action can produce positive reward;
- runtime values are unknown or outside their declared range.

For the initial contract, the verifier enumerates all 256 corners of the eight
non-terminal component ranges. The terminal reward magnitude is 100, while the
maximum absolute non-terminal envelope is 18, leaving an actual dominance
margin of 82.

## Commands

```bash
python3 formal/reward-contract/verify.py
python3 -m unittest discover -s formal/reward-contract -p 'test_*.py'
```

## Engine integration

The Rust engine should deserialize this schema into a versioned config, emit the
component breakdown and aggregate reward for every transition, and persist the
contract digest in experiment manifests. Any clipping or normalization must
occur after the raw component breakdown is recorded. Held-out win rate and
exploitability remain the deciding evaluation metrics; this contract does not
make shaped reward a substitute for match outcomes.
