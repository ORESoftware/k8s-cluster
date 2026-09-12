# Architecture

## Components

| Module        | Responsibility                                                       |
| ------------- | ------------------------------------------------------------------- |
| `wire`        | Length-prefixed framing over async streams (4-byte BE length + body). |
| `cell`        | The relay-cell vocabulary (`Relay`/`Extend`/`Begin`/`Data`/`End`).  |
| `crypto`      | Per-hop handshake, HKDF key derivation, ChaCha20-Poly1305 AEAD.      |
| `config`      | Directory (relay list) loading, key persistence, path selection.    |
| `policy`      | Exit range filtering (SSRF protection) + extend allowlist.          |
| `relay`       | The relay/exit node: accept, handshake, forward/peel one layer.     |
| `circuit`     | Client-side circuit construction + bidirectional splicing.          |
| `socks`       | Bounded SOCKS5 front-end (CONNECT + optional RFC 1929 auth).         |
| `web`         | Dashboard UI, docs server, `/api/*`, PAC.                           |
| `stats`       | Process-wide circuit counters for the dashboard.                    |

## Data flow

The client is the only party that holds every hop's key. Relays each hold one
key pair and know only their immediate neighbours.

- **Forward (client → destination):** the client onion-wraps a cell for the exit
  and wraps that in a `Relay` cell for each shallower hop. Each relay decrypts
  its one layer and forwards the inner blob to the next hop; the exit decrypts
  the final layer to plaintext and writes it to the destination socket.
- **Backward (destination → client):** the exit wraps destination bytes in a
  `Data` cell; each middle relay wraps what it reads from its next hop in a
  `Relay` cell under its backward key; the client peels every layer.

Because every forward message traverses every relay and every backward message
is wrapped by every relay, per-hop AEAD nonce counters stay in lockstep between
the client and each relay (one increment per frame, per direction).

## One circuit per connection

For simplicity each SOCKS connection builds its own fresh circuit (a single
stream per circuit — no multiplexing). This trades throughput for a much simpler
relay that is stream-agnostic: a relay just decrypts-and-forwards one direction
and encrypts-and-forwards the other, without tracking stream IDs.

## Concurrency model

Every accepted link is a Tokio task. A relay splits each TCP link into read/write
halves so the forward path (previous → next) and the backward pump
(next → previous) each have a single writer, avoiding locks. The client runs the
up and down directions as two concurrent futures joined together.
