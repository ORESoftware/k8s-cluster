# Overview

`tor-server.rs` is an onion-routing anonymizing proxy. It gives applications a
local **SOCKS5** port and tunnels each connection through a **telescoped
multi-hop circuit** of relay nodes, with **per-hop layered encryption**. No
single relay learns both who you are and where you are going, and no relay can
read the application payload meant for hops beyond it.

It is a compact, self-contained implementation of the core onion-routing idea —
suitable for running your own private overlay of relays across machines/regions
you control to anonymize outbound traffic.

## Roles

One binary, three modes (`argv[1]` or `TOR_ROLE`):

| Mode     | What it does                                                            |
| -------- | ---------------------------------------------------------------------- |
| `relay`  | An onion relay/exit node. One static identity key; forwards cells.     |
| `client` | The local SOCKS5 proxy **and** the web dashboard. Builds circuits.     |
| `keygen` | Generate/persist a relay keypair and print its public key.             |

## The path of a request

```
 app ──SOCKS5──▶ client ─▶ relay A ─▶ relay B ─▶ relay C (exit) ─▶ destination
                    │  E_KA( E_KB( E_KC( payload ) ) )
```

- **Entry (A)** sees the client, but only that the next hop is B.
- **Middle (B)** sees only A and C — not the client, not the destination.
- **Exit (C)** sees the destination, but not the client.

## What it is not

- Not compatible with the real Tor network — see [tor-interop](/docs/tor-interop).
- Not a full anonymity system with traffic-analysis defenses — see
  [security](/docs/security).
- Not a VPN or transparent packet tunnel — see
  [cloud/VPN/obfuscation guidance](/docs/cloud-vpn-obfuscation).

Continue to [architecture](/docs/architecture), the wire
[protocol](/docs/protocol), [usage](/docs/usage), [security](/docs/security),
and [deployment](/docs/deployment).
