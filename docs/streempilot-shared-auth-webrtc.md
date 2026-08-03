# StreemPilot WebRTC signaling + shared-auth

StreemPilot uses the existing `webrtc-signaling-rs` service for room membership and SDP/ICE relay. Authentication happens exactly once, during the WebSocket HTTP upgrade. Audio, video, WebRTC data channels, and subsequent signaling frames do **not** call shared-auth.

## Trust boundary

The signaling service accepts identity only from a reverse proxy that has already completed a successful shared-auth `/auth/verify` subrequest. It requires two proxy-only headers before trusting any `x-auth-*` response headers:

- `x-shared-auth-verified: 1`
- `x-streempilot-auth-proxy: <WEBRTC_AUTH_PROXY_SECRET>`

The proxy must strip all incoming `x-auth-*`, `x-shared-auth-verified`, and `x-streempilot-auth-proxy` request headers before setting its own values. `WEBRTC_AUTH_PROXY_SECRET` must be injected from Fiducia/ExternalSecret and must never be exposed to a browser or native app.

The trusted identity is serialized separately as `peer.identity`. A client's `hello.metadata` object is presentation-only. Reserved identity keys such as `identity`, `auth`, `userId`, `roles`, `aal`, and `verified` are removed by the signaling service.

## Modes

Set `WEBRTC_AUTH_MODE` on the signaling deployment:

- `disabled`: ignore shared-auth headers; intended only for isolated development.
- `optional` (default): accept guests, but trust an identity only when the verified proxy contract is complete. Spoofed/incomplete auth headers are rejected.
- `required`: reject the WebSocket upgrade unless the verified proxy identity is present. The process refuses to start in this mode without a 32-byte-or-longer `WEBRTC_AUTH_PROXY_SECRET`.

Production StreemPilot hosts, producers, and authenticated guests should use `required`. A separate explicitly guest-capable route may use `optional` after the room-grant API validates the invite.

## NGINX upgrade pattern

The concrete shared-auth Service DNS and secret source are deployment values; do not hard-code them in a public ConfigMap. The gateway shape is:

```nginx
location = /_streempilot_auth_verify {
    internal;
    proxy_pass http://${SHARED_AUTH_UPSTREAM}/auth/verify;
    proxy_pass_request_body off;
    proxy_set_header Content-Length "";
    proxy_set_header Authorization $http_authorization;
    proxy_set_header Cookie $http_cookie;
    proxy_set_header X-Original-URI $request_uri;
}

location /streempilot/webrtc/signal {
    auth_request /_streempilot_auth_verify;

    # Values returned by shared-auth after successful verification.
    auth_request_set $auth_user_id $upstream_http_x_auth_user_id;
    auth_request_set $auth_roles   $upstream_http_x_auth_roles;
    auth_request_set $auth_aal     $upstream_http_x_auth_aal;

    # Never forward caller-supplied identity headers.
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

Browser sessions may authenticate with the shared-auth cookie. Flutter desktop/mobile clients present a short-lived bearer token from platform secure storage during the WebSocket upgrade. Both paths produce the same trusted headers after `/auth/verify`.

## Performance and scaling

- Shared-auth is not in the signaling frame loop and cannot add latency to offer, answer, or ICE delivery after upgrade.
- The signaling service caps a WebSocket text message/frame at 64 KiB and client metadata at 4 KiB.
- Small studios continue to use direct WebRTC with TURN fallback.
- Larger rooms should move interactive media to an SFU; the signaling server remains a control-plane component and must not become the media relay.
- RTMP/SRT multistream publishing and isolated recording upload remain separate asynchronous planes so a slow destination cannot stall a call.

## Rollout

1. Deploy this signaling version in `optional` mode.
2. Configure the gateway auth subrequest and proxy secret; verify `peer.identity` appears and spoofed headers receive 401.
3. Deploy the StreemPilot room-grant API and map product room roles to shared-auth/room claims.
4. Switch the authenticated production route to `required`.
5. Add an explicit guest-invite route only after the room grant has expiry, room binding, role, and replay protection.
