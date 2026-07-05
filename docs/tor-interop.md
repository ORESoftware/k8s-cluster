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
| Cell format        | Fixed 514-byte cells, `RELAY` cells, stream IDs      | variable-length bincode cells, 1 stream/circuit |
| Handshake          | `ntor` / `ntor v3` (formally analyzed)               | ntor-*like* (X25519 + HKDF + HMAC)     |
| Directory          | Directory authorities + signed hourly consensus      | a static TOML file you distribute      |
| Onion services     | `.onion` v3 (rendezvous, HSDir, descriptors)         | none                                   |
| Path selection     | Guards, bandwidth weights, family/subnet constraints | uniform random                         |

Even the parts with the same name (a three-hop circuit, an ntor-ish handshake,
SOCKS5 in front) are wire-incompatible. A Tor relay would reject our CREATE, and
we would reject its cells.

## If you need the real Tor network

Use the official implementations rather than trying to make this interoperate:

- **Arti** — the Rust Tor client (`arti`), the modern, maintained option. Run
  `arti proxy` for a SOCKS port on the real network.
- **C Tor** (`tor`) — the reference daemon; run it and use its SOCKS port.

You can point the same applications at Arti/Tor's SOCKS port exactly as you would
point them at this project's — the *client interface* (SOCKS5) is the same even
though the *network* is not.

## When this project is the right tool

- A **private overlay** across machines/regions you control, to anonymize
  outbound traffic within a system you operate.
- Learning/experimenting with onion routing internals in readable Rust.
- Environments where running the full Tor stack is undesirable and you only need
  layered relaying among your own nodes.
