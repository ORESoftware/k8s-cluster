# Interoperability with the Tor network

Two things are true at once:

- The project's **own protocol** (the `overlay` backend) is **not** compatible
  with the Tor network — different everything (below).
- But you can still use the **real Tor network** through this project by
  selecting the **`arti` backend**, which embeds the Tor Project's official Rust
  client. `TOR_BACKEND=arti` (built with `--features arti`) gives you real Tor
  circuits, real exits, and `.onion` access behind the same SOCKS port and
  dashboard. Verified with `https://check.torproject.org/api/ip` →
  `{"IsTor":true,…}` and a working v3 `.onion` fetch.

So: to *reimplement* the Tor protocol from scratch — no, and you shouldn't. To
*use* the Tor network from here — yes, via Arti. The rest of this page explains
why reimplementing is the wrong path.

## Why not

Interoperating with the public Tor network means implementing the Tor
specifications, which differ from this project at every layer:

| Concern            | Tor                                                  | tor-server.rs                          |
| ------------------ | ---------------------------------------------------- | -------------------------------------- |
| Link layer         | TLS "OR connections" with specific cert handling     | plain TCP, length-prefixed frames      |
| Cell format        | Fixed 514-byte cells, `RELAY` cells, stream IDs      | bounded variable-length tagged cells, 1 stream/circuit |
| Handshake          | `ntor` / `ntor v3` (formally analyzed)               | ntor-*like* (X25519 + HKDF + HMAC)     |
| Directory          | Directory authorities + signed hourly consensus      | a static TOML file you distribute      |
| Onion services     | `.onion` v3 (rendezvous, HSDir, descriptors)         | none                                   |
| Path selection     | Guards, bandwidth weights, family/subnet constraints | uniform random                         |

Even the parts with the same name (a three-hop circuit, an ntor-ish handshake,
SOCKS5 in front) are wire-incompatible. A Tor relay would reject our CREATE, and
we would reject its cells.

## Using the real Tor network from here

This project already integrates Arti as a backend, so you don't run a separate
proxy:

```sh
cargo build --release --features arti
TOR_BACKEND=arti cargo run --release --features arti -- client
curl -x socks5h://127.0.0.1:9050 https://check.torproject.org/api/ip   # IsTor:true
```

Under the hood this uses [`arti-client`](https://crates.io/crates/arti-client),
the Tor Project's maintained Rust implementation, which handles the TLS link
layer, ntor handshake, directory consensus, path selection, flow control, and
onion services for you. The alternative is the C `tor` daemon; either way the
*client interface* (SOCKS5) is identical to this project's overlay mode — only
the *network* underneath differs.

### Bridges and traffic obfuscation

Builds with `--features arti` include Arti bridge and pluggable-transport
support. Set `TOR_ARTI_CONFIG` to a client TOML containing `[bridges]` and
`[[bridges.transports]]` entries, and install/mount the referenced obfs4proxy or
Snowflake client binary. This obfuscates the Tor client-to-bridge link for
censorship circumvention. It does not add padding/obfuscation to this project's
custom overlay, conceal destination traffic from the exit, or provide VPN
packet routing.

See [`arti-client.example.toml`](../arti-client.example.toml) for the expected
shape. Replace every placeholder with a current bridge line and make the
configured transport binary available to the process.

## When this project is the right tool

- A **private overlay** across machines/regions you control, to anonymize
  outbound traffic within a system you operate.
- Learning/experimenting with onion routing internals in readable Rust.
- Environments where running the full Tor stack is undesirable and you only need
  layered relaying among your own nodes.
