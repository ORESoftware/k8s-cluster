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
| `WS_POOL_MAX_HOSTS_PER_SHARD`   | `ceil(max/shards)+2`   | hard per-shard ceiling (0 = unbounded)             |
| `WS_POOL_RETIRE_COOLDOWN_MS`    | `15000`                | idle dwell before retiring a host                  |
| `WS_MDP_OPTIMIZER_URL`          | `dd-mdp-optimizer:8096`| optimizer endpoint (remote mode)                   |
| `WS_MDP_{ALPHA,GAMMA,EPSILON,…}`| see `pool_autotuner.dart` | learner hyperparameters + reward weights        |

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
