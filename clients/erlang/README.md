# Erlang client seed - `dd-rust-network-mutex`

Dependency-free Erlang protocol mirror for the broker wire format. It keeps
typed atoms for request/response discriminators and small JSONL request builders
for lock, composite lock, unlock, auth, version, RW, `lockInfo`, `ls`, and
heartbeat frames.

## Run

```bash
make test
```

The test is offline: no broker is required.

