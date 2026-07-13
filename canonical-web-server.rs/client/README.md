# canonical-web-server browser client

Offline-first browser sync for the authenticated `canonical-web-server.rs`
application. The marketing site does not use this package.

## Build and verify

```sh
npm ci
npm test
npm run typecheck
npm run build
```

Vite writes the single ESM bundle to `dist/app.js`.

## Bootstrap

Maud can bootstrap the current authenticated account without putting an auth
token in IndexedDB:

```html
<meta name="canonical-account-key" content="verified-user-sub">
<script type="module" src="/app-assets/app.js"></script>
```

Alternatively, call `window.bootstrapCanonicalSync({ accountKey })`. A
`getAccessToken` callback may be supplied for bearer-token APIs; otherwise
same-origin session cookies are used.

The Maud application shell declares `hx-ext="ws" ws-connect="/ws"`, so HTMX
owns the dashboard socket and its reconnect lifecycle. The bundle intercepts
typed sync frames before HTMX attempts an HTML swap and wakes the pull loop.
The sync engine keeps its own socket implementation as a fallback when it is
embedded outside that shell. In both cases frames are hints only; durable state
always comes from `GET /api/v1/sync/changes`.
