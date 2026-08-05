# DEN-2526 validation evidence

The durable Daedalus fabrication execution/outbox schema and every generated
adapter were validated by the secure cross-organization workflow:

- run: https://github.com/daedalus-fab/fabrication-server.rs/actions/runs/31049835500
- PostgreSQL: 17
- generated adapters: 748 total outputs checked
- contract tests: 100 total, 93 passed, 7 intentionally skipped, 0 failed
- complete schema apply: successful
- declarative diff after apply: zero drift
- per-database schemas and Supabase RLS checks: successful
- DPM convergence verification: successful

The ORESoftware-hosted workflow was unable to start because hosted Actions
billing was blocked, so the identical repository generator and PostgreSQL/DPM
checks ran on the daedalus-fab hosted runner. No credential is stored in this
repository or in the validation artifact.
