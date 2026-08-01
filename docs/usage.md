# Usage

## Build

```sh
cargo build --release
```

## Run a local 3-relay overlay

```sh
# 1. One keypair per relay (prints the base64 public key).
TOR_KEY_FILE=./relayA.key cargo run -- keygen
TOR_KEY_FILE=./relayB.key cargo run -- keygen
TOR_KEY_FILE=./relayC.key cargo run -- keygen

# 2. Fill the (addr, pubkey) triples into a directory file.
cp directory.example.toml directory.toml   # then paste the pubkeys

# 3. Start the relays.
TOR_LISTEN=127.0.0.1:9101 TOR_KEY_FILE=./relayA.key cargo run -- relay &
TOR_LISTEN=127.0.0.1:9102 TOR_KEY_FILE=./relayB.key cargo run -- relay &
TOR_LISTEN=127.0.0.1:9103 TOR_KEY_FILE=./relayC.key cargo run -- relay &

# 4. Start the client: SOCKS5 on :9050, dashboard on :9060.
TOR_DIRECTORY=./directory.toml TOR_HOPS=3 cargo run -- client &
```

## Use the real Tor network instead

Build with the `arti` feature and flip the backend — no relays or directory
needed (Tor's directory authorities provide the consensus):

```sh
cargo build --release --features arti
TOR_BACKEND=arti cargo run --release --features arti -- client
curl -x socks5h://127.0.0.1:9050 https://check.torproject.org/api/ip   # {"IsTor":true,…}
```

To use an Arti bridge/pluggable transport, point `TOR_ARTI_CONFIG` at an Arti
client TOML and make the configured transport executable available:

```sh
TOR_BACKEND=arti TOR_ARTI_CONFIG=./arti-client.toml \
  cargo run --release --features arti -- client
```

## Send traffic through it

```sh
# curl — socks5h resolves DNS at the exit (recommended for anonymity).
curl -x socks5h://127.0.0.1:9050 https://example.com/

# Chromium
chromium --proxy-server=socks5://127.0.0.1:9050

# Firefox: Settings → Network → Manual SOCKS v5, host 127.0.0.1 port 9050,
# and enable "Proxy DNS when using SOCKS v5". Or load http://127.0.0.1:9060/proxy.pac
```

## Dashboard

Open <http://127.0.0.1:9060/>. It shows live circuit counters, lets you fetch an
`http://` URL through a fresh circuit (handy for confirming your exit IP), links
to a `proxy.pac`, and serves these docs at `/docs`.

## Configuration

| Env var             | Mode   | Default            | Meaning                                   |
| ------------------- | ------ | ------------------ | ----------------------------------------- |
| `TOR_ROLE`          | all    | (argv[1])          | `relay` \| `client` \| `keygen`           |
| `TOR_BACKEND`       | client | `overlay`          | `overlay` (own relays) \| `arti` (real Tor, needs `--features arti`) |
| `TOR_NETWORK_SECRET`| all    | (empty = open)     | Overlay pre-shared key folded into handshakes |
| `TOR_ALLOW_OPEN_RELAY` | relay | `0`              | Explicitly permit a public bind without overlay PSK |
| `TOR_LISTEN`        | relay  | `0.0.0.0:9001`     | Relay listen address                      |
| `TOR_KEY_FILE`      | relay  | `./relay.key`      | Static identity key file                  |
| `TOR_EXIT_ALLOW_PRIVATE` | relay | `0`            | Allow exits to private/loopback ranges    |
| `TOR_EXIT_DENY_PORTS` | relay | `25`                | Comma-separated outbound port denylist    |
| `TOR_RELAY_PEERS`   | relay  | (any)              | Comma-separated `host:port` extend allowlist |
| `TOR_MAX_CIRCUITS`  | relay  | `1024`             | Max concurrent circuits before rejecting  |
| `TOR_CIRCUIT_IDLE_TIMEOUT_SECS` | relay | `0` (off) | Close circuits idle for this long         |
| `TOR_NETWORK_SECRET_FILE` | all | (unset)           | Read the overlay PSK from a file instead of env |
| `TOR_UI_TOKEN` / `_FILE` | client | (unset)         | Require this token for `/api/fetch`       |
| `TOR_SOCKS_LISTEN`  | client | `127.0.0.1:9050`   | Local SOCKS5 listen address               |
| `TOR_SOCKS_ALLOW_REMOTE` | client | `0`             | Permit remote bind; requires a password   |
| `TOR_SOCKS_USERNAME` | client | `tor`              | RFC 1929 username                         |
| `TOR_SOCKS_PASSWORD` / `_FILE` | client | (unset)  | RFC 1929 password                         |
| `TOR_MAX_SOCKS_CONNECTIONS` | client | `256`       | Concurrent SOCKS connection cap           |
| `TOR_UI_LISTEN`     | client | `127.0.0.1:9060`   | Dashboard/docs listen address             |
| `TOR_ARTI_CONFIG`   | client | (defaults)         | Arti client TOML for bridges/transports   |
| `TOR_DIRECTORY`     | client | (required)         | Path to the relay directory TOML          |
| `TOR_HOPS`          | client | `3`                | Relays per circuit                        |
| `TOR_DOCS_DIR`      | client | `./docs`           | Directory of markdown docs to serve       |
| `RUST_LOG`          | all    | `info`             | Log filter (`tracing` env-filter syntax)  |
