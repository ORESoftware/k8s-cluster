# Python client — `dd-rust-network-mutex`

Zero-dependency (stdlib-only) Python 3.8+ client that speaks the same JSON wire
protocol as every other client here (see [`../../PROTOCOL.md`](../../PROTOCOL.md)).

The `type` discriminator is a real `enum.Enum` (`RequestType` / `ResponseType`)
with typed builder functions, so a typo is an `AttributeError` at author time
rather than a silently-misrouted magic string.

## Layout

| File | Purpose |
|------|---------|
| `network_mutex/protocol.py` | enums + request builders + `Response` parser |
| `network_mutex/client.py`   | multiplexed TCP/UDS client (background reader thread, per-uuid queues) |
| `smoke.py`                  | end-to-end smoke (mirrors the Go/Dart smokes) |
| `tests/test_protocol.py`    | offline round-trip tests (no broker needed) |

## Run

```bash
# offline protocol tests (no broker)
python3 -m unittest discover -s tests -p 'test_*.py'

# live smoke (start a broker first; defaults to 127.0.0.1:6970)
python3 smoke.py
```

## Usage

```python
from network_mutex import NetworkMutexClient

with NetworkMutexClient.connect("127.0.0.1", 6970) as client:
    handle = client.acquire("my-key", ttl_ms=30_000)
    try:
        ...  # critical section; attach handle.fencing_token to downstream writes
    finally:
        client.release(handle)

    maybe_handle = client.try_acquire_many(["a", "b"], ttl_ms=30_000)
    if maybe_handle is not None:
        client.release(maybe_handle)
```
