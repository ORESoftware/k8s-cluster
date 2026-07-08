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
  resolved addresses are blocked. IPv4-mapped/6to4/NAT64 IPv6 forms that embed a
  private v4 address (e.g. `::ffff:127.0.0.1`) are unwrapped and blocked too, so
  they cannot be used to bypass the v4 rules. Override for local testing with
  `TOR_EXIT_ALLOW_PRIVATE=1`.
- **Extend allowlist.** Set `TOR_RELAY_PEERS` to pin which `host:port` a relay
  will extend to, preventing relays from being used to reach arbitrary internal
  hosts. In untrusted deployments (relays reachable by parties you don't
  control) this should be set, since `Extend` targets are otherwise unrestricted.
- **Overlay pre-shared key.** `TOR_NETWORK_SECRET` (or `TOR_NETWORK_SECRET_FILE`,
  which keeps it out of the process environment) is folded into every handshake,
  so only nodes/clients sharing it can build circuits — turning the open overlay
  into a closed one.
- **Timeouts & circuit cap.** Half-open handshakes are dropped after 20 s; dialing
  the next hop/destination is bounded (15 s relay, 60 s client); the SOCKS
  negotiation must finish in 30 s; `TOR_MAX_CIRCUITS` bounds concurrent circuits
  (reject, don't queue); `TOR_CIRCUIT_IDLE_TIMEOUT_SECS` optionally closes idle
  circuits (0 = off, to avoid breaking legitimately long-idle streams).
- **Dashboard `/api/fetch` auth.** The fetch endpoint is a server-side proxy
  primitive. When the dashboard is bound to a non-loopback address, set
  `TOR_UI_TOKEN` (or `TOR_UI_TOKEN_FILE`); requests must then present it via
  `?token=` or `Authorization: Bearer` (checked in constant time). Without it,
  `/api/fetch` is an unauthenticated proxy — the process logs a warning if bound
  non-loopback with no token. The URL's host/path are also rejected if they
  contain control characters, preventing CRLF header-injection/request smuggling.
- **Relay key file permissions.** The static identity secret is written `0600`.
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
