# live-mutex-rs — clients audit (2026-06-09)

Audited the 15-language client suite for correctness: offline protocol/parity
tests + end-to-end smokes against a live broker
(`dd-rust-network-mutex` on TCP 127.0.0.1:6970, started with default features —
TLS/OTel are runtime-optional). Method: ran each suite individually (not the
abort-on-first master script) to get a full matrix.

## Result matrix

| Language | Offline protocol test | Live smoke (vs broker) | Notes |
|---|---|---|---|
| Rust (src/client.rs) | — | covered by `cargo test` | (integration tests; not re-run here) |
| protocol parity (`check-protocol-parity.sh`) | **PASS** | n/a | all 15 langs mirror `src/protocol.rs` |
| Go | **PASS** (`go test ./...`) | **PASS** (`go run ./cmd/smoke`) | |
| Python | **PASS** (`unittest`) | **PASS** (`smoke.py`) | |
| C++ | **PASS** (`make test`) | **PASS** (`make run`) | header-only |
| Erlang | **PASS** (`make test`) | (no separate smoke) | |
| Java | **PASS** (`ProtocolTest`) | **PASS** (`Smoke`) | plain javac, no Maven |
| TypeScript | — | **PASS** (`npx tsx src/smoke.ts`) | |
| Gleam | **PASS** (`gleam test`) | **PASS** (`LIVE_MUTEX_SMOKE=1 gleam test`) | needed gleam ≥1.14 (see below) |
| Shell | n/a | **PASS** (`smoke.sh`) | |
| Elixir | blocked | blocked | no `mix`/elixir installed |
| OCaml | blocked | blocked | no `ocamlc` installed |
| C# | blocked | blocked | no `dotnet` installed |
| F# | blocked | blocked | no `dotnet` installed |
| PowerShell | n/a | blocked | no `pwsh` installed |
| Dart | blocked | blocked | no `dart` SDK (smoke can fall back to `docker run dart:stable`) |

**Every client with a locally-available toolchain passed both its offline and
live tests. No client correctness defects found.**

## Environment note (not a client bug)

The first offline run showed `gleam test` failing with *"package gleam_stdlib
requires Gleam ≥1.14.0 but you are using 1.11.1"* — a toolchain-version issue,
not a client defect. After `brew upgrade gleam`, both the offline and live Gleam
suites pass. The remaining blocked rows are purely missing toolchains
(`dotnet`, `mix`, `ocamlc`, `dart`, `pwsh`).

## How to reproduce

```bash
# terminal 1 — broker
cargo run --release --no-default-features --bin dd-rust-network-mutex
# terminal 2 — all smokes (auto-skips missing toolchains)
./scripts/run-all-client-smokes.sh
```
