# `dd_cli_config_client`

Shared Gleam/Erlang config reader for deployment boot flags.

Each deployment owns a `.cli-flags.toml` file using the
`ORESoftware/flags-2-env` schema. On boot, call `load_once()`. One local BEAM
owner process reconciles the current process environment with command-line
flags and writes the snapshot into `persistent_term`; all other actors read
through the typed Gleam helpers or the Erlang FFI.

Precedence is:

```text
CLI flag > environment variable > .cli-flags.toml default
```

The parser intentionally covers the deployment subset of the flags2env schema:
`env`, `aliases`, `short`, `type`, `default`, `true_aliases`, and
`false_aliases`.
