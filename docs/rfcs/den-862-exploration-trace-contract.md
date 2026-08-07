# DEN-862: replay-safe exploration and executed-action likelihood

The hermetic `exploration_contract` binary defines the minimum contract that the
existing soccer selector should expose without replacing its tactical scoring.

It provides:

- named/versioned profiles;
- explicit training, adaptation, validation, and production modes;
- greedy, epsilon-greedy, rank-weighted, Boltzmann, and uncertainty-directed
  distributions;
- deterministic seed + decision-nonce replay;
- selected action/rank/probability/log-probability;
- executed action/rank reconciliation after downstream feasibility or MPC
  validation;
- an explicit `unknown_after_downstream_override` likelihood status rather than
  assigning the promoted candidate's probability to a different executed
  action.

The binary is intentionally dependency-free and lives in the isolated 5v5 crate
so CI can compile and test the contract even when parent workspace dependencies
are unavailable.

```bash
cargo test --manifest-path standalone-5v5/Cargo.toml --bin exploration_contract
cargo run --quiet --manifest-path standalone-5v5/Cargo.toml --bin exploration_contract
```

The next production integration should adapt the existing selector output into
this trace shape and record the downstream mapping explicitly. On-policy
training must reject or separately model transitions whose executed-action
likelihood is unknown after an override.
