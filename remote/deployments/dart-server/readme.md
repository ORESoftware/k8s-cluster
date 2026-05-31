# `remote/deployments/dart-server` — `dd-dart-server`

Full-stack Dart deployment for the dd-next cluster.

A single Dart binary serves:

| Path                    | Role                                                                        |
| ----------------------- | --------------------------------------------------------------------------- |
| `GET /healthz`          | Liveness probe.                                                             |
| `GET /readyz`           | Readiness probe.                                                            |
| `GET /metrics`          | Prometheus exposition (counters + gauges + latency histograms).             |
| `GET /`, `/dart`        | 301 → `/dart/pages`.                                                        |
| `GET /dart/pages`       | Jaspr SSR home page.                                                        |
| `GET /dart/pages/*`     | Jaspr SSR routed pages (`/about`, `/architecture`, `/wss`, `/hot-reload`).  |
| `GET /dart/wss`         | WebSocket upgrade. Spawns a per-connection isolate session.                 |
| `GET /dart/app`         | Flutter web SPA (`index.html`).                                             |
| `GET /dart/app/*`       | Flutter web SPA assets (with index.html SPA fallback).                      |
| `GET /dart/mobile`      | Mobile-optimized Flutter web bundle (`index.html`).                         |
| `GET /dart/mobile/*`    | Mobile-optimized Flutter web bundle assets (with index.html SPA fallback).  |
| `GET /dart/assets/*`    | Same physical bundle as `/dart/app/*`, exposed under a stable `/assets/` URL.|
| `GET /dart/admin/hot-reload-status` | JSON status (only when `HOT_RELOAD=true`).                       |
| `GET\|POST /dart/admin/reload`      | Trigger hot reload across every isolate (only when `HOT_RELOAD=true`). |
| `GET /dart/admin/db`                | pg-defs contract surface + `select now()` ping (only when `DATABASE_URL` is set). |
| `GET /dart/admin/db/conversations`  | Sample query against `presence_convs` via the pg-defs `Row.fromJson` factory.  |

The architecture is intentionally Phoenix-shaped: each connected WebSocket
peer maps 1:1 to a fresh Dart `Isolate`, and a `:pg`-style cross-isolate
EventBus on the main isolate fans messages out across sessions.

