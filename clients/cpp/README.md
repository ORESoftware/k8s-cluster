# C++ client — `dd-rust-network-mutex`

Header-only, dependency-free C++17 client. Speaks the same JSON wire protocol as
every other client here (see [`../../PROTOCOL.md`](../../PROTOCOL.md)). JSON is a
small hand-rolled parser/serializer (`json.hpp`) so there is nothing to vendor.

The `type` discriminator is an `enum class` (`RequestType` / `ResponseType`) with
a `switch` over every variant — no bare-string `if (data.type == "...")` chains.
64-bit fencing tokens are preserved exactly (numbers are kept as text and parsed
on demand, never round-tripped through a `double`).

## Layout

| File | Purpose |
|------|---------|
| `include/network_mutex/json.hpp`     | minimal JSON value + parser + serializer |
| `include/network_mutex/protocol.hpp` | enums + request builders + `Response` parser |
| `include/network_mutex/client.hpp`   | multiplexed TCP client (background reader thread) |
| `smoke.cpp`        | end-to-end smoke (mirrors the Go/Dart smokes) |
| `test_protocol.cpp`| offline round-trip tests (no broker needed) |

## Run

```bash
make test    # offline protocol tests (no broker)
make run     # build + run the live smoke (start a broker first; 127.0.0.1:6970)
```

Requires a C++17 compiler (`clang++` or `g++`). `cmake` is **not** required — the
`Makefile` compiles the header-only client directly.

## Usage

```cpp
#include "network_mutex/client.hpp"

auto client = nm::Client::connect("127.0.0.1", 6970);
auto h = client->acquire("my-key", /*ttl_ms=*/30000);
// ... critical section; attach h.fencing_token to downstream writes ...
client->release(h);

auto maybe_h = client->try_acquire_many({"a", "b"}, /*ttl_ms=*/30000);
if (maybe_h) client->release(*maybe_h);
```
