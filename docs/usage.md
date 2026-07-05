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
| `TOR_LISTEN`        | relay  | `0.0.0.0:9001`     | Relay listen address                      |
| `TOR_KEY_FILE`      | relay  | `./relay.key`      | Static identity key file                  |
| `TOR_EXIT_ALLOW_PRIVATE` | relay | `0`            | Allow exits to private/loopback ranges    |
| `TOR_RELAY_PEERS`   | relay  | (any)              | Comma-separated `host:port` extend allowlist |
| `TOR_MAX_CIRCUITS`  | relay  | `1024`             | Max concurrent circuits before rejecting  |
| `TOR_SOCKS_LISTEN`  | client | `127.0.0.1:9050`   | Local SOCKS5 listen address               |
| `TOR_UI_LISTEN`     | client | `127.0.0.1:9060`   | Dashboard/docs listen address             |
| `TOR_DIRECTORY`     | client | (required)         | Path to the relay directory TOML          |
| `TOR_HOPS`          | client | `3`                | Relays per circuit                        |
| `TOR_DOCS_DIR`      | client | `./docs`           | Directory of markdown docs to serve       |
| `RUST_LOG`          | all    | `info`             | Log filter (`tracing` env-filter syntax)  |
