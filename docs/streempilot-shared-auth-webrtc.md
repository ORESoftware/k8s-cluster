# StreemPilot WebRTC signaling + shared-auth

StreemPilot uses the existing `webrtc-signaling-rs` service for room membership and SDP/ICE relay. Authentication and room admission happen exactly once, during the WebSocket HTTP upgrade. Audio, video, WebRTC data channels, and subsequent signaling frames do **not** call shared-auth or the StreemPilot API.

## Trust boundary

The signaling service accepts identity only from a reverse proxy that has completed a successful StreemPilot room-admission subrequest. Shared-auth/Supabase authenticates the user when StreemPilot mints the short-lived peer grant; the admission endpoint then verifies that grant against the exact WebSocket `room + peer`. Identity-only `/auth/verify` is not sufficient for production signaling because a valid user must not be able to join an arbitrary room.

The signaling service requires two proxy-only headers before trusting any `x-auth-*` response headers:

- `x-shared-auth-verified: 1`
- `x-streempilot-auth-proxy: <WEBRTC_AUTH_PROXY_SECRET>`

The proxy must strip all incoming `x-auth-*`, `x-shared-auth-verified`, and `x-streempilot-auth-proxy` request headers before setting its own values. `WEBRTC_AUTH_PROXY_SECRET` must be injected from Fiducia/ExternalSecret and must never be exposed to a browser, native app, or StreemPilot API response.

The trusted identity is serialized separately as `peer.identity`. A client's `hello.metadata` object is presentation-only. Reserved identity keys such as `identity`, `auth`, `userId`, `roles`, `aal`, and `verified` are removed by the signaling service.

## Admission grants

A StreemPilot peer grant is short-lived and signed. It binds one role, one `room`, one `peer`, one expiry, and one replay-auditable `jti`.

- Browser clients exchange the peer grant for a room-specific, host-only HttpOnly cookie through `POST /v1/signaling/sessions/browser-cookie`. The API and WebSocket gateway therefore share one public origin.
- Flutter desktop/mobile clients send the peer grant—not their long-lived shared-auth token—as `Authorization: Bearer <peer_token>` during the WebSocket upgrade.
- The gateway forwards either credential to `GET /v1/signaling/sessions/authorize` together with the original room and peer. The endpoint returns only gateway-consumable identity/scope headers.

The peer grant must never appear in a WebSocket URL, access log, client metadata object, or signaling frame.

## Modes

Set `WEBRTC_AUTH_MODE` on the signaling deployment:

- `disabled`: ignore trusted identity headers; intended only for isolated development.
- `optional` (default): accept guests, but trust an identity only when the verified proxy contract is complete. Spoofed/incomplete auth headers are rejected.
- `required`: reject the WebSocket upgrade unless the verified proxy identity is present. The process refuses to start in this mode without a 32-byte-or-longer `WEBRTC_AUTH_PROXY_SECRET`.

Production StreemPilot routes should use `required`. Guest invites use the same required signaling route after the API has redeemed the invite and minted a room/peer-bound peer grant; guest admission does not require an unauthenticated signaling route.

## NGINX upgrade pattern

The concrete StreemPilot API Service DNS and secret source are deployment values; do not hard-code them in a public ConfigMap.

```nginx
location = /_streempilot_signal_authorize {
    internal;
    proxy_pass http://${STREEMPILOT_API_UPSTREAM}/v1/signaling/sessions/authorize;
    proxy_pass_request_body off;
    proxy_set_header Content-Length "";

    # Flutter sends a peer grant as bearer. Browsers send the room-specific
    # HttpOnly cookie written by the StreemPilot API.
    proxy_set_header Authorization $http_authorization;
    proxy_set_header Cookie $http_cookie;

    # Bind authorization to the exact coordinates from the requested upgrade.
    proxy_set_header X-Streempilot-Room-Id $arg_room;
    proxy_set_header X-Streempilot-Peer-Id $arg_peer;
}

location /streempilot/webrtc/signal {
    auth_request /_streempilot_signal_authorize;

    # Values returned only after the peer grant matches room + peer.
    auth_request_set $auth_user_id $upstream_http_x_auth_user_id;
    auth_request_set $auth_roles   $upstream_http_x_auth_roles;
    auth_request_set $auth_aal     $upstream_http_x_auth_aal;
    auth_request_set $grant_room   $upstream_http_x_streempilot_room_id;
    auth_request_set $grant_peer   $upstream_http_x_streempilot_peer_id;
    auth_request_set $grant_jti    $upstream_http_x_streempilot_grant_jti;

    # Never forward caller-supplied trust headers.
    proxy_set_header X-Auth-User-Id "";
    proxy_set_header X-Auth-Roles "";
    proxy_set_header X-Auth-Aal "";
    proxy_set_header X-Shared-Auth-Verified "";
    proxy_set_header X-Streempilot-Auth-Proxy "";

    # Replace them with gateway-owned values after auth_request succeeds.
    proxy_set_header X-Auth-User-Id $auth_user_id;
    proxy_set_header X-Auth-Roles $auth_roles;
    proxy_set_header X-Auth-Aal $auth_aal;
    proxy_set_header X-Shared-Auth-Verified "1";
    proxy_set_header X-Streempilot-Auth-Proxy "${WEBRTC_AUTH_PROXY_SECRET}";

    proxy_http_version 1.1;
    proxy_set_header Upgrade $http_upgrade;
    proxy_set_header Connection "upgrade";
    proxy_set_header Host $host;
    proxy_read_timeout 900s;
    proxy_send_timeout 900s;
    proxy_pass http://dd-webrtc-signaling:8095/webrtc/signal;
}
```

The gateway must be the only network path to the signaling deployment in `required` mode. It must overwrite—not append to—every trust header. The private proxy proof is added only after a successful room-admission response.

## Performance and scaling

- Admission performs one local JWT verification during the WebSocket upgrade and adds no latency to offer, answer, or ICE delivery after connection.
- Reconnects re-authorize, so expired or out-of-scope grants fail before a signaling socket is allocated.
- The signaling service caps a WebSocket text message/frame at 64 KiB and client metadata at 4 KiB.
- Small studios continue to use direct WebRTC with TURN fallback.
- Larger rooms should move interactive media to an SFU; the signaling server remains a control-plane component and must not become the media relay.
- RTMP/SRT multistream publishing and isolated recording upload remain separate asynchronous planes so a slow destination cannot stall a call.

## Rollout

1. Deploy this signaling version in `optional` mode behind the gateway.
2. Deploy the StreemPilot room-admission API and configure same-origin browser cookie exchange plus native peer-grant bearer upgrades.
3. Configure the gateway auth subrequest and proxy secret; verify a matching grant produces `peer.identity` while spoofed, expired, cross-room, and cross-peer attempts receive 401.
4. Restrict direct network access to the signaling deployment and switch the production route to `required`.
5. Verify guest invite redemption mints expiring, room-bound, peer-bound, role-scoped grants with a unique `jti`.

## Merge gate

The branch contains the compiled `main.rs` integration—not only the helper module—and the real signaling crate passed all 45 tests with the repository's pinned private libraries checked out. The one-shot integration workflow has been removed.
