# Wire protocol

## Framing

Every message between hops is a frame: a 4-byte big-endian length followed by
that many bytes. Frames are capped at 1 MiB. A frame body is either a raw
handshake blob (during setup) or an AEAD ciphertext of a serialized cell.

## Handshake (ntor-*like*)

Per hop, key agreement runs between the client and that relay:

1. Each relay owns a long-term X25519 **static** keypair; its public key is in
   the directory.
2. The client generates an ephemeral X25519 key `x` and sends `X = x·G`
   (32 bytes) as the **CREATE** blob.
3. The relay generates an ephemeral `y` and both sides compute the shared secret
   from two Diffie–Hellman results:
   - `s1 = DH(client_eph, relay_static)` — authenticates the relay
   - `s2 = DH(client_eph, relay_eph)` — forward secrecy
4. `HKDF-SHA256(salt, s1 ‖ s2 ‖ psk)` yields `k_forward`, `k_backward`, and an
   auth key. `psk` is the optional overlay secret (`TOR_NETWORK_SECRET`), empty
   by default.
5. The relay replies **CREATED** = `Y = y·G` (32 bytes) ‖ `HMAC-SHA256(auth,
   X ‖ Y)` (32 bytes). The client verifies the MAC in constant time; a mismatch
   aborts the circuit.

This provides relay authentication and forward secrecy. It is deliberately
simpler than Tor's formally analyzed ntor and is **not** interoperable with it.

## Cells

After the handshake, each message is a bincode-serialized `Cell`, sealed under
the hop's direction key with ChaCha20-Poly1305 (12-byte nonce = a per-key,
per-direction monotonic counter):

| Cell     | Meaning                                                                 |
| -------- | ---------------------------------------------------------------------- |
| `Relay`  | Forward the (already-framed) inner blob verbatim to the next hop.       |
| `Extend` | Open a link to `addr` and send `create` to it — telescopes one hop.     |
| `Begin`  | (exit) open a TCP connection to `host:port`.                            |
| `Data`   | Application bytes to/from the exit's destination.                       |
| `End`    | Tear down the stream.                                                   |

## Circuit construction (telescoping)

```
client ─CREATE──▶ A           (direct; CREATED returns in the clear)
client ─Extend(B)─▶ A ─CREATE─▶ B     (CREATED returns wrapped in 1 layer)
client ─Relay(Extend(C))─▶ A ─Relay──▶ B ─CREATE─▶ C  (CREATED wrapped in 2 layers)
```

An `Extend` targeting hop *i* is executed by hop *i−1* (the current last hop).
The reply from hop *i* comes back wrapped in *i* `Relay` layers, which the client
peels using the backward keys it has established so far.
