# Architecture, threat model, and fair response

## Goals

1. Detect automated secret discovery, credential stuffing, and exploit reconnaissance with low operational risk.
2. Preserve integrity-protected evidence for correlation at the Cloudflare edge.
3. Keep attacker-controlled traffic isolated from production data, credentials, and control planes.
4. Make every automated response temporary and reversible.

A source address is not proof of a human identity. Shared NAT, carrier networks, VPN exits, compromised hosts, and security scanners make permanent IP-only punishment unreliable and unfair.

## Trust boundaries

- Cloudflare is the volumetric and application-edge boundary.
- Cloudflare Tunnel is the only intended origin path.
- The `honeypot-rs` namespace is isolated from application namespaces.
- The pod has no Kubernetes API token, persistent volume, database credential, Cloudflare mutation credential, or general outbound access.
- A separate controller may consume signed events and reconcile expiring edge controls only after independent log correlation.

Cloudflare must absorb, challenge, rate-limit, or discard denial-of-service traffic before the origin. Forwarding a flood to Kubernetes would turn the sensor into a resource-exhaustion and cost-amplification target.

A later Worker or ruleset may divert exact paths such as `/.env` and `/.git/config` only when the path is impossible in the protected application, body sizes and methods are bounded, edge rate limits run first, a kill switch can return an edge-generated 404, origin saturation disables diversion, and no credential-bearing request content is copied into logs.

## Default response table

| Evidence in rolling 24 hours | Recommendation | Expiration |
|---|---|---:|
| Initial lure discovery | Observe | none |
| Eight authentication attempts | Rate limit | 15 minutes |
| Sustained exploit or authentication probing | Managed challenge | 30 minutes |
| First exact honeytoken use | Managed challenge | 1 hour |
| Second exact honeytoken use | Temporary block | 24 hours |
| Three uses across three independent lures | Human review with interim hold | at most 24 hours pending review |

## Required safeguards

- Never create an automatic permanent block.
- Maintain allowlists for authorized scanners, uptime monitors, and researchers.
- Retain a reason, evidence window, and expiration for each edge control.
- Provide an immediate operator rollback path.
- Correlate a signed origin event with Cloudflare request evidence before applying friction longer than one hour.
- Do not retaliate, hack back, deliver malware, reflect traffic, publicly shame a source, or contact unrelated third parties.

## Deferred high-interaction capabilities

The first release intentionally excludes SSH shells, database listeners, packet capture, malware detonation, unrestricted uploads, arbitrary command execution, and full request retention. Any future high-interaction research environment requires separate infrastructure, authorization, containment, and evidence-handling policy.
