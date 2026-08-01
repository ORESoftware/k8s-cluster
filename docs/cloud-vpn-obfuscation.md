# Cloud proxy, VPN, and obfuscation boundaries

## Can this run on EC2 or another cloud VM?

Yes, as a SOCKS5 TCP proxy or as one relay in the private overlay. The safe
shape is:

```text
device/app -> encrypted WireGuard or SSH tunnel -> loopback SOCKS client
           -> overlay relays or Arti/Tor -> Internet destination
```

Keep the SOCKS and dashboard listeners off the public interface. Restrict relay
ports to known relay/client source addresses, require the overlay PSK, pin
`TOR_RELAY_PEERS`, and retain the default exit SSRF policy. RFC 1929 username and
password support is defense in depth for a private/tunneled listener; it is not
encryption and is unsafe by itself on the public internet.

## Is it a VPN replacement?

No. It accepts SOCKS5 `CONNECT` and carries TCP byte streams. It does not:

- create a TUN/TAP interface or install OS routes;
- carry UDP, QUIC, ICMP, or arbitrary IP packets;
- force DNS into the tunnel for applications that do not use `socks5h`;
- provide a kill switch or stop an application from bypassing the proxy.

WireGuard is the appropriate outer transport for whole-device/cloud VPN
semantics. This proxy can be layered behind it when an application also needs
the private onion overlay or real Tor exits.

## Can it obfuscate traffic?

The custom `overlay` backend cannot. Its variable-length TCP frames and timings
are fingerprintable and correlated; encryption hides contents, not traffic
shape.

The `arti` backend can load Arti bridge/pluggable-transport configuration via
`TOR_ARTI_CONFIG`. obfs4 or Snowflake can make the client-to-bridge connection
harder to identify for censorship circumvention. That is a first-link transport
feature, not a promise that all traffic is indistinguishable or correlation
resistant. The external transport executable and valid bridge lines must be
provisioned separately.

## Does it need RSA or a PKI?

The private overlay's primitives are appropriate modern choices: X25519 for
static-plus-ephemeral Diffie-Hellman, HKDF-SHA256 for key separation,
HMAC-SHA256 for transcript authentication, and ChaCha20-Poly1305 for per-hop
AEAD. The `TSR2` transcript binds the protocol version, relay static key, both
ephemeral keys, and role; non-contributory X25519 results are rejected.

RSA would not improve this private protocol. What it lacks for operation as a
public anonymity network is a signed identity/certificate hierarchy, rotating
onion keys, a signed directory consensus, revocation, guard/path policy,
fixed-size cells, padding, congestion/flow control, and extensive protocol
analysis. Use the Arti backend when those real Tor properties are required.
