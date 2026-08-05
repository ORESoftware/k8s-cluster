# zed-jetbrains-air

JetBrains Air integration for [zed-pkg](https://zpkg.tech). It gives Air agents a read-only view of the current project's Zed package state, stable diagnostics, and recommended resolutions backed by the real filesystem and installed `zed` CLI.

> **Current platform reality (August 2026):** Air is not based on the IntelliJ Platform and does not yet support third-party native plugins. This repository therefore ships an Air-native MCP/agent integration now and reserves a future native adapter for the day JetBrains publishes an Air extension SDK.

## What users get

- package identity and direct-dependency summary from `.zpkg.toml`
- lockfile and `zed_modules/` materialization state
- installed `zed` CLI availability/version
- stable diagnostics such as missing/invalid manifests, missing locks, lock-only workspaces, and stale materialization
- prioritized recommended commands with an explicit safety classification
- no hidden project mutation: inspection and MCP tools are read-only

## Architecture

```text
JetBrains Air
  ├─ Chat / agent commands
  └─ MCP client
       └─ zed-air mcp (Rust, stdio JSON-RPC)
            ├─ reads .zpkg.toml / .zpkg.lock / zed_modules
            ├─ checks zed --version
            └─ recommends authoritative zed-cli commands
```

The companion process intentionally does not reimplement dependency resolution, package installation, publishing, or store mutation. Those remain owned by `zed-cli`.

## Install and run

```sh
cargo install --path .
zed-air inspect --json
```

For Air, copy `.air/mcp.json.example` to `.air/mcp.json` in the target workspace, or merge the server entry into an existing workspace `.mcp.json`:

```json
{
  "mcpServers": {
    "zed-pkg": {
      "command": "zed-air",
      "args": ["mcp"]
    }
  }
}
```

JetBrains Air can launch workspace MCP servers after MCP support and workspace server launching are enabled in Settings.

## MCP tools

| Tool | Purpose |
| --- | --- |
| `zed_project_status` | Full project report with package state, diagnostics, and actions |
| `zed_recommended_actions` | Focused prioritized resolution list; commands are returned, never executed |
| `zed_explain_diagnostic` | Explanation for a stable diagnostic code such as `ZED007` |

## CLI

```sh
zed-air inspect [--root PATH] [--json]
zed-air doctor  [--root PATH] [--json]
zed-air mcp
```

## Diagnostic contract

Diagnostics use stable codes so Air prompts, tests, docs, and future UI surfaces do not depend on prose:

- `ZED001`: CLI unavailable
- `ZED002`: manifest missing
- `ZED003`: lockfile missing
- `ZED004`: lock-only workspace
- `ZED005`: invalid manifest
- `ZED006`: invalid lockfile
- `ZED007`: locked packages not materialized
- `ZED008`: manifest newer than lockfile
- `ZED009`: package identity incomplete

## Roadmap

1. Add a machine-readable `zed doctor --json` command to `zed-cli` and consume it here as the authoritative diagnostic backend.
2. Add file watching/debouncing so Air agents can refresh state after manifest and lockfile changes.
3. Add dependency graph and provenance tools backed by `zed-interfaces`.
4. Add safe-action policy metadata and optional Air approval prompts; keep execution outside the diagnostic server.
5. Add a native Air adapter only after JetBrains publishes a supported extension SDK.

## Extraction note

The initial implementation may live temporarily under `zed-monorepo/incubator/zed-jetbrains-air`. It is intentionally self-contained so it can be moved unchanged to the root of `zed-pkg/zed-jetbrains-air`.

## License

MIT
