# `remote/deployments/webrtc-media-rs`

Rust WebRTC media-plane configuration service for the EC2 Kubernetes runtime.

This service is deliberately separate from `dd-webrtc-signaling`. The signaling service keeps doing
room membership and offer/answer/ICE forwarding over WebSocket. `dd-webrtc-media` publishes the
optional media-plane configuration that clients or signaling code can consume when the deployment is
configured for STUN, TURN, SFU, or a media relay.

It does not relay audio, video, data-channel packets, or TURN UDP packets by itself. When
`WEBRTC_MEDIA_MODE` enables `turn`, `sfu`, or `media`, a backing data-plane service must exist with
the required public UDP/TCP networking, security-group rules, and credentials.

## Modes

`WEBRTC_MEDIA_MODE` is a comma/plus/slash/space separated set:

- `disabled` or `stun` - default; exposes STUN-only ICE metadata.
- `turn` - advertises TURN ICE servers and credentials.
- `sfu` - advertises an SFU endpoint for clients that negotiate through a media router.
- `media` or `relay` - advertises a generic media-relay endpoint.
- `all` or `full` - enables TURN, SFU, and media-relay metadata.

The service returns `503` from `/healthz` and `/readyz` if a selected mode is missing required
configuration. This keeps an accidentally enabled media mode from becoming silently half-live.

## Environment

| Env | Default | Purpose |
| --- | --- | --- |
| `HOST` | `0.0.0.0` | HTTP listen host. |
| `PORT` | `8125` | HTTP listen port. |
| `WEBRTC_MEDIA_MODE` | `disabled` | Media-plane capability mode. |
| `WEBRTC_STUN_URLS` | `stun:stun.l.google.com:19302` | Comma-separated STUN URLs. |
| `WEBRTC_TURN_URLS` | empty | Comma-separated TURN URLs, for example `turn:turn.example.com:3478?transport=udp,turns:turn.example.com:5349`. |
| `WEBRTC_TURN_USERNAME` | empty | TURN username to return from `/ice` when TURN mode is enabled. |
| `WEBRTC_TURN_CREDENTIAL` | empty | TURN credential to return from `/ice` when TURN mode is enabled. Source this from Kubernetes Secret data. |
| `WEBRTC_SFU_ENDPOINT` | empty | HTTPS/WSS endpoint for the SFU control plane when SFU mode is enabled. |
| `WEBRTC_MEDIA_RELAY_ENDPOINT` | empty | HTTPS/WSS endpoint for a generic media relay when media relay mode is enabled. |
| `WEBRTC_PUBLIC_HOST` | empty | Optional public host advertised in config summaries. |
| `WEBRTC_UDP_PORT_RANGE` | empty | Optional data-plane UDP port range metadata. |
| `WEBRTC_TURN_UDP_PORT` | empty | Optional TURN UDP port metadata, commonly `3478`. |
| `WEBRTC_TURNS_TLS_PORT` | empty | Optional TURN-over-TLS port metadata, commonly `5349`. |

## HTTP API

- `GET /webrtc-media/` renders a short service page.
- `GET /webrtc-media/healthz` and `/webrtc-media/readyz` return JSON health.
- `GET /webrtc-media/config` returns redacted media-plane configuration.
- `GET /webrtc-media/capabilities` returns the enabled capability booleans.
- `GET /webrtc-media/ice` returns a browser-shaped `iceServers` payload. TURN credentials are
  included only here.
- `GET /webrtc-media/metrics` exposes Prometheus metrics.
- `GET /docs/api`, `/api/docs`, and `/api/docs.json` expose generated API docs.

Unprefixed internal routes are also mounted for in-cluster callers: `/healthz`, `/readyz`,
`/config`, `/capabilities`, `/ice`, and `/metrics`.

## Networking Notes

HTTP config traffic can safely sit behind the existing authenticated gateway. UDP media traffic
cannot. If TURN or SFU mode is enabled, add a dedicated data-plane deployment or external service and
open the relevant UDP/TCP ports outside the HTTP gateway path.

Recommended first production step is TURN:

1. Deploy a real TURN server such as coturn, or a Rust TURN implementation if we decide to own the
   protocol surface.
2. Give it a public hostname and UDP/TCP listener ports.
3. Store TURN credentials in `dd-agent-secrets` or a dedicated ExternalSecret.
4. Set `WEBRTC_MEDIA_MODE=turn`, `WEBRTC_TURN_URLS`, `WEBRTC_TURN_USERNAME`, and
   `WEBRTC_TURN_CREDENTIAL` on this service.
5. Have clients or `dd-webrtc-signaling` fetch `/webrtc-media/ice` and pass the returned
   `iceServers` into `RTCPeerConnection`.

Add an SFU later only when the product needs server-side room media routing, recording, moderation,
simulcast, or large multi-party calls.

> **ORM policy:** prefer **SeaORM** over sqlx for new database code (MASH stack: maud, axum, SeaORM, supabase, htmx).
