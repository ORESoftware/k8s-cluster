# Java client — `dd-rust-network-mutex`

Zero-dependency Java 17 client. Speaks the same JSON wire protocol as every other
client here (see [`../../PROTOCOL.md`](../../PROTOCOL.md)). JSON is hand-rolled in
`Json.java` so the client has **no** transitive dependencies.

The `type` discriminator is a Java `enum` (`Protocol.RequestType` /
`Protocol.ResponseType`) with a `switch` over every variant. Lock handles are
`record`s. 64-bit fencing tokens are parsed as `long` (never a `double`).

## Layout

| File | Purpose |
|------|---------|
| `src/main/java/.../Json.java`               | minimal JSON parser/serializer |
| `src/main/java/.../Protocol.java`           | enums + request builders + `Response` |
| `src/main/java/.../NetworkMutexClient.java` | multiplexed TCP client (background reader thread) |
| `src/main/java/.../Smoke.java`              | end-to-end smoke (mirrors the Go/Dart smokes) |
| `src/test/java/.../ProtocolTest.java`       | offline round-trip tests (plain `main`, no JUnit) |

## Run

```bash
./build.sh                                              # compile -> ./out (plain javac)
java -cp out com.oresoftware.networkmutex.ProtocolTest  # offline protocol tests
java -cp out com.oresoftware.networkmutex.Smoke         # live smoke (broker on :6970)
```

If `javac` isn't on `PATH`, point at a JDK 17+, e.g. on macOS Homebrew:

```bash
JAVAC=/opt/homebrew/opt/openjdk@17/bin/javac ./build.sh
```

A `pom.xml` is included for Maven-based consumers, but no Maven build is required.

## Usage

```java
try (var client = NetworkMutexClient.connect("127.0.0.1", 6970)) {
  var h = client.acquire("my-key", 30_000);  // ttl ms
  try {
    // critical section; attach h.fencingToken() to downstream writes
  } finally {
    client.release(h);
  }

  var maybeH = client.tryAcquireMany(List.of("a", "b"), 30_000);
  maybeH.ifPresent(client::release);
}
```
