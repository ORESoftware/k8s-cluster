# Agent instructions

## Product boundary

`zed-air` is a read-only diagnostic bridge for JetBrains Air. Do not move package resolution, installation, publishing, authentication, or store ownership out of `zed-cli`.

## Safety

- Inspection tools must never mutate the workspace.
- Recommended commands must declare whether they are read-only, mutate the project, or install software.
- Never execute a recommended command from inside the MCP server.
- Avoid reading secrets, credential files, or environment-variable values.
- Write logs only to stderr while serving MCP; stdout is reserved for newline-delimited JSON-RPC messages.

## Compatibility

- Keep stable diagnostic codes backward-compatible.
- Negotiate MCP protocol versions and remain compatible with the latest stable version supported by JetBrains Air.
- Treat the 2026 Air native-plugin surface as unavailable until JetBrains publishes an official SDK and compatibility policy.

## Validation

Run:

```sh
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
```
