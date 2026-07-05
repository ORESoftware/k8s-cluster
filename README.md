# tor-server.rs

An onion-routing anonymizing proxy in Rust. It exposes a local **SOCKS5** port
(the same interface Tor presents to applications) and tunnels each connection
through a **telescoped multi-hop circuit** of relay nodes, with genuine
**per-hop layered encryption**. No single relay sees both who you are and where
you are going, and no relay can read the application payload destined for hops
beyond it.

This is a compact, readable implementation of the core onion-routing idea — not
a drop-in Tor client, and not interoperable with the real Tor network. It is
meant for running your own private overlay of relays (e.g. across your own
clusters/regions) to anonymize outbound traffic.

## How it works

One binary, three modes (selected by `argv[1]` or the `TOR_ROLE` env var):

| Mode     | Role                                                                       |
| -------- | -------------------------------------------------------------------------- |
| `relay`  | An onion relay/exit node. Holds one static X25519 identity, forwards cells. |
| `client` | The local SOCKS5 proxy. Builds a fresh circuit per connection.             |
| `keygen` | Generate/persist a relay keypair and print its public key.                 |

```
  app ──SOCKS5──▶ client ──▶ relay A ──▶ relay B ──▶ relay C(exit) ──▶ destination
                    │           │           │              │
                    │  K_A      │  K_B      │  K_C         │  (plaintext to dest)
                    └── onion-wraps every payload: E_KA( E_KB( E_KC( data ) ) )
```

- **Entry (relay A)** knows the client but only that the next hop is B.
- **Middle (relay B)** knows only A and C — neither the client nor the destination.
- **Exit (relay C)** knows the destination but not the client.

### Circuit construction (telescoping)

The client handshakes with relay A directly, then asks A to `Extend` to B,
tunnelling B's handshake through the A layer; then asks B to `Extend` to C
through the A+B layers. Each hop's key is established by the client and that hop
alone.

### Per-hop handshake

An ntor-like construction (simplified, not the formally analyzed Tor ntor):

- Each relay owns a long-term X25519 static keypair (public key published in the
  directory).
- Per hop, the client sends an ephemeral public key `X`.
- Both sides derive a shared secret from two Diffie–Hellman results:
  `s1 = DH(client_eph, relay_static)` (authenticates the relay) and
  `s2 = DH(client_eph, relay_eph)` (forward secrecy), fed through HKDF-SHA256 to
  produce forward/backward keys plus an auth key.
- The relay returns its ephemeral public key and an HMAC-SHA256 over the
  transcript; the client verifies it, proving the relay holds the static secret
  matching the directory.

### Layered data encryption

Each hop has independent forward/backward keys. Payloads are sealed with
**ChaCha20-Poly1305** (AEAD), one layer per hop, with a monotonic per-direction
nonce counter (each key is used in exactly one ordered stream, so nonces never
repeat). Relays peel/add exactly one layer; the client peels all of them.

## Build

```sh
cargo build --release
```

## Run a local 3-relay overlay

```sh
# 1. Generate a keypair per relay (prints the base64 public key).
TOR_KEY_FILE=./relayA.key cargo run -- keygen
TOR_KEY_FILE=./relayB.key cargo run -- keygen
TOR_KEY_FILE=./relayC.key cargo run -- keygen

# 2. Put the three (addr, pubkey) pairs into a directory file.
cp directory.example.toml directory.toml   # then fill in the pubkeys

# 3. Start the relays.
TOR_LISTEN=127.0.0.1:9101 TOR_KEY_FILE=./relayA.key cargo run -- relay &
TOR_LISTEN=127.0.0.1:9102 TOR_KEY_FILE=./relayB.key cargo run -- relay &
TOR_LISTEN=127.0.0.1:9103 TOR_KEY_FILE=./relayC.key cargo run -- relay &

# 4. Start the client (local SOCKS5 proxy on :9050).
TOR_DIRECTORY=./directory.toml TOR_HOPS=3 cargo run -- client &

# 5. Send traffic through it. Use socks5h so DNS is resolved at the exit.
curl -x socks5h://127.0.0.1:9050 https://example.com/
```

The client also serves a **web dashboard** at <http://127.0.0.1:9060/>.

## Web dashboard & docs

In `client` mode a small web server runs alongside the SOCKS proxy
(`TOR_UI_LISTEN`, default `127.0.0.1:9060`):

- **`/`** — a dashboard showing live circuit counters, a "browse through the
  onion network" box (fetch an `http://` URL through a fresh circuit — handy to
  confirm your exit IP), and copy-paste proxy config.
- **`/docs`** and **`/docs/{name}`** — the markdown files in `docs/` rendered to
  HTML.
- **`/proxy.pac`** — a browser proxy auto-config pointing at the SOCKS port.
- **`/api/status`**, **`/api/fetch?url=`** — JSON used by the dashboard.

Browsers cannot speak SOCKS from a web page, so the UI's job is to prove the
overlay works and hand you the config to point real apps (curl, Firefox,
Chromium) at the SOCKS proxy — which carries HTTPS end-to-end.

## Backends: private overlay or the real Tor network

The same SOCKS port and dashboard run over either of two backends, chosen with
`TOR_BACKEND`:

