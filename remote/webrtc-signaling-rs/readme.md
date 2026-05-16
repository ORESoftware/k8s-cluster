# `remote/webrtc-signaling-rs`

Rust WebRTC signaling service for the EC2 Kubernetes runtime.

This deployment does the WebRTC handshake coordination only:

- room membership over WebSocket
- peer join/leave notifications
- offer/answer forwarding
- ICE candidate forwarding
- Prometheus metrics
- health checks

It intentionally does not relay audio, video, or WebRTC data-channel payloads.

## Why Signaling Instead Of A Media Relay

WebRTC needs a small amount of server help before peers can talk directly. The
server lets peers find each other, exchange SDP offers/answers, and exchange ICE
candidates. Once that handshake succeeds, browsers and mobile clients send media
or data directly to each other.

That signaling-only shape is the right default here because it keeps the service
small, cheap, and low latency:

- The server never handles camera/microphone media, so CPU and bandwidth stay
  tiny compared with an SFU or TURN relay.
- Browser to browser, browser to mobile, and mobile to mobile all use the same
  WebSocket JSON protocol.
- The Kubernetes deployment only needs normal HTTP/WebSocket scaling and
  telemetry.
- Sensitive media/data payloads do not pass through the cluster after the peer
  connection is established.

A media relay is still useful later, but it solves a different problem. Add a
TURN server such as coturn when direct peer-to-peer paths fail because of strict
NATs, corporate firewalls, or mobile carrier networks. Add an SFU only when we
need server-side multi-party media routing, recording, simulcast, moderation, or
large rooms.

## Public Gateway Paths

When deployed behind `dd-remote-gateway`:

- `GET /webrtc/` renders the service page.
- `GET /webrtc/healthz` returns JSON health.
- `GET /webrtc/metrics` exposes Prometheus metrics.
- `GET /webrtc/signal?room=<roomId>&peer=<peerId>` upgrades to WebSocket.

Use `wss://54.91.17.58/webrtc/signal?room=<roomId>&peer=<peerId>` from browser
or mobile clients when hitting the current EC2 gateway.

## WebSocket Protocol

Clients send JSON text frames. Supported message types:

- `hello`: optional metadata update.
- `ping`: returns `pong`.
- `offer`: forwards an SDP offer.
- `answer`: forwards an SDP answer.
- `ice` or `candidate`: forwards ICE candidates.
- `renegotiate`: asks peers to renegotiate.
- `message`: generic app-level signaling message.
- `bye`: announces the peer is leaving and closes the connection.

Use a `to` field for targeted delivery. Omit `to` to broadcast to every other
peer in the same room.

```json
{
  "type": "offer",
  "to": "mobile-peer",
  "payload": {
    "sdp": "..."
  }
}
```

The server wraps forwarded frames as:

```json
{
  "type": "signal",
  "signalType": "offer",
  "room": "demo-room",
  "from": "browser-peer",
  "to": "mobile-peer",
  "payload": {
    "sdp": "..."
  }
}
```

## Client Responsibilities

Clients still own the actual `RTCPeerConnection` setup:

- configure STUN/TURN servers in the client
- create offers and answers
- call `setLocalDescription` and `setRemoteDescription`
- add ICE candidates
- open media tracks or data channels

For early tests, public STUN is usually enough. For production mobile support,
deploy TURN and pass those credentials to clients.
