# `rust-network-mutex-rs` clients

Fifteen language clients/seeds that all speak the same JSON wire protocol (see
`../PROTOCOL.md`) to the Rust broker. Each client mirrors the Rust
`Request` / `Response` enum in its native idiom — **no magic strings**,
unlike the upstream Node `live-mutex` library which uses `if (data.type
=== '…')` chains in `broker.js`.

| Runtime    | Path                | Discriminator construct                       | Check / smoke command                                        |
|------------|---------------------|-----------------------------------------------|--------------------------------------------------------------|
| Rust       | `../src/client.rs`  | serde tagged enum (`#[serde(tag="type")]`)    | `cargo test --no-default-features` (in deployment root)      |
| TypeScript | `ts/`               | discriminated union (`type Request = … \| …`) | `pnpm --dir clients/ts smoke`                                |
| Go         | `go/`               | typed `RequestType` const block + switch      | `go run ./clients/go/cmd/smoke`                              |
| Dart       | `dart/`             | `sealed class Request` + pattern matching     | `dart run clients/dart/bin/smoke.dart`                       |
| Gleam      | `gleam/`            | `pub type Request { … }` (true ADT)           | `LIVE_MUTEX_SMOKE=1 gleam test` (in `clients/gleam/`)        |
| Python     | `python/`           | `enum.Enum` `RequestType` + typed builders    | `python3 clients/python/smoke.py`                            |
| C++        | `cpp/`              | `enum class RequestType` + `switch`           | `make -C clients/cpp run`                                    |
| Java       | `java/`             | `enum RequestType` + `switch` (records)       | `clients/java/build.sh && java -cp clients/java/out com.oresoftware.networkmutex.Smoke` |
| Erlang     | `erlang/`           | atoms + function clauses                      | `make -C clients/erlang test`                                |
| Elixir     | `elixir/`           | atoms + pattern-matched function heads        | `mix test` (in `clients/elixir/`)                            |
| OCaml      | `ocaml/`            | variants + pattern matching                   | `make -C clients/ocaml test`                                 |
| C#         | `csharp/`           | `enum RequestType` / `enum ResponseType`      | `dotnet run --project clients/csharp`                        |
| F#         | `fsharp/`           | discriminated unions + pattern matching       | `dotnet run --project clients/fsharp`                        |
| Shell      | `shell/`            | `LMX_REQ_*` / `LMX_RES_*` bash constants      | `./clients/shell/smoke.sh`                                   |
| PowerShell | `powershell/`       | `$LmxReq` / `$LmxRes` constant tables         | `pwsh ./clients/powershell/smoke.ps1`                        |

The TypeScript client also ships a head-to-head benchmark harness
(`ts/src/compare.ts`) that runs the same workload against
`oresoftware/live-mutex` (the upstream Node broker) and our Rust broker
in the same Node process and prints a side-by-side throughput / latency
report. Sample local result on M-class darwin (`WORKERS=8 KEYS=4
DURATION_MS=2000`):

```
ours      total= 102775  throughput=   51388 ops/s  avg=   0.16ms  max=   1.35ms  errors=0
theirs    total=  76411  throughput=   38206 ops/s  avg=   0.21ms  max=   3.69ms  errors=0
[compare] ratio (ours / theirs) = 1.35x
```

## Wait / No-Wait Acquire

All exclusive-lock clients now expose both blocking acquire and fail-fast
try-lock behavior for single-key and composite locks. Blocking acquire sends
`wait:true` and keeps the request registered through the queued
`acquired:false` notice until the final `acquired:true` grant. Try-lock helpers
send `wait:false`, read exactly one response, and return no handle on
contention, so failed attempts cannot leave a deferred waiter behind.

## Run every smoke test

```bash
# offline protocol discriminator parity (no broker)
./clients/check-protocol-parity.sh
```

```bash
# 1. start the broker (in another terminal)
cargo run --release --no-default-features --bin dd-rust-network-mutex

# 2. run every client smoke
./scripts/run-all-client-smokes.sh
```

The script auto-falls back to running the Dart smoke inside `dart:stable`
in Docker if no local Dart SDK is installed.

## Why an enum (instead of magic strings)?

Adding a new request variant on the broker side (in `src/protocol.rs`) must be
reflected in every client mirror. Static languages catch much of that through
exhaustiveness checks; `./clients/check-protocol-parity.sh` catches the shared
wire discriminator drift for every runtime:

- Rust → enum exhaustiveness is enforced by `match`.
- TypeScript → `assertNever(value: never): never` makes the `switch`
  exhaustiveness-checked by `tsc`.
- Go → `switch` over `RequestType` plus `staticcheck`'s `exhaustive`
  linting rule.
- Dart → `sealed class` + Dart 3 pattern matching is exhaustive by
  construction.
- Gleam → custom types are real ADTs; non-exhaustive `case` is a compile
  error.
- Python → `enum.Enum` request/response discriminators plus typed builders.
- C++ → `enum class` request/response discriminators and switch helpers.
- Java → `enum RequestType` / `enum ResponseType` with typed request builders.
- Erlang → atoms and exhaustive function clauses over known wire tags.
- Elixir → atoms and pattern-matched function heads over known wire tags.
- OCaml → variants and pattern matching.
- C# → enums and switch expressions.
- F# → discriminated unions and pattern matching.
- Shell → `LMX_REQ_*` / `LMX_RES_*` readonly constants used to build every frame.
- PowerShell → `$LmxReq` / `$LmxRes` constant tables used to build every frame.

The upstream `live-mutex` broker switches on bare strings, so a typo
silently routes to "no handler". This is the structural fix.
