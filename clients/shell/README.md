# Shell (bash/zsh/sh) client

A dependency-free client for the `dd-rust-network-mutex` broker that speaks the
newline-delimited JSON wire protocol in [`../../PROTOCOL.md`](../../PROTOCOL.md).
The only requirement is **bash** (3.2+, the default on macOS and Linux) — the
TCP transport is bash's built-in `/dev/tcp`, so there is no `nc`, `python`, or
`jq` dependency. zsh/sh users just run the scripts: the `#!/usr/bin/env bash`
shebang selects the right interpreter.

Like every other client here, the wire `type` discriminators live in named
constants (`LMX_REQ_*` / `LMX_RES_*` in `live_mutex_client.sh`) rather than being
sprinkled around as magic strings, and `../check-protocol-parity.sh` greps this
file to confirm it mirrors every variant in `src/protocol.rs`.

## Files

- `live_mutex_client.sh` — the client library: connect, acquire / try_acquire /
  release, acquire_many / release_many, the reader-writer helpers
  (acquire_read / acquire_write / release_read / release_write), plus `ls`,
  `lock_info`, and `heartbeat`.
- `smoke.sh` — end-to-end smoke test mirroring `../python/smoke.py`.

## Smoke test

```bash
# 1. start the broker (in another terminal)
cargo run --release --no-default-features --bin dd-rust-network-mutex

# 2. run the shell smoke
./clients/shell/smoke.sh
```

Override the endpoint with `LIVE_MUTEX_HOST` / `LIVE_MUTEX_PORT` (defaults
`127.0.0.1:6970`), and set `LIVE_MUTEX_TOKEN` when the broker requires auth.

## Using it from your own script

```bash
. clients/shell/live_mutex_client.sh

lmx_connect 127.0.0.1 6970
lmx_acquire "my-key" 30000          # blocks until granted
echo "held with fencing token $LMX_FENCE (handle $LMX_LOCK_UUID)"
# ... critical section ...
lmx_release "my-key" "$LMX_LOCK_UUID"
lmx_disconnect
```

`lmx_try_acquire` returns `0` when granted, `2` on contention, and `1` on error,
so a fail-fast caller can branch on the exit status without leaving a deferred
waiter behind. Each successful acquire sets `LMX_LOCK_UUID` and `LMX_FENCE`;
composite grants set `LMX_FENCES` (the raw `fencingTokens` object).
