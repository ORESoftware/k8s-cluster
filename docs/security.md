# Security model & hardening

## What the design protects

- **Unlinkability across a single relay.** Layered encryption means the entry
  relay never sees the destination and the exit never sees the client. A middle
  relay sees neither endpoint.
- **Relay authentication + forward secrecy.** The ntor-like handshake proves a
  relay holds the static key published in the directory, and mixes an ephemeral
  key so compromising a relay's long-term key does not retroactively decrypt
  past circuits.
- **Per-hop confidentiality & integrity.** Every layer is ChaCha20-Poly1305
  (AEAD) with non-reusable nonces.

## Hardening built in

- **Exit policy (SSRF protection).** By default an exit refuses to connect to
  loopback, RFC1918/CGNAT/ULA private ranges, link-local, and the cloud metadata
  address `169.254.169.254`. It resolves the destination first and rejects if all
  resolved addresses are blocked. Override for local testing with
  `TOR_EXIT_ALLOW_PRIVATE=1`.
- **Extend allowlist.** Set `TOR_RELAY_PEERS` to pin which `host:port` a relay
  will extend to, preventing relays from being used to reach arbitrary internal
  hosts.
- **Overlay pre-shared key.** `TOR_NETWORK_SECRET` is folded into every
  handshake, so only nodes/clients sharing it can build circuits — turning the
  open overlay into a closed one.
- **Handshake timeout + circuit cap.** Half-open connections are dropped after
  20 s; `TOR_MAX_CIRCUITS` bounds concurrent circuits (reject, don't queue).
- **Framer bounds.** Frames are capped at 1 MiB; doc names are sanitized against
  path traversal; the fetch preview is size-capped.

## Known limitations (do not treat as full Tor)

- **No traffic-analysis resistance.** No fixed-size cells, padding, or timing
  obfuscation. An adversary observing two links can correlate flows by size and
  timing.
- **One stream per circuit.** No stream multiplexing or guard-node persistence;
  path selection is uniform-random, not bandwidth/geography weighted.
- **No directory authority / consensus.** Relays come from a static file you
  distribute; there is no reputation, flagging, or revocation.
- **The exit sees plaintext for non-TLS traffic.** As with any proxy, an exit
  can read/modify unencrypted (`http://`) content. Use TLS (`https://`)
  end-to-end.
- **The handshake is ntor-*like*, not the formally verified Tor ntor.**

## Operational advice

- Run relays only on infrastructure you are authorized to use. An exit makes
  connections on your behalf.
- Prefer `socks5h://` (remote DNS) so lookups happen at the exit, not locally.
- For real-world anonymity against capable adversaries, use Tor/Arti — see
  [tor-interop](/docs/tor-interop).
