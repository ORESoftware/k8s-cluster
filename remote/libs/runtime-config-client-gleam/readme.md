# `dd_runtime_config_client` (Gleam)

Shared receiver helper for the dd-runtime-config control plane. The Rust
sibling lives at `remote/libs/runtime-config-client-rs/`. Schema is shared
through `remote/libs/interfaces/shared/schema/runtime-config.schema.json`.

Two files:

- `src/dd_runtime_config_client.gleam` — public Gleam API (one `start_*`
  call plus three `handle_*` functions returning `mist.ResponseData`
  responses).
- `src/dd_runtime_config_client_ffi.erl` — Erlang FFI that does the actual
  work: `persistent_term` snapshot storage, raw `gen_tcp` HTTP POST for
  registration, exponential-backoff retry loop.

Apply/reset routes require `X-Server-Auth` to match
`RUNTIME_CONFIG_SERVER_SECRET`. If the secret is missing, the helper fails
closed unless `RUNTIME_CONFIG_ALLOW_UNAUTHENTICATED=true` is set for a local
smoke test. Older snapshot generations are acknowledged but ignored so a slow
cron push cannot overwrite newer config already applied to the process.

## Wiring a Gleam service

In `gleam.toml`:

```toml
[dependencies]
dd_runtime_config_client = { path = "../../libs/runtime-config-client-gleam" }
```

In the service's `http_server.gleam`, add three arms to the existing route
pattern match (whichever style it uses — path-only or `method, path`):

```gleam
import dd_runtime_config_client

// inside `route/1`:
case request.path_segments(req) {
  // ...existing arms...
  ["internal", "runtime-config"]          -> dd_runtime_config_client.handle_snapshot(req)
  ["internal", "update-runtime-config"]   -> dd_runtime_config_client.handle_apply(req)
  ["internal", "runtime-config", "reset"] -> dd_runtime_config_client.handle_reset(req)
  _                                       -> not_found()
}
```

In the service's `main` (after the HTTP supervisor starts):

```gleam
let _ = dd_runtime_config_client.start_registration_loop()
```

And on the deployment yaml, the same six env vars every other subscriber
gets (see `remote/deployments/runtime-config-rs/readme.md`).
