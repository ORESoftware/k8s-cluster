# Wire protocol

## Framing

Every message between hops is a frame: a 4-byte big-endian length followed by
that many bytes. Frames are capped at 1 MiB. A frame body is either a raw
handshake blob (during setup) or an AEAD ciphertext of a serialized cell.

## Handshake (ntor-*like*)

Per hop, key agreement runs between the client and that relay:

1. Each relay owns a long-term X25519 **static** keypair; its public key is in
   the directory.
2. The client generates an ephemeral X25519 key `x` and sends the marker `TSR2`
   followed by `X = x·G` (36 bytes total) as the **CREATE** blob.
3. The relay generates an ephemeral `y` and both sides compute the shared secret
   from two Diffie–Hellman results:
   - `s1 = DH(client_eph, relay_static)` — authenticates the relay
   - `s2 = DH(client_eph, relay_eph)` — forward secrecy
4. Both parties reject any non-contributory X25519 result. HKDF-SHA256 binds
   `s1`, `s2`, `TSR2`, the relay static key, `X`, `Y`, and the optional overlay
   PSK to produce independent forward, backward, and authentication keys.
5. The relay replies **CREATED** = `TSR2` ‖ `Y` ‖ `HMAC-SHA256(auth,
   role ‖ relay_static ‖ X ‖ Y)` (68 bytes). The client verifies the MAC in
   constant time; a mismatch or version mismatch aborts the circuit.

This provides relay authentication and forward secrecy. It is deliberately
simpler than Tor's formally analyzed ntor and is **not** interoperable with it.

## Cells

After the handshake, each message is an explicitly encoded `Cell`, sealed under
the hop's direction key with ChaCha20-Poly1305 (12-byte nonce = a per-key,
per-direction monotonic counter). The first byte is the cell tag. Byte fields
use a 4-byte big-endian length; UTF-8 strings use a 2-byte big-endian length;
`Begin` ends with its 2-byte big-endian port. The decoder operates only inside
the already bounded 1 MiB frame and rejects unknown tags, invalid UTF-8,
truncation, length overflow, and trailing bytes.

This protocol has no backward-compatibility downgrade: legacy peers fail the
explicit `TSR2` marker/length checks.

| Cell     | Meaning                                                                 |
| -------- | ---------------------------------------------------------------------- |
| `Relay`  | Forward the (already-framed) inner blob verbatim to the next hop.       |
| `Extend` | Open a link to `addr` and send `create` to it — telescopes one hop.     |
| `Begin`  | (exit) open a TCP connection to `host:port`.                            |
| `Data`   | Application bytes to/from the exit's destination.                       |
| `End`    | Tear down the stream.                                                   |

Tags are `Relay=0`, `Extend=1`, `Begin=2`, `Data=3`, and `End=4`.

## Circuit construction (telescoping)

```
client ─CREATE──▶ A           (direct; CREATED returns in the clear)
client ─Extend(B)─▶ A ─CREATE─▶ B     (CREATED returns wrapped in 1 layer)
client ─Relay(Extend(C))─▶ A ─Relay──▶ B ─CREATE─▶ C  (CREATED wrapped in 2 layers)
```

An `Extend` targeting hop *i* is executed by hop *i−1* (the current last hop).
The reply from hop *i* comes back wrapped in *i* `Relay` layers, which the client
peels using the backward keys it has established so far.
