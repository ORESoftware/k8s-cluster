# Architecture

## Decision

Use Rust for the companion process rather than Java or Kotlin. Air is not an IntelliJ Platform IDE, so an IntelliJ plugin cannot be loaded into it today. Rust keeps the integration aligned with `zed-cli`, makes a small cross-platform executable, and supports Air through its documented MCP surface.

## Components

### `zed-air` CLI

A local executable with two modes:

- `inspect` / `doctor`: deterministic human or JSON output
- `mcp`: newline-delimited JSON-RPC over stdio

### Scanner

The scanner searches upward from the requested path for `.zpkg.toml` or `.zpkg.lock`, then reads only:

- `.zpkg.toml`
- `.zpkg.lock`
- existence of `zed_modules/`
- metadata timestamps
- `zed --version`

It does not inspect credentials or execute mutating package commands.

### Diagnostic model

Every finding contains:

- stable code
- severity
- user-facing explanation
- safe resolution guidance

Every recommended action contains:

- stable action ID
- optional command
- rationale
- safety class

### Future authoritative backend

The filesystem scanner is an MVP. The preferred long-term contract is a new `zed doctor --json` command in `zed-cli`, with schemas hosted in `zed-interfaces`. `zed-air` should then become a thin transport/UI adapter over that shared contract.

## Native plugin future

Create a `native-air-adapter/` module only when all of the following are true:

1. JetBrains publishes an Air extension SDK.
2. Third-party installation/distribution is documented.
3. Version compatibility and signing requirements are documented.
4. Air exposes a supported diagnostics/tool-window/action surface.

Until then, do not create an IntelliJ plugin and label it as Air-compatible.