| Backend            | `TOR_BACKEND` | What it uses                                              |
| ------------------ | ------------- | -------------------------------------------------------- |
| Private overlay    | `overlay` (default) | This project's own onion relays (from your directory). |
| **Real Tor**       | `arti`        | The actual Tor network via [Arti](https://gitlab.torproject.org/tpo/core/arti). |

The project's *own* protocol is **not** wire-compatible with Tor (custom cells,
an ntor-*like* handshake, no directory/consensus) — see
[docs/tor-interop.md](docs/tor-interop.md). Rather than reimplement tor-spec, the
`arti` backend embeds the Tor Project's official Rust client, so real Tor is a
build flag away:

```sh
# Build with Tor support (pulls the Arti stack) and run over the real network.
cargo build --release --features arti
TOR_BACKEND=arti cargo run --release --features arti -- client

# Verify: the Tor Project's own checker reports IsTor:true and a Tor exit IP.
curl -x socks5h://127.0.0.1:9050 https://check.torproject.org/api/ip
# {"IsTor":true,"IP":"185.220.101.181"}
# .onion services work too:
curl -x socks5h://127.0.0.1:9050 https://duckduckgogg42xjoc72x3sjasowoarfbgcmvfimaftt6twagswzczad.onion/
```

With `arti`, `TOR_DIRECTORY` is not needed (Tor's directory authorities provide
the consensus). The dashboard's backend badge shows which mode is active.

## Hardening

- **Exit policy (SSRF protection):** exits refuse loopback, private
  (RFC1918/CGNAT/ULA), link-local, and cloud-metadata (`169.254.169.254`)
  destinations by default. Override for local testing with
  `TOR_EXIT_ALLOW_PRIVATE=1`.
- **Overlay pre-shared key:** `TOR_NETWORK_SECRET` is folded into every
  handshake, so only nodes/clients sharing it can build circuits.
- **Extend allowlist:** `TOR_RELAY_PEERS` pins which peers a relay will extend to.
- **Limits & timeouts:** handshakes time out after 20 s; `TOR_MAX_CIRCUITS`
  bounds concurrent circuits; frames are capped at 1 MiB; doc names are
  sanitized against path traversal.

See [docs/security.md](docs/security.md) for the full model.

## Configuration

| Env var             | Mode   | Default            | Meaning                                  |
| ------------------- | ------ | ------------------ | ---------------------------------------- |
| `TOR_ROLE`          | all    | (from argv[1])     | `relay` \| `client` \| `keygen`          |
| `TOR_BACKEND`       | client | `overlay`          | `overlay` (own relays) \| `arti` (real Tor; needs `--features arti`) |
| `TOR_NETWORK_SECRET`| all    | (empty = open)     | Overlay pre-shared key folded into handshakes |
| `TOR_LISTEN`        | relay  | `0.0.0.0:9001`     | Relay listen address                     |
| `TOR_KEY_FILE`      | relay  | `./relay.key`      | Static identity key file (created if absent) |
| `TOR_EXIT_ALLOW_PRIVATE` | relay | `0`           | Allow exits to private/loopback ranges   |
| `TOR_RELAY_PEERS`   | relay  | (any)              | Comma-separated `host:port` extend allowlist |
| `TOR_MAX_CIRCUITS`  | relay  | `1024`             | Max concurrent circuits before rejecting |
| `TOR_SOCKS_LISTEN`  | client | `127.0.0.1:9050`   | Local SOCKS5 listen address              |
| `TOR_UI_LISTEN`     | client | `127.0.0.1:9060`   | Dashboard/docs listen address            |
| `TOR_DIRECTORY`     | client | (required)         | Path to the relay directory TOML         |
| `TOR_HOPS`          | client | `3`                | Number of relays per circuit             |
| `TOR_DOCS_DIR`      | client | `./docs`           | Directory of markdown docs to serve      |
| `RUST_LOG`          | all    | `info`             | Log filter (`tracing` env-filter syntax) |

## Tests

- **Rust unit tests:** `cargo test` (handshake agreement/authentication, AEAD
  round-trip, nonce-desync detection, exit-policy ranges).
- **Browser e2e (Playwright + Puppeteer):** [tests/](tests/) drive a real
  headless Chromium **through the SOCKS proxy** to surf a local origin, proving
  traffic traverses the overlay (the client's `circuits_built` counter grows),
  DNS resolves at the exit, and caches are busted on every navigation. They also
  exercise the dashboard and rendered docs.

  ```sh
  cd tests && npm install && npm run setup && npm test
  ```

## Container

```sh
docker build -t oresoftware/tor-server:0.1.0 .
```

See the `Dockerfile` header and `k8s/` for relay and client deployments. The
relay `StatefulSet` persists each pod's identity key on a volume so its public
key is stable across restarts.

## Scope & security caveats

This is an educational/private-overlay implementation. Known limitations vs.
production Tor:

- **One stream per circuit** (no stream multiplexing); a new 3-hop circuit is
  built per SOCKS connection.
- **No traffic-analysis defenses**: no fixed-size cells, no padding, no timing
  obfuscation. An observer of two links can correlate by size/timing.
- **No directory authority / consensus**: relays are listed in a static file
  you distribute; there is no reputation, flagging, or bandwidth weighting.
- **CONNECT only**; no SOCKS UDP associate, no `.onion`-style hidden services.
- **Handshake** is ntor-*like* but not the formally verified Tor ntor.

Run relays only on infrastructure you are authorized to use, and be mindful that
the exit node makes connections on your behalf.