> **Why a Dart Phoenix?** Static typing end-to-end, one language for server
> + SSR + SPA + iOS + Android, real hot deploy via the VM service, no JS
> framework treadmill, HTMX/WS as the wire protocol, Flutter for every
> rich client surface. The full pitch (with comparison tables and
> reproducible benchmark numbers) is on the
> [`/dart/pages/about`](#) public page rendered by Jaspr.

---

## Repo layout

```
remote/deployments/dart-server/
├── pubspec.yaml                 # server pubspec (Dart 3.10, jaspr, rxdart)
├── analysis_options.yaml
├── readme.md
├── Dockerfile                   # multi-stage: flutter → dart compile → debian-slim
├── .dockerignore
├── bin/
│   └── server.dart              # process entrypoint: HTTP + WSS routing
├── lib/
│   ├── server/
│   │   ├── event_bus.dart            # :pg-style topic registry (cross-isolate)
│   │   ├── isolate_session.dart      # body of each per-connection isolate
│   │   ├── session_supervisor.dart   # spawn/teardown + frame pump + wiring
│   │   ├── presence.dart             # userId ↔ sessionId bidirectional index
│   │   ├── conversation_registry.dart# conversations + members + recent-msgs cache
│   │   ├── in_memory_cache.dart      # generic TTL + LRU cache primitive
│   │   ├── hot_reloader.dart         # VM-service driven hot reload (JIT only)
│   │   ├── metrics.dart              # tiny Prometheus counter/gauge store
│   │   ├── postgres.dart             # PgPool wrapper + column-name normaliser
│   │   ├── static_files.dart         # MIME-aware static file server
│   │   └── wss_components.dart       # Jaspr StatelessComponents for every HTMX OOB fragment
│   ├── db/
│   │   ├── pg_contract.dart          # single import site for dd_pg_defs (re-exports + assertion)
│   │   └── presence_convs_repo.dart  # example repo using pg-defs SelectSql + Row.fromJson
│   ├── jaspr/
│   │   ├── render.dart          # `renderJasprPage(route)` thin wrapper
│   │   ├── layout.dart          # `<head>` + nav + inline CSS
│   │   └── pages.dart           # all SSR pages (Home, About, Architecture, WssDemo)
│   └── shared/
│       ├── wire_messages.dart   # Inbound/Outbound/Bus message sealed classes
│       └── htmx_fragments.dart  # HTMX inbound JSON parser (typed HtmxInbound)
├── flutter_app/
│   ├── pubspec.yaml             # Flutter web app (RxDart-driven shell)
│   ├── analysis_options.yaml
│   ├── web/
│   │   ├── index.html           # `/dart/app/index.html`, base href `/dart/app/`
│   │   └── manifest.json        # PWA manifest
│   └── lib/
│       ├── main.dart            # Material shell + Stream-driven cards
│       └── wss_client.dart      # speaks the HTMX/WS protocol; RxDart subjects
├── flutter_mobile_app/
│   ├── pubspec.yaml             # mobile-shaped Flutter web bundle (separate project)
│   ├── analysis_options.yaml
│   ├── web/
│   │   ├── index.html           # `/dart/mobile/index.html`, base href `/dart/mobile/`
│   │   └── manifest.json        # PWA manifest
│   └── lib/
│       └── main.dart            # one-column landing list + stub /dart/wss connect button
├── k8s/ec2/
│   ├── dd-dart-server.deployment.yaml
│   ├── dd-dart-server.service.yaml
│   └── kustomization.yaml
├── tools/
│   ├── http_loadtest.dart      # zero-dep HTTP load tester (req/s + p50/p95/p99)
│   └── wss_loadtest.dart       # zero-dep WSS load tester (msg/s + first-frame latency)
└── scripts/
    ├── build-and-run.sh         # in-pod build (matches akka/billing pattern)
    ├── dev.sh                   # local JIT runner with hot reload enabled
    └── bench.sh                 # drives http_loadtest + wss_loadtest, writes bench-results.json
```

---

## Architecture

### Per-connection isolates (Phoenix-style)

Every accepted WebSocket spawns a fresh `Isolate`. The supervisor on the
main isolate creates four `ReceivePort`s per session:

```
                ┌────────────── handshake ──────────────┐
main isolate    │                                       │
                │     spawn → Isolate.spawn(...)        │
                │                                       │
                ↓                                       ↓
         WebSocket                              session isolate
         (HTTP upgrade)                         (private RxDart graph)
              │  inbound                                │
              │ ──────► InboundText / InboundBinary ───►│
              │                                         │
              │ ◄──── OutboundText (HTMX fragment) ─────│
              │ ◄──── OutboundBinary ───────────────────│
              │ ◄──── OutboundClose / MetricEvent ──────│
              │                                         │
              │      ┌───── exit / error ports ─────────┤
              ↓      ↓                                  │
         teardown ◄──┘                                  │
                                                        │
        ┌────────── pg-style EventBus on main ──────────┘
        │
        │  BusJoin / BusLeave / BusPublish (out)
        │  BusDelivery (in)
        └──────────► fanout to other sessions' mailboxes
```

Killing a session isolate is contained: `errorsAreFatal: true` makes
unhandled exceptions terminate the worker, the supervisor observes the
exit port, and tears down the WebSocket. Nothing else in the process is
affected.

### `:pg`-style EventBus

The bus models Erlang's [`:pg`](https://www.erlang.org/doc/man/pg)
process-group registry. It lives on the main isolate (the only place we
can fan messages out to multiple session SendPorts) and exposes:

```dart
register(sessionId, mailbox);    // called by supervisor on adopt
unregister(sessionId);           // called on teardown
join(sessionId, topic);          // member of `topic`
leave(sessionId, topic);         // remove
publish(topic, kind, data, fromSessionId, includeSelf);
```

Sessions never address each other directly. They issue:

* `BusJoin('lobby')` to subscribe (idempotent).
* `BusPublish(topic: 'lobby', kind: 'chat.say', data: {...})` to broadcast.
* `BusLeave('lobby')` to unsubscribe.

The bus enqueues a `BusDelivery` envelope onto every joined session's
mailbox `SendPort`. Topology stays star-shaped (every session ↔ bridge),
which is the only topology Dart isolates can actually pump frames over.

Three well-known topics are auto-joined by every session:

| Topic       | Source                                             | What rides on it                                   |
| ----------- | -------------------------------------------------- | -------------------------------------------------- |
| `lobby`     | sessions (HTMX `say` trigger)                      | `chat.say`, `chat.system`                          |
| `presence`  | supervisor (system events)                         | `presence.identified`, `presence.session_left`     |
| `conv-list` | supervisor (system events)                         | `conv.created`, `conv.user_joined`, `conv.bumped`  |

Per-conversation topics use `conv:<conversationId>` and are joined on
demand via `ConversationJoin`.

### Presence index

`lib/server/presence.dart` keeps:

```
userId      → Set<sessionId>     // who's online
sessionId   → userId              // reverse map
userId      → displayName         // friendly label
```

Each session is auto-bound to a synthetic `anon-<sessionId>` user on
adopt so every code path can treat presence as always-populated. Sessions
can rebind themselves by sending `Identify(userId, displayName)`; the
supervisor swaps the binding atomically and broadcasts
`presence.identified` on the `presence` topic so every other session can
re-render their UI.

The index is observable: `Presence.changes` emits `PresenceChange`
events, used by tests and the metrics gauges.

### ConversationRegistry

`lib/server/conversation_registry.dart` keeps:

```
conversationId → ConversationMeta            // id, title, kind, counts, timestamps
conversationId → Set<userId>                  // members
userId         → Set<conversationId>          // reverse index
conversationId → List<ConversationMessage>    // bounded LRU+TTL cache
```

The recent-messages cache is backed by [InMemoryCache](#inmemorycache),
defaulting to "last 32 messages, 24h TTL, 1024 distinct conversations".
This is **not** durable storage — it's a hot-path cache that survives
across reconnects but doesn't outlive the process. Pair it with NATS or
Postgres outbox if you need persistence.

User-level vs. session-level membership:

| Action                          | User-level        | Bus-level           |
| ------------------------------- | ----------------- | ------------------- |
| `ConversationJoin(c)`           | add userId once   | bus.join this sid   |
| `ConversationLeave(c)`          | unchanged         | bus.leave this sid  |
| `ConversationLeave(c, drop=1)` *and last session of user is gone* | remove userId  | bus.leave this sid  |
| `ConversationDelete(c)`         | drop everyone     | drop topic          |

This split lets a user keep their conversation memberships across
reconnects (or across multiple browser tabs) without manually re-joining.

### InMemoryCache

`lib/server/in_memory_cache.dart` is a tiny generic primitive:

* TTL eviction (default + per-entry overrides), with a periodic sweep timer.
* Optional capacity bound with LRU eviction (`get` bumps a key to the tail).
* Observable `Stream<CacheEvent>` for hits / misses / puts / evicts / expires.
* Hit/miss/evict/expire counters exposed for `/metrics`.

Used by the conversation registry for recent messages; you can also use
it for short-lived per-user state, recent-presence rosters, or the like.

### HTMX wire format

The `WsRoutes` are entirely framework-free for the browser: HTMX 2.x +
the `ws` extension drive the connection.

**Outbound (server → browser).** Every HTML fragment is produced by a
real **Jaspr `StatelessComponent`** in `lib/server/wss_components.dart`,
not by string concatenation. Each fragment is wrapped by `OobWrap`,
which renders an `hx-swap-oob` div HTMX uses to pick its target slot:

```dart
class Counter extends StatelessComponent {
  const Counter(this.value);
  final int value;

  @override
  Iterable<Component> build(BuildContext context) sync* {
    yield OobWrap(
      targetId: 'live-counter',
      child: div(classes: 'counter', [
        span(classes: 'value', [text('$value')]),
        form(
          attributes: const {'ws-send': ''},
          [button([text('bump')], attributes: const {'name': 'bump', 'value': '1'})],
        ),
      ]),
    );
  }
}
```

The session isolate hands the component to `renderFragment(...)` —
which lazily inits Jaspr on the current isolate and runs
`renderComponent(c, standalone: true)` — and ships the resulting
HTML over the WebSocket. This gives us:

* **automatic escaping** — `text(name)` and attribute values are
  escaped by Jaspr's renderer; no manual `htmlEscape` callsites left
  in the codebase,
* **composable panels** — `IdentityPanel`, `ConvList`, `ConvPanel`,
  `LobbyPanel`, etc. are testable in isolation, and
* **one mental model** — the same component model drives both the
  `/dart/pages/*` SSR pages and the `/dart/wss` HTMX fragments.

The rendered fragment looks like:

```html
<div id="live-counter" hx-swap-oob="innerHTML">
  <div class="counter">
    <span class="value">7</span>
    <form ws-send><button name="bump" value="1">bump</button></form>
  </div>
</div>
```

**Inbound (browser → server).** HTMX serialises `ws-send` forms into
JSON with a `HEADERS` discriminator:

```json
{
  "text": "hello world",
  "HEADERS": {
    "HX-Request": "true",
    "HX-Trigger-Name": "say",
    "HX-Trigger": "say"
  }
}
```

`lib/shared/htmx_fragments.dart` parses this into a typed `HtmxInbound`
object and the session pattern-matches on `triggerName` (`bump`, `reset`,
`echo`, `say`).

### RxDart on both sides

**Server.** Each session isolate keeps its own subject graph:

```dart
final _counter = BehaviorSubject<int>.seeded(0);
final _history = BehaviorSubject<List<String>>.seeded(const []);
final _lobby   = BehaviorSubject<List<Map<String, Object?>>>.seeded(const []);
final _inbound    = PublishSubject<HtmxInbound>();
final _busInbound = PublishSubject<BusDelivery>();
```

These subjects fold inbound events into HTML fragments which the
supervisor pushes to the WS peer. Subscriptions are torn down in
`_dispose`.

**Client (Flutter web).** `wss_client.dart` exposes the same state as
`BehaviorSubject<int>`, `BehaviorSubject<List<String>>`,
`BehaviorSubject<List<LobbyEntry>>`, etc. Material widgets observe via
`StreamBuilder`. The Flutter shell speaks the *same* HTMX protocol as
the SSR demo page so the server only has to render fragments once.

### Postgres via `pg-defs`

The canonical Postgres schema for the cluster is owned by
[`remote/libs/pg-defs/schema/schema.sql`](../../libs/pg-defs/schema/schema.sql)
and a generator emits per-language adapters under
`remote/libs/pg-defs/generated/`. dd-dart-server consumes the Dart
adapter (`generated/dart`, package `dd_pg_defs`) the same way every
other Dart-flavoured service in this monorepo does — as a `path:`
dependency from `pubspec.yaml`:

```yaml
dependencies:
  postgres: ^3.5.11
  dd_pg_defs:
    path: ../../libs/pg-defs/generated/dart
```

Three layers wire it into the server:

1. **`lib/server/postgres.dart`** — `PgPool` thin wrapper around
   `package:postgres`'s `Pool.withUrl(...)`. Adds `selectRows<T>`
   (`Sql.named`-aware), `execute`, `withTransaction`, lifecycle
   metrics, and — critically — `normalisePgColumnMap`, which converts
   the snake_case + `_json`-suffixed column names that pg-defs
   `*SelectSql` strings emit into the camelCase keys the generated
   `*Row.fromJson` factories expect.

2. **`lib/db/pg_contract.dart`** — single import site for the contract
   surface, mirroring the role of [`rest-api-rs/src/pg_contract.rs`](../rest-api-rs/src/pg_contract.rs).
   Re-exports the table name + select-SQL constants, declares
   `localReadableTables` / `localWritableTables`, and provides
   `assertPgContract()` which is called once from `main()` so a
   schema regen that drops a referenced table fails fast.

3. **`lib/db/presence_convs_repo.dart`** — example repo built on the
   `*SelectSql` constants. Reads `presence_convs`, `presence_conv_members`,
   `presence_events` (the cross-pod outbox table), and
   `presence_consumer_checkpoints`, decoding each row through the
   pg-defs `Row.fromJson` factory and validating with the
   regex / enum / length checks that come for free from the schema.

Postgres is opt-in: when `DATABASE_URL` (or `RDS_DATABASE_URL`,
`AGENT_TASKS_RDS_DATABASE_URL`) is unset, the pool isn't created and
`/dart/admin/db` reports `enabled: false`. The rest of the server —
WSS, SSR, hot reload, in-memory `Presence` and `ConversationRegistry`
— still boots normally. This is the same shape `rest-api-rs` uses,
chosen so local laptops can run the WSS + SSR demo without an RDS.

```sh
# happy path (DATABASE_URL set)
curl -s http://localhost:8089/dart/admin/db | jq .
# {
#   "enabled": true,
#   "ping": { "ok": true, "duration_ms": 4, "now_utc": "2026-05-22T22:08:01" },
#   "contract": { "exported": [...], "readable": [...], "writable": [...] },
#   "metrics": { "queries": 7, "queryErrors": 0, "rowsRead": 12 }
# }

curl -s 'http://localhost:8089/dart/admin/db/conversations?limit=5' | jq .
```

The contract assertion runs at boot, before the HTTP server binds, so
a `dd_pg_defs` regen that drops a table we depend on fails the pod
with a descriptive `StateError` instead of surfacing as a runtime
SQL error against a live database.

### Hot reload (Phoenix-style code swap, in-process)

**Yes — this is real hot reload, not "restart the dyno".** Dart server
processes can hot-load new code while running, without dropping in-flight
WebSockets, RxDart subscriptions, the EventBus, the conversation cache,
or any other in-memory state. This is the same VM Service Protocol that
Flutter uses for hot reload, exposed via the
[`vm_service` package](https://pub.dev/packages/vm_service) and called
from inside our own process.

```
┌──────────────────────────── one Dart process ────────────────────────────┐
│                                                                          │
│   main isolate ───────────────── isolate group ────────── session isos   │
│   ─────────────                   ─────────────              ─────       │
│   HTTP routing                    every Isolate.spawn        N alive     │
│   EventBus / Presence             stays in the main          WebSockets  │
│   ConversationRegistry            isolate's group            unchanged   │
│   HotReloader  ─────────► reloadSources(anyIsolateId, ...)               │
│        ▲                                                                 │
│        │                                                                 │
│   PollingDirectoryWatcher  ◄── lib/, bin/   (fs change events)           │
│        ▲                                                                 │
│        │                                                                 │
│   /dart/admin/reload  (manual trigger via curl / button)                 │
│                                                                          │
└──────────────────────────────────────────────────────────────────────────┘
```

`reloadSources(isolateId)` reloads source for **every** isolate in the
same isolate group as the target. Because session isolates are spawned
via `Isolate.spawn` from the main isolate, they share an isolate group
with it — so a single reload call covers every active WebSocket session
at once. The HotReloader picks one isolate per group as the target so we
do exactly one round-trip per group instead of one per isolate.

**What gets picked up immediately**

| Change                                                | Active sessions see it? |
| ----------------------------------------------------- | ----------------------- |
| New body in `_renderCounter` / `_renderConvPanel`     | ✅ at next frame emit   |
| New HTMX trigger handler in `_handleHtmxTrigger`      | ✅ at next inbound msg  |
| New top-level helper in `lib/server/...`              | ✅ at next call site    |
| New RxDart pipeline node added to `_wirePipelines`    | ⚠️ on next session      |
| New file under `lib/`                                 | ✅ once imported        |
| New Jaspr page in `lib/jaspr/pages.dart::pickPage`    | ✅ at next page render  |

**What's preserved across a reload**

* All open WebSockets — peers see no disconnect, no extra `Hello!` greet.
* `BehaviorSubject` values + subscription topology in every session.
* Counter values, echo history, lobby chat, conversation membership,
  recent-messages cache.
* EventBus topic membership map, Presence index, ConversationRegistry
  rows.
* Process-wide metric counters (gauges keep ticking through the reload).

**What CANNOT be hot-reloaded**

* AOT binaries (`dart compile exe`) — there's no JIT in the runtime, so
  there's nothing to swap. Run with `dart run --enable-vm-service`
  (DEV_MODE=true) for hot reload; AOT is the production path.
* Class-shape changes: adding/removing fields, changing supertype,
  changing `const`-ness. The VM rejects these; the reloader records the
  failure in `byIsolate` and keeps running.
* Static initialisers / top-level `final` bindings — their values are
  preserved as-is. To re-evaluate, restart.

**How to use it locally**

```bash
cd remote/deployments/dart-server
scripts/dev.sh                    # JIT + watcher + VM-service in-process
# in another shell:
curl -s localhost:8089/dart/admin/hot-reload-status | jq .
# edit lib/server/isolate_session.dart, save — reload fires in <1s
```

**How to use it in-cluster**

Set `DEV_MODE=true` (or `HOT_RELOAD=true`) on the `dd-dart-server`
Deployment and re-roll. The pod boots `dart run --enable-vm-service`
instead of `dart compile exe`, the watcher mounts onto the hostPath
repo, and `git pull` on the EC2 host plus `kubectl exec ... curl
localhost:8089/dart/admin/reload` triggers a hot reload — no pod
restart, no WebSocket churn. Production keeps `DEV_MODE=false` so the
binary path stays AOT.

**Failure modes**

* If the change *would* require a class-shape reload, the JSON result
  has `success: false` and `byIsolate.<name>` carries the VM's
  failure reason. Restart the pod or revert.
* If the watcher misses a save (rare, polling at 250ms), `POST
  /dart/admin/reload` is the manual override.
* `force=1` query param tells the VM to reload even if mtimes claim
  nothing changed.

`dart_hot_reload_*` counters and `dart_hot_reload_last_ms` gauge expose
the reload pipeline to Prometheus.

### Service worker / offline

`flutter build web --pwa-strategy=offline-first` emits
`flutter_service_worker.js` with a fingerprinted asset list. The file is
copied into `public/` and served with `Cache-Control: no-cache` so the
client always picks up a new SW; everything else (fingerprinted
`main.dart.js`, `canvaskit/`, `assets/...`) is served as
`max-age=31536000, immutable`.

The HTMX vendor scripts (`htmx.min.js`, `htmx-ext-ws.min.js`) live under
`/dart/app/vendor/` so the SW can cache them — visiting `/dart/app` once
is enough for the SPA to work offline thereafter.

---

## Endpoints in detail

### `GET /dart/pages` & `GET /dart/pages/*`

Server-side rendered with [Jaspr 0.23](https://pub.dev/packages/jaspr).
`lib/jaspr/render.dart::renderJasprPage` invokes `Jaspr.initializeApp()`
once on startup and calls `renderComponent(...)` per request. The
`build_runner` step generates `lib/jaspr_options.dart` so
`Jaspr.initializeApp()` resolves a no-op default options object even
though we do not use any `@client` annotated components.

Pages:

* `/` — home, links into the rest.
* `/about` — stack summary.
* `/architecture` — the diagram + commentary above.
* `/wss` — pure-HTMX WSS demo. Loads `htmx.org@2.0.6` + `htmx-ext-ws@2.0.4`
  from a CDN.

### `GET /dart/wss`

WebSocket upgrade. The bridge:

1. Generates a 7-char base36 `sessionId`.
2. Spawns a session isolate via `SessionSupervisor.adopt`.
3. Performs the `SendPort` handshake.
4. Sends a `SessionBootMessage`.
5. Pumps WS ↔ session frames until either side closes.

Supported HTMX triggers:

| Trigger name      | Fields                                          | Effect                                                                    |
| ----------------- | ----------------------------------------------- | ------------------------------------------------------------------------- |
| `bump`            | —                                               | `_counter += 1`; re-render `#live-counter`.                               |
| `reset`           | —                                               | `_counter = 0`; re-render `#live-counter`.                                |
| `echo`            | `message`                                       | Append to per-session history; re-render.                                 |
| `say`             | `text`                                          | `BusPublish` to `lobby`; every joined session sees a delivery.            |
| `identify`        | `user_id`, `display_name`                       | Rebind session in [Presence]; broadcast `presence.identified`.            |
| `open-conv`       | `conversation_id`, `title`, `kind`              | Upsert conversation in [ConversationRegistry]; broadcast `conv.created`.  |
| `join-conv`       | `conversation_id`                               | Add user as member; bus.join `conv:<id>`; broadcast `conv.user_joined`.   |
| `leave-conv`      | `conversation_id`, `drop`                       | bus.leave; optionally drop user-level membership.                         |
| `say-conv`        | `conversation_id`, `text`                       | Append to recent-msgs cache; broadcast `conv.message` on `conv:<id>`.     |
| `switch-conv`     | `conversation_id`                               | Local-only: changes which conv the panel renders.                         |
| `delete-conv`     | `conversation_id`                               | Wipe registry + recent cache; broadcast `conv.deleted`.                   |

### `GET /dart/app` & `GET /dart/app/*`

Flutter web bundle from `flutter build web`. The base href is
`/dart/app/` so all relative URLs resolve against the bundle root. SPA
fallback returns `index.html` for any path that doesn't map to a real
file (the static server checks for path traversal first).

### `GET /dart/assets/*`

Same physical bundle, mounted at a stable `/dart/assets/` prefix. Public
SSR pages reference `/dart/assets/manifest.json` etc. so they don't
have to know the SPA's internal layout.

### `GET /dart/mobile` & `GET /dart/mobile/*`

Independent Flutter web bundle, built from the sibling `flutter_mobile_app/`
project with `flutter build web --base-href=/dart/mobile/`. Served from
`MOBILE_STATIC_DIR` (defaults: `./mobile-public` locally,
`/opt/dd-dart-server/mobile-public` in the Docker runtime image,
`/opt/dd-next-1/remote/deployments/dart-server/mobile-public` on the
EC2-mounted repo path).

The bundle is a tiny landing surface: a single-column list of the Jaspr
SSR pages with large tap targets, plus a stubbed "Connect to /dart/wss"
button. Real session adoption lives in `flutter_app/` for now; the
mobile bundle's job is to be a fast, viewport-locked entry point that
links across the deployment.

The Jaspr SSR layer at `/dart/pages/*` does **not** route through the
mobile bundle — mobile is owned entirely by the static handler at
`/dart/mobile/*`, and the SSR registry in `lib/jaspr/pages.dart` is
unchanged.

#### Mobile front-end — local dev

```bash
cd remote/deployments/dart-server/flutter_mobile_app
flutter pub get
flutter run -d chrome              # served at http://localhost:<random>/
# or, against the live server route:
flutter build web --release --base-href=/dart/mobile/
# then point STATIC_DIR/MOBILE_STATIC_DIR at build/web and run the server.
```

`scripts/build-and-run.sh` builds the mobile bundle alongside
`flutter_app/` and atomically swaps the result into `MOBILE_STATIC_DIR`,
so the in-cluster path is a no-op once you've pushed.

---

## Build

### Local Docker

```bash
# From the repo root.
docker build \
  -f remote/deployments/dart-server/Dockerfile \
  -t dd-dart-server:dev .

docker run --rm -p 8089:8089 dd-dart-server:dev
# open http://localhost:8089/dart/pages
```

### Local dev (JIT + hot reload)

```bash
cd remote/deployments/dart-server
scripts/dev.sh
# JIT mode; PollingDirectoryWatcher on lib/ + bin/; reload in <1s on save.
```

The same script runs in-cluster when the Deployment has `DEV_MODE=true`.

### Benchmarks

`tools/http_loadtest.dart` and `tools/wss_loadtest.dart` are dependency-free
load testers. `scripts/bench.sh` drives both against a running server and
writes a JSON results file you can pipe through `jq` or feed into Datadog.

```bash
cd remote/deployments/dart-server
# Start the server first (scripts/dev.sh in another shell, or AOT binary).
scripts/bench.sh                 # default: 30s, 32 HTTP conns + 128 WSS conns
BENCH_DURATION=120 scripts/bench.sh
BENCH_HOST=10.0.0.7 scripts/bench.sh

cat bench-results.json | jq '.[] | {kind, rps, send_rps, recv_rps, latency, first_frame_latency}'
```

The `/dart/pages/about` page documents representative numbers from the
same harness; reproduce them on your hardware to fill the page in with
your own measurements.

### In-cluster (EC2 host-mounted repo)

```bash
cd remote/deployments/dart-server
scripts/build-and-run.sh
# Reads HTTP_HOST/HTTP_PORT/STATIC_DIR/DEV_MODE/HOT_RELOAD from env.
```

The Kubernetes pod runs `scripts/build-and-run.sh` from the EC2-mounted
repo at `/opt/dd-next-1`, so a `git pull` on the host plus a
`kubectl rollout restart deployment/dd-dart-server` is enough to deploy
new code. Cargo-style cache anchoring is handled with hostPath volumes
for `~/.pub-cache` and `/opt/flutter/bin/cache`.

---

## Metrics

Exposed at `GET /metrics` in Prometheus exposition format. Counters
prefixed `dart_*`:

| Metric                                      | Type    | Source                                     |
| ------------------------------------------- | ------- | ------------------------------------------ |
| `dart_http_requests_total`                  | counter | every accepted HTTP request                |
| `dart_http_404_total`                       | counter | route fallback                             |
| `dart_pages_rendered_total`                 | counter | Jaspr SSR success                          |
| `dart_pages_render_error_total`             | counter | Jaspr SSR failure                          |
| `dart_app_requests_total`                   | counter | `/dart/app/*` requests                     |
| `dart_mobile_requests_total`                | counter | `/dart/mobile/*` requests                  |
| `dart_assets_requests_total`                | counter | `/dart/assets/*` requests                  |
| `dart_wss_upgrade_total`                    | counter | WS upgrade requests                        |
| `dart_sessions_spawned_total`               | counter | isolates ever spawned                      |
| `dart_sessions_opened_total`                | counter | isolates that completed boot               |
| `dart_sessions_closed_total`                | counter | clean session shutdown                     |
| `dart_sessions_teardown_total`              | counter | supervisor teardown (any cause)            |
| `dart_sessions_spawn_failed_total`          | counter | `Isolate.spawn` errors                     |
| `dart_sessions_ws_error_total`              | counter | WS-level errors during a session           |
| `dart_sessions_isolate_error_total`         | counter | unhandled exceptions inside a session      |
| `dart_session_bumps_total`                  | counter | `bump` HTMX trigger fired                  |
| `dart_session_resets_total`                 | counter | `reset` HTMX trigger fired                 |
| `dart_session_echoes_total`                 | counter | `echo` HTMX trigger fired                  |
| `dart_session_says_total`                   | counter | `say` HTMX trigger fired (bus publish)     |
| `dart_session_lobby_deliveries_total`       | counter | `BusDelivery` for `lobby/chat.say`         |
| `dart_eventbus_register_total`              | counter | sessions registered with the bus           |
| `dart_eventbus_unregister_total`            | counter | sessions unregistered                      |
| `dart_eventbus_join_total`                  | counter | `BusJoin` accepted                         |
| `dart_eventbus_leave_total`                 | counter | `BusLeave` accepted                        |
| `dart_eventbus_publish_total`               | counter | `BusPublish` accepted                      |
| `dart_eventbus_publish_empty_total`         | counter | publish to a topic with no joiners         |
| `dart_eventbus_delivered_total`             | counter | individual `BusDelivery`s actually sent    |
| `dart_presence_identify_total`              | counter | `Identify` outbound frames accepted        |
| `dart_conv_created_total`                   | counter | conversations created                      |
| `dart_conv_deleted_total`                   | counter | conversations deleted                      |
| `dart_conv_join_total`                      | counter | `ConversationJoin` calls                   |
| `dart_conv_leave_total`                     | counter | `ConversationLeave` calls                  |
| `dart_conv_message_total`                   | counter | `ConversationSay` accepted                 |
| `dart_session_conv_deliveries_total`        | counter | per-conv `BusDelivery`s a session received |
| `dart_sessions_live` (gauge)                | gauge   | currently-running session isolates         |
| `dart_eventbus_topics` (gauge)              | gauge   | distinct non-empty topics                  |
| `dart_eventbus_sessions` (gauge)            | gauge   | sessions registered with the bus           |
| `dart_eventbus_total_joins` (gauge)         | gauge   | sum of joiners across all topics           |
| `dart_presence_users` (gauge)               | gauge   | distinct online users                      |
| `dart_presence_sessions` (gauge)            | gauge   | session bindings in [Presence]             |
| `dart_conversations` (gauge)                | gauge   | conversations in the registry              |
| `dart_conversation_memberships` (gauge)     | gauge   | sum of memberships across conversations    |
| `dart_conversation_recent_cache_size` (g)   | gauge   | live entries in recent-messages cache      |
| `dart_conversation_recent_cache_hits` (g)   | gauge   | cumulative cache hits                      |
| `dart_conversation_recent_cache_misses` (g) | gauge   | cumulative cache misses                    |
| `dart_conversation_recent_cache_evicts` (g) | gauge   | cumulative LRU evictions                   |
| `dart_conversation_recent_cache_expires` (g)| gauge   | cumulative TTL expirations                 |
| `dart_hot_reload_attempt_total`             | counter | every `reloadAll` invocation               |
| `dart_hot_reload_success_total`             | counter | reload calls where every group succeeded   |
| `dart_hot_reload_failure_total`             | counter | reload calls where ≥1 group failed         |
| `dart_hot_reloads_total` (gauge)            | gauge   | cumulative reload count (mirrors counter)  |
| `dart_hot_reloads_failed_total` (gauge)     | gauge   | cumulative failed reload count             |
| `dart_hot_reload_last_ms` (gauge)           | gauge   | wall-clock duration of the most recent reload |
| `dart_pg_queries_total` (gauge)             | gauge   | cumulative pg-defs / pool queries          |
| `dart_pg_query_errors_total` (gauge)        | gauge   | cumulative pg query failures               |
| `dart_pg_rows_read_total` (gauge)           | gauge   | cumulative rows decoded into pg-defs Row classes |
| `dart_pg_connections_opened_total` (gauge)  | gauge   | pool open count (idempotent if already open) |
| `dart_pg_connections_closed_total` (gauge)  | gauge   | pool close count                           |
| `dart_pg_notify_events_total` (gauge)       | gauge   | LISTEN/NOTIFY events received (when wired) |

### Isolate-pool autotuner + latency telemetry

The coordinator also exposes the metrics that drive (and observe) the MDP
isolate-pool autotuner. Counters are folded from every shard; gauges are
summed/aggregated at scrape time; histograms are folded from per-shard
`ObserveEvent`s into one pod-wide distribution.

| Metric                                       | Type      | Source                                                        |
| -------------------------------------------- | --------- | ------------------------------------------------------------- |
| `dart_ws_adopt_latency_seconds`              | histogram | acquire/spawn-a-host + attach, per accepted WS                |
| `dart_ws_first_frame_latency_seconds`        | histogram | attach → first outbound frame written to the socket           |
| `dart_session_cold_start_spawns_total`       | counter   | host spawned on a connection's hot path (no warm host free)   |
| `dart_sessions_refused_capacity_total`       | counter   | connection shed (1013) because the pool hit its hard ceiling  |
| `dart_session_hosts_prewarmed_total`         | counter   | hosts pre-spawned off the hot path by the reconciler          |
| `dart_session_hosts_retired_total`           | counter   | idle hosts gracefully retired toward target                   |
| `dart_pool_autotuner_ticks_total`            | counter   | control-loop iterations                                       |
| `dart_pool_optimizer_ok_total`               | counter   | `dd-mdp-optimizer` recommendations applied (remote mode)      |
| `dart_pool_optimizer_miss_total`             | counter   | optimizer unreachable/unmappable; held setpoint (remote mode) |
| `dart_pool_idle_hosts` (gauge)               | gauge     | empty live hosts (over-provisioning cost)                     |
| `dart_pool_free_slots` (gauge)               | gauge     | free session slots across live hosts                          |
| `dart_pool_target_hosts` (gauge)             | gauge     | sum of per-shard warm-pool targets                            |
| `dart_pool_target_hosts_global` (gauge)      | gauge     | coordinator's chosen pod-wide host-isolate target             |
| `dart_pool_target_density` (gauge)           | gauge     | coordinator's chosen per-host session cap (density action)    |
| `dart_sessions_per_host_cap` (gauge)         | gauge     | live per-host density actually applied across shards          |
| `dart_pool_autotuner_mode` (gauge)           | gauge     | 0=off, 1=local, 2=remote                                       |
| `dart_pool_shield_enabled` (gauge)           | gauge     | 1 when the storm-dampening shield is active (`WS_MDP_SHIELD`)  |
| `dart_pool_shield_engaged_total`             | counter   | ticks where the shield clamped the policy's chosen directive   |
| `dart_pool_shield_max_hosts` (gauge)         | gauge     | pod-wide host-isolate budget the shield enforces (memory governor) |
| `dart_pool_autotuner_epsilon` (gauge)        | gauge     | current ε-greedy exploration rate (local mode)                |
| `dart_pool_autotuner_reward_ema` (gauge)     | gauge     | EMA of the per-tick reward (local mode)                       |
| `dart_pool_autotuner_updates` (gauge)        | gauge     | Q-learning updates applied (local mode)                       |
| `dart_pool_autotuner_states_visited` (gauge) | gauge     | distinct state buckets seen (local mode)                      |

Prometheus scrapes these from the coordinator's `admin` port (`8088`) via
the `dd-dart-server` jobs in `remote/argocd/observability/{prometheus,
otel-collector}.configmap.yaml`. The **Dart WSS Runtime** Grafana dashboard
(`grafana.dashboards.configmap.yaml`, uid `dd-dart-wss-runtime`) renders the
pool target vs live/idle hosts, the adopt/first-frame latency quantiles, the
pool churn (cold starts / refusals / prewarm / retire), and the autotuner's
learning curve.

### Crash resilience

WebSocket sockets live on a **gateway-shard** isolate; the per-session
RxDart/Jaspr render logic runs on a **session-host** isolate. The blast
radius of a crash depends on which isolate dies, so each layer is hardened
to contain errors instead of propagating them to the VM:

* **Per session** — every render pipeline (`.listen(..., onError:)`), the
  HTMX/bus trigger handlers, the 1 Hz ticker, and `_emitFragment` catch and
  count their own failures (`dart_session_render_errors_total`,
  `dart_session_tick_errors_total`). A bad render drops one frame; the next
  state change re-renders. It can no longer escape the session.
* **Per session-host** — `hostIsolateEntry` runs inside a
  `runZonedGuarded`, and hosts are now spawned `errorsAreFatal: false`. A
  stray async error that slips a session is caught at the host boundary
  (rate-limited `session_host_uncaught_error` log) rather than killing the
  isolate and dropping all ~`sessionsPerHost` sessions on it. Genuine
  termination (OOM, explicit kill) still fires the supervisor's exit port,
  which closes each attached socket cleanly (1000) so clients reconnect.
* **Per gateway-shard** — `gatewayShardEntry` runs inside a
  `runZonedGuarded` (forwarding `dart_gateway_shard_uncaught_errors_total`),
  and the control-message + gauge-report callbacks are individually
  guarded so a malformed directive can't take down the shard's control
  plane (which also drives SIGTERM drain).
* **Shard self-heal** — if a shard isolate ever does exit unexpectedly
  (i.e. *not* during pod drain), the coordinator respawns a replacement
  after a 1 s backoff so the `SO_REUSEPORT` listener pool returns to full
  width (`dart_gateway_shards_respawned_total`, capped at 100 to avoid a
  crash loop). Established connections on sibling shards are unaffected;
  only new accepts rebalance across the briefly-narrower pool.

**Blast radius if an isolate is lost anyway**

| Isolate            | Connections affected                          | Recovery                                                            |
| ------------------ | --------------------------------------------- | ------------------------------------------------------------------- |
| session-host       | the host's sessions (≤ `sessionsPerHost`)     | supervisor closes each socket 1000 → clients reconnect; siblings OK |
| gateway-shard      | sessions the kernel routed to it (≈ total ÷ `WS_GATEWAY_SHARDS`) | coordinator respawns the shard; capacity self-restores              |
| HTTP isolate       | 0 WS (only `:8090` pages/admin/metrics)       | —                                                                   |
| main / coordinator | all (pod exits)                               | Kubernetes restarts the pod                                         |

| Metric                                        | Type    | Source                                                       |
| --------------------------------------------- | ------- | ------------------------------------------------------------ |
| `dart_session_render_errors_total`            | counter | a render/pipeline/handler threw in a session (frame dropped) |
| `dart_session_tick_errors_total`              | counter | the per-session 1 Hz ticker callback threw                   |
| `dart_gateway_shard_uncaught_errors_total`    | counter | an error reached a gateway shard's zone guard                |
| `dart_gateway_shards_respawned_total`         | counter | shards respawned by the coordinator after an unexpected exit |
| `dart_session_hosts_debug_crashed_total`      | counter | hosts hard-killed by the chaos hook (`WS_DEBUG_CRASH`)       |
| `dart_debug_crash_host_requests_total`        | counter | `POST /dart/admin/debug/crash-host` calls accepted           |

**Fault injection.** With `WS_DEBUG_CRASH=1`, `POST /dart/admin/debug/crash-host`
(admin port `8088`, non-public) makes the busiest live shard hard-kill one of
its session-host isolates. Use it under load to confirm empirically that only
that host's ≤ `sessionsPerHost` sockets close (cleanly, 1000) and reconnect,
while the shard, its sibling hosts, and the pod keep running. Off by default.

---

## MDP isolate-pool autotuner

> Behind `WS_MDP_MODE` (`off` by default). `off` keeps the original
> lazy-spawn-only supervisor; `local`/`remote` turn on the directive-driven
> warm pool and the coordinator control loop.

**Problem.** Each gateway shard lazily spawns a session-host isolate when no
warm host has a free slot, *on the accepting connection's hot path*. That
cold start is pure latency, and idle hosts only ever retire on crash — so the
pool both stalls under bursty arrivals and wastes memory after a trough. The
open question is the steady-state size: how many host isolates should the pod
keep warm to carry medium load toward 50K connections without paying
cold-start latency or over-provisioning?

**Approach.** Model it as a small MDP and learn the answer online:

* **State** — pool utilisation bucket × arrival-trend bucket
  (`liveSessions / (liveHosts × sessionsPerHost)` and the session-count
  delta).
* **Action** — a *joint* choice over two levers, decoded from one action
  index over the `WS_POOL_SIZE_LEVELS × WS_HOST_DENSITY_LEVELS` grid:
  1. the pod-wide host-isolate target from `WS_POOL_SIZE_LEVELS`
     (default `20,30,40,50`), and
  2. the per-host session **density** (`sessionsPerHost` cap) from
     `WS_HOST_DENSITY_LEVELS` (default `100,250,500,1000`) — how densely to
     pack sessions onto each isolate. Low density spreads load across more,
     quieter event loops (lower per-isolate contention, more base-heap
     overhead); high density packs fewer, busier isolates (cheaper RAM
     floor, higher tail latency under contention). So for the same offered
     load the learner can compare e.g. "40 hosts × 250/host" against
     "20 hosts × 1000/host" and keep whichever the reward prefers.
* **Reward** — `-(latency·w + coldStarts·w + refusals·w + idleHosts·w +
  size·w)`: the cheapest pool that keeps p99 adopt/first-frame latency low
  and cold-starts/refusals at zero wins. Density is optimised *implicitly*
  through this same reward — over-packing shows up as adopt/first-frame
  latency (per-isolate contention); under-packing shows up as extra hosts
  (cold-starts/refusals or idle/size cost).

The coordinator runs one control loop every `WS_MDP_CONTROL_INTERVAL_MS`. It
reads the aggregated telemetry above, asks the policy for a `(targetHosts,
sessionsPerHost)` decision, divides the host target across the live shards
(density is per-host, so it is broadcast unchanged), and pushes a
`ShardPoolDirective` to each. Every shard reconciles its pool toward the
per-shard host target — pre-spawning warm hosts off the hot path (up to
`WS_POOL_MAX_HOSTS_PER_SHARD`, never below `WS_POOL_MIN_WARM_HOSTS`) and
retiring hosts that have sat empty for `WS_POOL_RETIRE_COOLDOWN_MS` — and
adopts the new density cap for subsequent placements (existing sessions are
untouched).

**Storm-dampening shield (`lib/server/pool_shield.dart`).** ε-greedy
exploration over the *full* size × density grid will, by design, occasionally
pick a combination whose capacity is far below the current live load — e.g.
density `100` at 40K sessions, which would need ~500 host isolates pod-wide.
Broadcast verbatim, that pick collapses every warm host's free-slot count to
zero in one tick, forces a synchronized cold-start / `1013` refusal storm on
the accept hot path, and wedges the per-shard response loop (the failure seen
at the very top of the 50K load-test ramp). The shield clamps the **applied**
directive into the feasible, memory-bounded region *without* constraining what
the policy learns (the Q-table still records the raw choice; only the
broadcast setpoint is corrected — classic shielded RL):

1. **density-decrease slew** — density may fall at most `WS_MDP_DENSITY_MAX_DROP`
   per tick (default `0.5` = halve), so a big single-tick drop can't strand
   every host over the new cap at once. Rising density is unrestricted.
2. **density feasibility floor** — density is raised until the shield's
   pod-wide host budget (`WS_MDP_MAX_HOST_BUDGET`, default `192`) can hold the
   load; this floor is the memory governor (a smaller budget packs sessions
   onto fewer isolates).
3. **host floor** — given the shielded density, the host target is raised to
   hold the load plus `WS_MDP_CAPACITY_HEADROOM` (default `0.2`) spare.
4. **host ceiling** — the target never exceeds that budget.

The shield's budget is deliberately **decoupled** from the operator's hard
refusal ceiling (`WS_POOL_MAX_HOSTS_PER_SHARD`): the deployment runs the
ceiling at `0` (never shed connections), which on its own would leave density
unbounded and let a low-density exploration fork hundreds of host isolates.
The shield enforces the memory budget regardless, so the warm pool + density
stay memory-safe even with an unbounded refusal ceiling. (When the refusal
ceiling *is* set, the shield uses the smaller of the two.) The default
`WS_POOL_MAX_HOSTS_PER_SHARD` was also widened to the larger of the size-ladder
share and `ceil(192/shards)` so a finite ceiling, if configured, doesn't
re-introduce the refusal storm. `dart_pool_shield_engaged_total` counts the
ticks the shield bit; `dart_pool_shield_max_hosts` shows the active budget;
`dart_pool_target_density` shows the *post-shield* density.

**Two brains, same action set:**

* `local` — an in-process tabular Q-learner (`lib/server/pool_autotuner.dart`,
  ε-greedy, zero external deps). Self-contained and unit-tested; this is the
  one to use for the joint size × density experiment.
* `remote` — delegates to the cluster's `dd-mdp-optimizer` service
  (`POST /telemetry/learn`). It asks two concurrent ladders per tick —
  candidate actions `pool-20 … pool-50` for the host target and
  `density-100 … density-1000` for the per-host cap — and holds each lever's
  previous setpoint independently when the optimizer is unreachable or
  returns an unmappable action.

| Env var                         | Default                | Meaning                                            |
| ------------------------------- | ---------------------- | -------------------------------------------------- |
| `WS_MDP_MODE`                   | `off`                  | `off` / `local` / `remote`                          |
| `WS_POOL_SIZE_LEVELS`           | `20,30,40,50`          | discrete pod-wide host-isolate targets             |
| `WS_HOST_DENSITY_LEVELS`        | `100,250,500,1000`     | discrete per-host density caps (2nd lever; single value pins density) |
| `WS_MDP_CONTROL_INTERVAL_MS`    | `5000`                 | control-loop cadence                               |
| `WS_POOL_MIN_WARM_HOSTS`        | `1`                    | warm floor per shard                               |
| `WS_POOL_MAX_HOSTS_PER_SHARD`   | `max(ceil(max/shards)+2, ceil(192/shards))` | hard per-shard ceiling (0 = unbounded) |
| `WS_POOL_RETIRE_COOLDOWN_MS`    | `15000`                | idle dwell before retiring a host                  |
| `WS_MDP_SHIELD`                 | `true`                 | clamp the directive into the feasible region (storm-dampening) |
| `WS_MDP_MAX_HOST_BUDGET`        | `192`                  | pod-wide host-isolate budget the shield enforces (memory governor) |
| `WS_MDP_CAPACITY_HEADROOM`      | `0.2`                  | spare capacity the applied directive must provide above live load |
| `WS_MDP_DENSITY_MAX_DROP`       | `0.5`                  | max fraction density may fall per tick (0 disables slew) |
| `WS_MDP_OPTIMIZER_URL`          | `dd-mdp-optimizer:8096`| optimizer endpoint (remote mode)                   |
| `WS_MDP_{ALPHA,GAMMA,EPSILON,…}`| see `pool_autotuner.dart` | learner hyperparameters + reward weights        |
| `WS_DEBUG_CRASH`                | `off`                  | chaos hook: mounts `POST /dart/admin/debug/crash-host` (admin port) to hard-kill one session-host isolate; off by default |

> **Next increment — library-segmented pools.** The autotuner now tunes two
> levers (pool size × host density) over one homogeneous host pool. The planned
> follow-up adds 2–3 *typed* pools (`lite` / `render` / `data`) with per-kind
> host entrypoints so a benchmark/passthrough host never initialises Jaspr into
> its heap, and folds a pool-count action (`{2,3}`) into the same joint action
> index — exactly how the density lever was added to the size lever here. The
> action space and directive plumbing are built to extend to that without
> rework.

---

## Why this shape

* **Dart isolates instead of `async` actors.** Dart's `Isolate` is the only
  concurrency primitive that gives true isolation — separate heaps,
  independent GC, no shared mutable state. That maps cleanly onto BEAM's
  per-connection process model and gives us the same fault-isolation
  guarantee.

* **`:pg`-style bus instead of direct SendPort topology.** SendPorts can
  only be used by the isolate that owns the receiving `ReceivePort` — so
  N-to-N session communication has to go through the main isolate. The
  EventBus formalises that with topic-based routing.

* **HTMX over WebSocket instead of a JS framework.** HTML fragments are
  the wire format. The server already knows how to render HTML. The
  client doesn't have to be reconstructed in TS, doesn't need a virtual
  DOM, doesn't need a build step. The Flutter SPA is an opt-in rich
  surface, not a hard requirement.

* **Jaspr for SSR public pages.** Dart-native component model with the
  same SSR ergonomics as a JS-side framework, but it stays in the same
  toolchain as the rest of the deployment. No Node.

* **Flutter for the SPA.** Single language end-to-end. The Flutter web
  build produces a real PWA with a fingerprinted service worker, and
  RxDart on the client mirrors RxDart on the server.

* **Hot reload as a first-class feature.** Phoenix's
  [`code_swap`/`code_change`](https://hexdocs.pm/elixir/GenServer.html#c:code_change/3)
  story is one of the marquee benefits of the BEAM. Dart's VM Service
  Protocol gives us the same superpower: ship new render code, new
  HTMX handlers, new Jaspr pages to a running cluster without dropping
  any open WebSocket. JIT in dev/staging, AOT in prod — you opt into
  the trade-off via a single env var.

---

## Concurrency model & the road past 50K

The [Architecture](#per-connection-isolates-phoenix-style) section above gives
the *conceptual* unit — one WebSocket peer ↔ one private RxDart/Jaspr graph.
This section explains how that unit is actually laid out across isolates so a
**single pod** carries 50K concurrent sockets at ~2.77 GiB, and how the MDP/POMDP
autotuner is the lever that pushes the ceiling higher.

### Three nested layers of concurrency

A booted pod is not one isolate per connection — at 50K that would mean 50K
heaps and 50K event loops. Instead it is a three-layer tree, and each layer is
a *different* axis of parallelism:

```
                       ┌──────────────────────────────────────────────┐
   main / coordinator  │  routing decisions · :pg EventBus · Presence  │
   (1 isolate)         │  ConversationRegistry · MDP control loop      │
                       │  /metrics + admin (:8088) · shard self-heal   │
                       └───────────────┬──────────────────────────────┘
                                       │ spawns + ShardPoolDirective
              ┌────────────────────────┼────────────────────────┐
              ▼                        ▼                        ▼
        gateway shard 0          gateway shard 1   …      gateway shard N-1     ← WS_GATEWAY_SHARDS
        (SO_REUSEPORT :8089)     (SO_REUSEPORT)           (SO_REUSEPORT)           kernel hash-balances
        SessionSupervisor        SessionSupervisor        SessionSupervisor       new TCP accepts
              │                        │                        │
        ┌─────┴─────┐            ┌─────┴─────┐            ┌─────┴─────┐
        ▼           ▼            ▼           ▼            ▼           ▼
     host 0      host 1   …   host k     host k+1  …   host m     host m+1       ← warm host pool
     ≤ D sess.   ≤ D sess.     ≤ D sess.  ≤ D sess.     ≤ D sess.  ≤ D sess.        D = sessionsPerHost
     │ │ │ …      │ │ │ …       │ │ │ …    │ │ │ …        │ │ │ …    │ │ │ …          (density)
     each "│" = one session: private BehaviorSubjects + Jaspr render closure
```

| Layer | Count | Axis of parallelism it buys | Tuned by |
| ----- | ----- | --------------------------- | -------- |
| **Gateway shards** | `WS_GATEWAY_SHARDS` | **Accept + socket I/O throughput.** All shards `bind` the same port with `SO_REUSEPORT`; the kernel spreads inbound connections across them, so socket reads/writes run on N event loops in parallel. Scales with vCPUs. | static (set to ~#vCPU) |
| **Session-host isolates** | MDP `targetHosts` | **Fault isolation + GC parallelism.** A crash or a long GC pause on one host only touches its own sessions (see [Crash resilience](#crash-resilience)); other hosts keep pumping frames on their own heaps. | MDP lever #1 |
| **Sessions per host** | MDP `sessionsPerHost` (`D`) | **Memory amortisation.** Multiplexing hundreds of sessions onto one isolate spreads the ~tens-of-KB base heap + scheduler cost across all of them — this is what keeps 50K sockets at single-digit GiB. | MDP lever #2 |

Why this shape scales where naïve `async` or 1:1-isolate models stall:

* **The accept path is sharded, not single-threaded.** A lone listener isolate
  serializes every TLS/WS handshake; `SO_REUSEPORT` across `WS_GATEWAY_SHARDS`
  was the change that broke the **~39K wall** (8 → 16 shards) by parallelising
  accepts across cores.
* **Sessions are multiplexed, not 1:1 with isolates.** Carrying `D` sessions per
  host turns "50K heaps" into "≈ `50K / D` heaps". At `D≈300` that is ~167 host
  isolates instead of 50,000 — the difference between 2.77 GiB and OOM.
* **Fan-out stays star-shaped.** SendPorts can only be read by their owning
  isolate, so cross-session delivery routes through the `:pg` EventBus on main.
  Every session ↔ bridge edge is the only topology isolates can actually pump
  over; topics keep that fan-out bounded to interested sessions.

### What we actually measured (single pod)

| Scenario | Result |
| -------- | ------ |
| 50,000 sockets (30K rust @ 2 msg/s + 20K gleam) | RSS **2.77 GiB**, 6.5 / 10 cores, p50 adopt ≈ 6 ms, **0** failed, **0** restarts |
| 30,000 @ 2 msg/s (= 60K msg/s inbound) | 6.2 cores, 1.46 GiB, adopt p99.5 **< 1 ms** |
| Kill one host isolate under load | only its sessions drop (**312 / 30K = 1.04 %**), reconnect in **~6 s**, pod survives |

The remaining single-pod ceiling is **node CPU**, not the Dart server's memory
or any intrinsic isolate limit — i.e. the architecture is already in the regime
where the *operating point* (how many hosts, how dense), not the design, decides
the ceiling. That operating point is exactly what the autotuner searches.

### Why MDP — and why it's really a POMDP

Going past 50K is not a code change; it's a search for the cheapest pool that
still holds latency. The two levers — **host count** and **density** — trade off
against each other and against load shape, message rate, and GC behaviour, so a
hand-picked constant is wrong the moment traffic moves. The
[MDP isolate-pool autotuner](#mdp-isolate-pool-autotuner) learns it online
instead (see that section for the full state/action/reward and env vars).

It is a **POMDP**, not a clean MDP, because the variables that actually decide
the right setpoint are **hidden**:

| True (hidden) state | What we can observe (the belief inputs) |
| ------------------- | --------------------------------------- |
| per-isolate event-loop queue depth / contention | `dart_ws_adopt_latency_seconds`, `dart_ws_first_frame_latency_seconds` histograms |
| GC pause distribution per host heap | tail of those same latency histograms |
| kernel run-queue / scheduler pressure | adopt-latency drift vs. arrival rate |
| real arrival process | session-count delta (arrival-trend bucket) |
| effective free capacity | utilisation bucket + cold-starts / refusals |

The autotuner never reads contention or GC directly — it acts on a **belief**
formed from that partial projection (utilisation bucket × arrival-trend bucket,
with the latency histograms as the channel that betrays hidden contention). A
density that looks free by slot-count but is actually thrashing one event loop
shows up *only* as a fatter p99 tail; the reward
(`-(latency·w + coldStarts·w + refusals·w + idleHosts·w + size·w)`) turns that
hidden cost into a learnable signal and walks the policy off it.

### How that lifts the ceiling above 50K

1. **Find the densest safe packing.** The learner searches `WS_HOST_DENSITY_LEVELS`
   for the most sessions/host that doesn't tip p99 — directly raising how many
   sockets fit under the memory budget. More density at equal latency = more
   connections per GiB.
2. **Keep the ramp clean.** Holding cold-starts and `1013` refusals at zero
   (the `dart_session_cold_start_spawns_total` / `dart_sessions_refused_capacity_total`
   terms) means you can push the connect ramp harder without the synchronized
   stalls that capped earlier runs.
3. **Stay memory-safe while you grow.** The
   [storm-dampening shield](#mdp-isolate-pool-autotuner) clamps any exploratory
   pick into the feasible region, so to scale up you raise the budget
   (`WS_MDP_MAX_HOST_BUDGET`) and the accept width (`WS_GATEWAY_SHARDS`, ~#vCPU)
   and let the policy **re-converge** on the new optimum — no manual re-tuning.
4. **Make horizontal scaling linear.** Once a single pod's CPU is the wall, a
   learned, stable per-pod density makes each pod's capacity predictable, so
   adding replicas behind the `dd-dart-server` Service scales connections
   roughly linearly — every pod autotunes its own pool independently.

### Next increment — library-segmented (typed) pools

The action space is built to grow. Today it is `size × density` over one
homogeneous host pool. Folding a **pool-count** lever (`{2,3}`) into the same
joint action index adds `lite` / `render` / `data` host entrypoints, so a
benchmark/passthrough connection never initialises Jaspr into its heap. Lower
base heap per host → higher achievable density at equal latency → a higher
connection ceiling on the same hardware — learned the same way the density
lever was added to the size lever, with no plumbing rework.
