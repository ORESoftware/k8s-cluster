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
- **End-to-end integrity from the exit.** Application `Data`/`End` cells are
  accepted by the client only when they decrypt at the *exit* layer. Because each
  relay holds the backward key for its own hop, this prevents a malicious
  middle/entry relay from injecting bytes the application would attribute to the
  exit — sealing a valid cell at the exit layer requires the exit's key.
- **Modern private-overlay primitives.** The custom protocol uses X25519,
  HKDF-SHA256, HMAC-SHA256, and ChaCha20-Poly1305. RSA is neither needed nor
  desirable for this private key agreement. This is not the same as possessing
  Tor's Ed25519 certificate hierarchy, signed consensus, or analyzed ntor-v3.

## Hardening built in

- **Exit policy (SSRF protection).** By default an exit refuses to connect to
  loopback, RFC1918/CGNAT/ULA private ranges, link-local, and the cloud metadata
  address `169.254.169.254`. It resolves the destination first and rejects if all
  resolved addresses are blocked. IPv4-mapped/6to4/NAT64 IPv6 forms that embed a
  private v4 address (e.g. `::ffff:127.0.0.1`) are unwrapped and blocked too, so
  they cannot be used to bypass the v4 rules. Override for local testing with
  `TOR_EXIT_ALLOW_PRIVATE=1`. `TOR_EXIT_DENY_PORTS` defaults to `25` to reduce
  SMTP abuse from a cloud exit; operators can set a stricter comma-separated
  denylist.
- **Extend allowlist.** Set `TOR_RELAY_PEERS` to pin which `host:port` a relay
  will extend to, preventing relays from being used to reach arbitrary internal
  hosts. In untrusted deployments (relays reachable by parties you don't
  control) this should be set, since `Extend` targets are otherwise unrestricted.
  A non-loopback relay started without an allowlist logs a warning at startup.
- **Middle-only relays.** `TOR_DISABLE_EXIT=1` makes a relay refuse `Begin`
  outright, so it never resolves or connects to a real destination. Any client
  can otherwise turn *any* reachable relay into its exit by sending `Begin`;
  this flag lets an operator confine exiting to designated nodes and keep other
  relays purely as onion-forwarding middles. (Effective only when the directory
  holds more relays than `TOR_HOPS`, or a middle-only relay would be forced into
  the exit slot and its circuits would fail.)
- **Overlay pre-shared key.** `TOR_NETWORK_SECRET` (or `TOR_NETWORK_SECRET_FILE`,
  which keeps it out of the process environment) is folded into every handshake,
  so only nodes/clients sharing it can build circuits — turning the open overlay
  into a closed one.
- **Fail-closed exposed listeners.** Non-loopback relays require a PSK unless
  `TOR_ALLOW_OPEN_RELAY=1` is set. Non-loopback SOCKS requires an explicit remote
  opt-in and RFC 1929 credentials. The optional HTTP `CONNECT` front-end
  (`TOR_HTTP_LISTEN`) inherits the identical posture: loopback by default, and a
  non-loopback bind requires `TOR_HTTP_ALLOW_REMOTE=1` plus the shared proxy
  password, with `Proxy-Authorization: Basic` checked in constant time. It
  supports `CONNECT` only (no absolute-URI forwarding) and applies the backend's
  exit policy to every destination, so it is never an open proxy. Non-loopback
  dashboard proxying requires a token unless the unsafe override is explicit.
- **Timeouts & circuit cap.** Half-open handshakes are dropped after 20 s; dialing
  the next hop/destination is bounded (15 s relay, 60 s client); the SOCKS
  negotiation must finish in 30 s; `TOR_MAX_CIRCUITS` bounds concurrent circuits
  (reject, don't queue); `TOR_CIRCUIT_IDLE_TIMEOUT_SECS` optionally closes idle
  circuits (0 = off, to avoid breaking legitimately long-idle streams).
  `TOR_MAX_SOCKS_CONNECTIONS` separately bounds accepted application streams. A
  circuit holds its `TOR_MAX_CIRCUITS` slot until *both* its forward handler and
  its detached backward pump finish, so the cap reflects real resource use and a
  long-lived stream cannot free a slot while its sockets are still open.
- **Dashboard `/api/fetch` auth.** The fetch endpoint is a server-side proxy
  primitive. When the dashboard is bound to a non-loopback address, set
  `TOR_UI_TOKEN` (or `TOR_UI_TOKEN_FILE`); requests must then present it via
  `?token=` or `Authorization: Bearer` (checked in constant time). Without it,
  `/api/fetch` is an unauthenticated proxy — the process logs a warning if bound
  non-loopback with no token. The URL's host/path are also rejected if they
  contain control characters, preventing CRLF header-injection/request smuggling.
  The dashboard is rendered server-side with Maud (auto-escaping), so the fetched
  response's status line and the directory's relay names/addresses cannot inject
  script into the dashboard origin; the live-stats WebSocket updates the grid via
  `textContent`, never `innerHTML`. The UI's only script (htmx) is vendored into
  the binary and served from `/vendor/…` — no external CDN is contacted at
  runtime, removing that supply-chain and CSP exposure.
- **Relay key file safety.** The static identity secret is atomically created
  with `create_new`; on Unix its `0600` mode is applied at creation time.
- **Framer/parser bounds.** Frames and the explicit cell codec are capped at
  1 MiB; unknown tags, truncation, length overflow, invalid string encoding,
  and trailing cell bytes are rejected; doc names are sanitized against path
  traversal; the fetch preview is size-capped.

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
- **Not a VPN.** There is no TUN/TAP device, IP routing, UDP ASSOCIATE, ICMP, DNS
  interception, kill switch, or leak prevention for applications that ignore
  SOCKS settings.
- **Remote SOCKS auth is not transport encryption.** RFC 1929 credentials and
  SOCKS payloads are visible on the client-to-proxy link. Bind locally or place
  it inside WireGuard, SSH, mTLS, or a trusted private network.
- **Static private-overlay trust.** The relay X25519 key doubles as the pinned
  authentication key. There are no Ed25519 signatures/certificates, automated
  onion-key rotation, signed directory consensus, or revocation channel.
- **Tracked Arti RSA advisory.** Arti 0.44 transitively contains `rsa 0.9.10`,
  which RustSec flags as `RUSTSEC-2023-0071` with no fixed release. The timing
  issue leaks RSA *private* keys during private-key operations. This binary's
  Arti integration is client-only and does not create, load, or use an RSA
  private key; its own overlay has no RSA at all. The audit exception is pinned
  to that advisory only and must be removed when Arti/RustCrypto provides a
  fixed dependency.

## Operational advice

- Run relays only on infrastructure you are authorized to use. An exit makes
  connections on your behalf.
- Prefer `socks5h://` (remote DNS) so lookups happen at the exit, not locally.
- Use the supplied NetworkPolicy and high-entropy Secret mounts in Kubernetes.
- Rotate a relay identity by generating a new key, updating its pinned directory
  entry, and restarting the affected relay/client in a coordinated maintenance
  window. Mixed v1/`TSR2` deployments are intentionally unsupported.
- For real-world anonymity against capable adversaries, use Tor/Arti — see
  [tor-interop](/docs/tor-interop).
