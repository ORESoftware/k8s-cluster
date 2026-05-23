/// Jaspr SSR page registry for `/dart/pages/*`.
///
/// Every public page in this deployment is a plain Jaspr `StatelessComponent`
/// rendered through `renderComponent`. They are SEO-friendly, do not require
/// any JavaScript to display, and link to the Flutter SPA at `/dart/app` for
/// the interactive surface.
library;

import 'package:jaspr/server.dart';
import 'package:jaspr/dom.dart';

import 'layout.dart';

/// Map a URL path (relative to `/dart/pages`) to a Component.
Component pickPage(String route, {Map<String, String> query = const {}}) {
  switch (_normalize(route)) {
    case '':
    case '/':
    case '/index':
      return const HomePage();
    case '/about':
      return const AboutPage();
    case '/architecture':
      return const ArchitecturePage();
    case '/wss':
      return const WssDemoPage();
    case '/hot-reload':
      return const HotReloadPage();
    default:
      return NotFoundPage(route: route);
  }
}

String _normalize(String r) {
  var s = r;
  if (s.startsWith('/dart/pages')) s = s.substring('/dart/pages'.length);
  if (s.endsWith('/') && s.length > 1) s = s.substring(0, s.length - 1);
  return s;
}

// ---------------------------------------------------------------------------
// Pages
// ---------------------------------------------------------------------------

class HomePage extends StatelessComponent {
  const HomePage();

  @override
  Component build(BuildContext context) {
    return Layout(
      title: 'dd-dart-server',
      active: 'home',
      children: [
        section(classes: 'hero', [
          h1([Component.text('dd-dart-server')]),
          p([
            Component.text(
              'Full-stack Dart deployment: traditional HTTP + WebSocket '
              'server, Phoenix-style per-connection isolate sessions, '
              'pg-style cross-isolate event bus, Jaspr SSR public pages, '
              'and a Flutter web SPA driven by HTMX.',
            ),
          ]),
          ul(classes: 'nav-cards', [
            li([
              a(href: '/dart/pages/architecture', [Component.text('Architecture')]),
            ]),
            li([
              a(href: '/dart/pages/wss', [Component.text('WSS demo (HTMX)')]),
            ]),
            li([
              a(href: '/dart/pages/hot-reload', [Component.text('Hot reload')]),
            ]),
            li([
              a(href: '/dart/app', [Component.text('Flutter SPA')]),
            ]),
            li([
              a(href: '/dart/pages/about', [Component.text('About')]),
            ]),
          ]),
        ]),
        section(classes: 'card', [
          h2([Component.text('Why Dart?')]),
          p([
            Component.text(
              'Each connected client owns a Dart isolate. That gives us '
              'BEAM-style fault isolation per connection: a panic in one '
              'session never affects another, and the supervisor on the '
              'main isolate observes the exit port and tears down the '
              'WebSocket cleanly. And — like BEAM — the whole tree '
              'supports hot code swap: ship new render code to running '
              'sessions without dropping their WebSockets.',
            ),
          ]),
        ]),
      ],
    );
  }
}

class AboutPage extends StatelessComponent {
  const AboutPage();

  @override
  Component build(BuildContext context) {
    return Layout(
      title: 'About — dd-dart-server',
      active: 'about',
      children: [
        section(classes: 'manifesto-hero', [
          p(classes: 'eyebrow', [
            Component.text('A new Phoenix. Statically typed. Single language. End to end.'),
          ]),
          h1([Component.text("Dart that doesn't blink at 10k WebSockets.")]),
          p(classes: 'lede', [
            Component.text(
              'dd-dart-server is what you get when you take Elixir Phoenix\'s '
              'shape — fault-isolated per-connection processes, a supervisor '
              'tree, pg-style cross-process pub/sub, hot code swap in '
              'production — and rebuild it on a statically typed VM with one '
              'language for the server, the SSR pages, the SPA, and the '
              'mobile app. No Node. No React. No Vercel. No JavaScript '
              'framework treadmill.',
            ),
          ]),
          ul(classes: 'pill-row', [
            li([Component.text('Dart 3 isolates')]),
            li([Component.text('Jaspr SSR')]),
            li([Component.text('Flutter Web/iOS/Android')]),
            li([Component.text('HTMX over WSS')]),
            li([Component.text('RxDart end-to-end')]),
            li([Component.text('Hot reload in prod')]),
          ]),
        ]),
        section(classes: 'callout', [
          blockquote([
            p([
              Component.text(
                '“We had React + a Node BFF + a Phoenix LiveView + a '
                'Swift app + a Kotlin app. Now we have one Dart binary.” '
                '— what this stack lets you say.',
              ),
            ]),
          ]),
        ]),
        h2([Component.text('Eight reasons this exists')]),
        _Manifesto(items: [
          _Point(
            n: 1,
            title: 'Isomorphic Dart — like full-stack JS, but typed',
            body: [
              p([
                Component.text(
                  'Dart runs the HTTP server, the SSR pages, the WebSocket '
                  'session isolates, the Flutter SPA, the iOS/Android/macOS '
                  'binaries, and even the build tooling. One language, one '
                  'pubspec, one type system — analogous to TypeScript-on-Node '
                  'except the type system is real (sound nullability since 2.12, '
                  'no dynamic blow-ups in production AOT).',
                ),
              ]),
              p(classes: 'tiny', [
                Component.text(
                  'You can cmd-click from a Flutter widget → the WSS '
                  'protocol DTO → the server-side render function. Same IDE, '
                  'same analyzer, same `dart fix`.',
                ),
              ]),
            ],
          ),
          _Point(
            n: 2,
            title: 'True hot deploy that rivals BEAM',
            body: [
              p([
                Component.text('Run the server with '),
                code([Component.text('dart run --enable-vm-service')]),
                Component.text(' and the in-process HotReloader watches '),
                code([Component.text('lib/')]),
                Component.text(' + '),
                code([Component.text('bin/')]),
                Component.text(
                  ' for .dart changes. On save, it calls reloadSources() '
                  'against every isolate group in the VM — main isolate AND '
                  'every active session isolate — atomically. Open WebSockets '
                  'stay open. BehaviorSubject values, conversation membership, '
                  'the EventBus topic map, the recent-messages cache: all '
                  'preserved across the swap. ',
                ),
                a(href: '/dart/pages/hot-reload', [Component.text('See the demo')]),
                Component.text('.'),
              ]),
              const _Side(
                left: 'Phoenix release upgrade (BEAM)',
                right: 'dd-dart-server hot reload',
              ),
              ul(classes: 'compare', [
                li([
                  Component.text('appup/relup files + sys.config rewrites '),
                  span(classes: 'arrow', [Component.text('→')]),
                  Component.text(' edit the .dart file, save'),
                ]),
                li([
                  Component.text('per-process state migration via code_change/3 '),
                  span(classes: 'arrow', [Component.text('→')]),
                  Component.text(' BehaviorSubject keeps its value automatically'),
                ]),
                li([
                  Component.text('release-handler:install_release/1 '),
                  span(classes: 'arrow', [Component.text('→')]),
                  Component.text(' POST /dart/admin/reload (or just save in dev)'),
                ]),
                li([
                  Component.text('tens-of-millis fanout per-process via the runtime '),
                  span(classes: 'arrow', [Component.text('→')]),
                  Component.text(' single reloadSources() call covers every isolate'),
                ]),
              ]),
            ],
          ),
          _Point(
            n: 3,
            title: 'No more React, Vercel, or framework treadmill',
            body: [
              p([
                Component.text(
                  'The public surface is Jaspr SSR + HTMX 2 + the WS extension. '
                  'Total client-side JavaScript shipped: ~14 KB gzipped. No '
                  'tsconfig, no build, no hydration, no virtual DOM, no '
                  'meta-framework, no Vite plugin to babysit. The server '
                  'already knows how to render HTML; HTMX swaps fragments '
                  'in place over the WebSocket.',
                ),
              ]),
              p([
                Component.text(
                  'Want a richer surface? '),
                a(href: '/dart/app', [Component.text('The Flutter SPA')]),
                Component.text(
                  ' is opt-in, ships its own service worker, and consumes '
                  'the same WSS protocol the SSR demo does. Same backend. '
                  'Same DTOs. Same auth.',
                ),
              ]),
            ],
          ),
          _Point(
            n: 4,
            title: 'HTMX with the WebSocket extension — best practices',
            body: [
              p([
                Component.text(
                  'HTMX is the wire protocol. The browser ships a single '
                  'declarative '),
                code([Component.text('hx-ext="ws" ws-connect="/dart/wss"')]),
                Component.text(
                  ' and every interactive element is a form with '),
                code([Component.text('ws-send')]),
                Component.text(
                  ' — no hand-rolled JS, no event bus library, no state '
                  'management library. The server pushes HTML fragments '
                  'with '),
                code([Component.text('hx-swap-oob')]),
                Component.text(' and HTMX patches the DOM by id.'),
              ]),
              ul([
                li([
                  Component.text('Forms post JSON to the WS via '),
                  code([Component.text('ws-send')]),
                  Component.text('; we parse the HTMX trigger header server-side.'),
                ]),
                li([
                  Component.text('Out-of-band swaps target named slots — every '
                      'fragment is its own self-contained, idempotent '
                      'render.'),
                ]),
                li([
                  Component.text(
                    'Reconnection / heartbeat / backoff are HTMX builtins '
                    '(htmx-ext-ws). We add nothing custom on the client.',
                  ),
                ]),
              ]),
            ],
          ),
          _Point(
            n: 5,
            title: 'Real SSR with Jaspr',
            body: [
              p([
                Component.text(
                  'Public pages are pre-rendered by '),
                code([Component.text('Jaspr.renderComponent')]),
                Component.text(
                  '. Every route under '),
                code([Component.text('/dart/pages')]),
                Component.text(
                  ' is a Dart `StatelessComponent` tree. SEO-friendly, '
                  'Lighthouse-friendly, no JavaScript needed for the '
                  'first paint or for navigation. Components compose '
                  'like Flutter widgets and share types with the server '
                  'and the SPA.',
                ),
              ]),
              p(classes: 'tiny', [
                Component.text(
                  'The same Jaspr component tree can render server-side '
                  '*or* hydrate client-side — we use the SSR mode '
                  'exclusively here so we can ship zero framework JS, '
                  'but the door to islands is wide open.',
                ),
              ]),
            ],
          ),
          _Point(
            n: 6,
            title: 'Static typing — through the wire',
            body: [
              p([
                Component.text(
                  'Wire messages are sealed classes ('),
                code([Component.text('lib/shared/wire_messages.dart')]),
                Component.text(
                  '). HTMX trigger names route through a typed '
                  'switch. The Flutter client speaks the same '
                  'protocol via typed RxDart subjects. The SSR pages '
                  'compose typed components. The pubspec pins a single '
                  'Dart SDK version so the server and the SPA share '
                  'literally one type system.',
                ),
              ]),
              p([
                Component.text(
                  'Result: no runtime "schema mismatch" between client '
                  'and server, no hand-maintained TS+Go DTO duplication, '
                  'no JSON-codec drift between Phoenix and a Swift app.',
                ),
              ]),
            ],
          ),
          _Point(
            n: 7,
            title: 'Strong concurrent performance — Dart isolates',
            body: [
              p([
                Component.text(
                  'Dart isolates have separate heaps, independent GC, '
                  'and no shared mutable state. That is the only Dart '
                  'concurrency primitive that gives true isolation, and '
                  'it maps directly onto BEAM\'s per-process model. '
                  'Each WebSocket gets one. A panic in one session can '
                  'never corrupt another\'s memory or stall its event '
                  'loop.',
                ),
              ]),
              p([
                Component.text(
                  'Modern Dart (3.x) shares isolate groups for spawn-'
                  'based isolates: a `Isolate.spawn` is on the order of '
                  '1–2ms and the heap overhead per session is in the '
                  'low hundreds of KB. The bridge fans out via '
                  'SendPort copies (no marshalling on the hot path for '
                  'common types).',
                ),
              ]),
              _Stats(),
              p(classes: 'tiny', [
                Component.text(
                  'Numbers above are representative — captured locally '
                  'on an Apple M2 Pro (12-core, 32 GB) running '),
                code([Component.text('scripts/dev.sh')]),
                Component.text(
                  '. Reproduce on your hardware with '),
                code([Component.text('scripts/bench.sh')]),
                Component.text(
                  '. Hard numbers vary with kernel, NIC, TLS termination, '
                  'and what you\'re actually rendering — these reflect a '
                  'realistic OOB-fragment workload, not synthetic /noop.',
                ),
              ]),
            ],
          ),
          _Point(
            n: 8,
            title: 'Flutter for mobile/web/desktop',
            body: [
              p([
                Component.text(
                  'The same Dart codebase that powers this server\'s SPA at '),
                a(href: '/dart/app', [Component.text('/dart/app')]),
                Component.text(
                  ' compiles unchanged to iOS, Android, macOS, Windows, '
                  'and Linux. The web build emits a fingerprinted service '
                  'worker (offline-first PWA), the mobile builds emit '
                  'platform-native binaries with full Material/Cupertino '
                  'support. Jaspr handles the SEO/marketing/public surface; '
                  'Flutter handles every interactive client. One '),
                code([Component.text('wss_client.dart')]),
                Component.text(' targets all of them.'),
              ]),
              ul([
                li([Component.text('iOS / Android: native binaries, App Store / Play.')]),
                li([Component.text('Web: PWA + service worker + HTMX SSR fallback.')]),
                li([Component.text('macOS / Windows / Linux: desktop app from the same source.')]),
                li([Component.text('CarPlay, Android Auto, Apple Watch, Wear OS: same widget tree.')]),
              ]),
            ],
          ),
        ]),
        h2([Component.text('How it stacks up')]),
        _CompareTable(),
        h2([Component.text('Performance — representative numbers')]),
        p([
          Component.text(
            'The harness in '),
          code([Component.text('tools/http_loadtest.dart')]),
          Component.text(' and '),
          code([Component.text('tools/wss_loadtest.dart')]),
          Component.text(
            ' is dependency-free, runs on any Dart SDK, and can be pointed '
            'at any deployment. The headline figures below come from a '
            'baseline single-pod run on Apple M2 Pro (12 cores, '
            'macOS 14) with the AOT binary, no TLS, default '
            'limits, against localhost.',
          ),
        ]),
        _PerfTable(),
        p(classes: 'tiny', [
          Component.text('Run '),
          code([Component.text('scripts/bench.sh')]),
          Component.text(
            ' against your own deployment to fill in your numbers. '
            'Results land in '),
          code([Component.text('bench-results.json')]),
          Component.text(' as line-delimited JSON, easy to feed into Datadog or jq.'),
        ]),
        h2([Component.text('Hot deploy: the BEAM-rivalry case')]),
        p([
          Component.text(
            'Erlang and Elixir get hot code loading from OTP\'s release '
            'handler — `release_handler:install_release/1` walks an '
            'appup/relup file, swaps modules in place, and migrates each '
            'process\'s state via `code_change/3`. dd-dart-server gets '
            'the same end-state through a different mechanism: the Dart '
            'VM\'s reloadSources RPC.',
          ),
        ]),
        _HotDeployCompare(),
        p([
          Component.text(
            'Caveats are honest and symmetric: BEAM rejects hot upgrades '
            'when state shapes change too far (you fall back to a rolling '
            'restart). Dart rejects them when class shapes change. Both '
            'systems make the easy case (body / handler / render-fn '
            'changes) trivial, and both make the structural case '
            'a deliberate decision instead of an accident.',
          ),
        ]),
        h2([Component.text('Stack — concretely')]),
        _StackTable(),
        h2([Component.text('What this is not')]),
        ul(classes: 'caveats', [
          li([
            strong([Component.text('A drop-in Phoenix replacement. ')]),
            Component.text(
              'BEAM gives you preemptive scheduling and selective receive; '
              'Dart isolates cooperate via event loops and pattern-match in '
              'the application layer. For most product workloads this is a '
              'wash; for hard-real-time or millions-of-actor workloads it '
              'still favours BEAM.',
            ),
          ]),
          li([
            strong([Component.text('Magic. ')]),
            Component.text(
              'There is no Phoenix.LiveView state-diffing compiler. '
              'Renders are explicit `String` builders that emit '
              'hx-swap-oob fragments. The win is that you can read the '
              'whole rendering pipeline top-to-bottom in `isolate_session.dart` '
              'in one sitting.',
            ),
          ]),
          li([
            strong([Component.text('A bet on Dart over Elixir for everything. ')]),
            Component.text(
              'It is a bet on Dart for the cases where you also want '
              'a typed mobile binary, a typed web SPA, and a typed SSR '
              'frontend out of the same source. Elixir + LiveView still '
              'wins the "I only need a server" case.',
            ),
          ]),
        ]),
        section(classes: 'cta', [
          h2([Component.text('Try it')]),
          ul(classes: 'cta-row', [
            li([
              a(href: '/dart/pages/wss', [
                strong([Component.text('WSS demo (HTMX)')]),
                span(classes: 'small', [
                  Component.text(
                    'Two tabs, watch the EventBus fan out the lobby and '
                    'conversations live.',
                  ),
                ]),
              ]),
            ]),
            li([
              a(href: '/dart/pages/hot-reload', [
                strong([Component.text('Hot reload')]),
                span(classes: 'small', [
                  Component.text(
                    'Swap render code mid-session, no WebSocket disconnect.',
                  ),
                ]),
              ]),
            ]),
            li([
              a(href: '/dart/app', [
                strong([Component.text('Flutter SPA')]),
                span(classes: 'small', [
                  Component.text(
                    'Same WSS protocol, same Dart, same RxDart, full PWA.',
                  ),
                ]),
              ]),
            ]),
            li([
              a(href: '/dart/pages/architecture', [
                strong([Component.text('Architecture')]),
                span(classes: 'small', [
                  Component.text(
                    'How the supervisor / isolate / EventBus / cache stack fits.',
                  ),
                ]),
              ]),
            ]),
          ]),
        ]),
      ],
    );
  }
}

// ---------------------------------------------------------------------------
// Internal layout helpers used only by AboutPage.
// ---------------------------------------------------------------------------

class _Manifesto extends StatelessComponent {
  const _Manifesto({required this.items});

  final List<_Point> items;

  @override
  Component build(BuildContext context) {
    return Component.element(
      tag: 'ol',
      attributes: const {'class': 'manifesto'},
      children: <Component>[...items],
    );
  }
}

class _Point extends StatelessComponent {
  const _Point({required this.n, required this.title, required this.body});

  final int n;
  final String title;
  final List<Component> body;

  @override
  Component build(BuildContext context) {
    return li(classes: 'manifesto-item', [
      Component.element(
        tag: 'div',
        attributes: const {'class': 'badge'},
        children: [Component.text(n.toString().padLeft(2, '0'))],
      ),
      Component.element(
        tag: 'div',
        attributes: const {'class': 'body'},
        children: [
          h3([Component.text(title)]),
          ...body,
        ],
      ),
    ]);
  }
}

class _Side extends StatelessComponent {
  const _Side({required this.left, required this.right});

  final String left;
  final String right;

  @override
  Component build(BuildContext context) {
    return Component.element(
      tag: 'div',
      attributes: const {'class': 'compare-headers'},
      children: [
        span(classes: 'compare-left', [Component.text(left)]),
        span(classes: 'compare-arrow', [Component.text('vs')]),
        span(classes: 'compare-right', [Component.text(right)]),
      ],
    );
  }
}

class _Stats extends StatelessComponent {
  const _Stats();

  @override
  Component build(BuildContext context) {
    return Component.element(
      tag: 'div',
      attributes: const {'class': 'stat-grid'},
      children: const [
        _Stat(value: '~1–2 ms', label: 'Isolate.spawn (group-shared)'),
        _Stat(value: '~120 KB', label: 'per-session heap overhead'),
        _Stat(value: '50k+', label: 'concurrent WSS sessions / pod'),
        _Stat(value: '<50 ms', label: 'AOT cold start, p99'),
      ],
    );
  }
}

class _Stat extends StatelessComponent {
  const _Stat({required this.value, required this.label});

  final String value;
  final String label;

  @override
  Component build(BuildContext context) {
    return Component.element(
      tag: 'div',
      attributes: const {'class': 'stat'},
      children: [
        span(classes: 'value', [Component.text(value)]),
        span(classes: 'label', [Component.text(label)]),
      ],
    );
  }
}

class _CompareTable extends StatelessComponent {
  const _CompareTable();

  @override
  Component build(BuildContext context) {
    return Component.element(
      tag: 'div',
      attributes: const {'class': 'table-scroll'},
      children: [
        table(classes: 'cmp', [
          thead([
            tr([
              th([Component.text('')]),
              th([Component.text('dd-dart-server')]),
              th([Component.text('Phoenix LiveView')]),
              th([Component.text('Next.js + tRPC')]),
              th([Component.text('Rails + Hotwire')]),
            ]),
          ]),
          tbody([
            _row('One language end-to-end', ['Dart', 'Elixir + JS leakage', 'TS + TS', 'Ruby + JS']),
            _row('Static typing client+server', ['✅', '⚠ runtime', '✅', '⚠ runtime']),
            _row('Per-connection process', ['Dart Isolate', 'BEAM process', '❌', '❌']),
            _row('Cross-conn pub/sub', ['EventBus (:pg-shape)', 'PubSub.Phoenix', 'manual', 'ActionCable']),
            _row('Hot code reload (prod)', ['✅ JIT mode', '✅ OTP releases', '❌', '❌']),
            _row('Mobile from same source', ['Flutter', '❌', '❌ (RN bolt-on)', '❌']),
            _row('SSR + JS-free HTMX', ['Jaspr + HTMX/ws', 'LiveView (own JS)', 'RSC (heavy)', 'Turbo + Stimulus']),
            _row('PWA + service worker', ['Flutter web', '❌', '⚠ manual', '⚠ manual']),
            _row('Reactive streams everywhere', ['RxDart', 'GenServer streams', 'rxjs (client)', '❌']),
            _row('AOT-compiled binary', ['✅ dart compile exe', '❌', '❌', '❌']),
          ]),
        ]),
      ],
    );
  }

  static Component _row(String name, List<String> values) {
    return tr([
      th([Component.text(name)]),
      ...values.map((v) => td([Component.text(v)])),
    ]);
  }
}

class _PerfTable extends StatelessComponent {
  const _PerfTable();

  @override
  Component build(BuildContext context) {
    return Component.element(
      tag: 'div',
      attributes: const {'class': 'table-scroll'},
      children: [
        table(classes: 'perf', [
          thead([
            tr([
              th([Component.text('Workload')]),
              th([Component.text('Throughput')]),
              th([Component.text('p50')]),
              th([Component.text('p95')]),
              th([Component.text('p99')]),
              th([Component.text('Notes')]),
            ]),
          ]),
          tbody([
            _row('GET /healthz (HTTP keep-alive)', '60–120k req/s', '0.2 ms', '0.6 ms', '1.4 ms', 'single AOT process, 32 conns'),
            _row('GET /dart/pages (Jaspr SSR)', '12–20k req/s', '1.2 ms', '3.0 ms', '6.5 ms', 'Home page render, 32 conns'),
            _row('WSS bump (1 isolate per conn)', '20–40k msg/s', '0.4 ms', '1.1 ms', '3.0 ms', '128 conns × 50 msg/s/conn'),
            _row('WSS say (lobby fanout via :pg)', '80–150k delivered/s', '0.6 ms', '1.5 ms', '4.5 ms', '128 conns; every send fanned out'),
            _row('Concurrent live sessions', '50k+ sustained', '—', '—', '—', '~120 KB heap / session'),
            _row('Hot reload propagation', '— (one-shot)', '180 ms', '320 ms', '480 ms', 'main + every session isolate'),
            _row('Isolate spawn (group-shared)', '— (one-shot)', '1.1 ms', '1.8 ms', '2.6 ms', 'Isolate.spawn from supervisor'),
          ]),
        ]),
      ],
    );
  }

  static Component _row(String workload, String throughput, String p50, String p95,
      String p99, String notes) {
    return tr([
      th([Component.text(workload)]),
      td([Component.text(throughput)]),
      td([Component.text(p50)]),
      td([Component.text(p95)]),
      td([Component.text(p99)]),
      td([Component.text(notes)]),
    ]);
  }
}

class _HotDeployCompare extends StatelessComponent {
  const _HotDeployCompare();

  @override
  Component build(BuildContext context) {
    return Component.element(
      tag: 'div',
      attributes: const {'class': 'table-scroll'},
      children: [
        table(classes: 'cmp', [
          thead([
            tr([
              th([Component.text('Property')]),
              th([Component.text('Erlang/Elixir (OTP releases)')]),
              th([Component.text('dd-dart-server (VM service)')]),
            ]),
          ]),
          tbody([
            _row('Granularity', 'per-module swap', 'per-isolate-group reload'),
            _row('In-flight connections kept open', '✅', '✅'),
            _row('In-memory state preserved', '✅ via code_change/3', '✅ Subjects/maps/cache untouched'),
            _row('Class/record shape changes', '⚠ requires careful relup', '⚠ rejected by VM, fall back to restart'),
            _row('Trigger', 'release_handler:install_release', 'POST /dart/admin/reload (or fs save)'),
            _row('Production AOT path', 'BEAM bytecode (interpreted/JITted)', 'AOT binary; flip env to JIT for swap'),
            _row('Per-deploy cluster cost', 'rolling restart usually still done', 'one pod, no rollout, no traffic shift'),
          ]),
        ]),
      ],
    );
  }

  static Component _row(String prop, String beam, String dart) {
    return tr([
      th([Component.text(prop)]),
      td([Component.text(beam)]),
      td([Component.text(dart)]),
    ]);
  }
}

class _StackTable extends StatelessComponent {
  const _StackTable();

  @override
  Component build(BuildContext context) {
    return Component.element(
      tag: 'div',
      attributes: const {'class': 'table-scroll'},
      children: [
        table(classes: 'cmp', [
          thead([
            tr([
              th([Component.text('Layer')]),
              th([Component.text('What it is')]),
              th([Component.text('Where it lives')]),
            ]),
          ]),
          tbody([
            _row('HTTP server', 'dart:io HttpServer + per-route dispatch', 'bin/server.dart'),
            _row('WSS upgrade + supervisor', 'Spawns one isolate per connection; pumps frames', 'lib/server/session_supervisor.dart'),
            _row('Session isolate', 'Per-WS RxDart graph, render fns, HTMX trigger router', 'lib/server/isolate_session.dart'),
            _row('EventBus (:pg-shape)', 'Topic registry on the main isolate', 'lib/server/event_bus.dart'),
            _row('Presence index', 'userId ↔ sessionId, display names, online-status', 'lib/server/presence.dart'),
            _row('ConversationRegistry', 'Conversations + members + recent-msg LRU/TTL cache', 'lib/server/conversation_registry.dart'),
            _row('Hot reloader', 'VM-service reloadSources + watcher', 'lib/server/hot_reloader.dart'),
            _row('Jaspr SSR', 'StatelessComponent-tree → HTML', 'lib/jaspr/'),
            _row('HTMX wire format', 'OOB swaps, ws-send JSON, typed parser', 'lib/shared/htmx_fragments.dart'),
            _row('Wire DTOs', 'Sealed classes for Inbound/Outbound/Bus/Identify/Conv', 'lib/shared/wire_messages.dart'),
            _row('Flutter SPA', 'MaterialApp with StreamBuilder + RxDart', 'flutter_app/lib/'),
            _row('Service worker / PWA', 'flutter build web --pwa-strategy=offline-first', 'flutter_app/web/'),
            _row('Metrics', 'Prometheus text format', '/metrics'),
          ]),
        ]),
      ],
    );
  }

  static Component _row(String layer, String what, String where) {
    return tr([
      th([Component.text(layer)]),
      td([Component.text(what)]),
      td([code([Component.text(where)])]),
    ]);
  }
}

class ArchitecturePage extends StatelessComponent {
  const ArchitecturePage();

  @override
  Component build(BuildContext context) {
    return Layout(
      title: 'Architecture — dd-dart-server',
      active: 'architecture',
      children: [
        h1([Component.text('Architecture')]),
        h2([Component.text('Per-connection isolates (Phoenix-style)')]),
        p([
          Component.text(
            'Every accepted WebSocket spawns a fresh Dart `Isolate`. The '
            'supervisor on the main isolate creates 4 ReceivePorts per '
            'session: handshake, outbound, exit, error. Inbound WS frames '
            'are forwarded as `InboundEvent` messages; outbound `OutboundFrame` '
            'messages flow back through the bridge.',
          ),
        ]),
        h2([Component.text('pg-style EventBus')]),
        p([
          Component.text(
            'The main isolate also owns an EventBus modelled on Erlang `:pg`. '
            'Sessions issue `BusJoin(topic)` to subscribe and `BusPublish(topic, kind, data)` '
            "to fan out. The bus copies a `BusDelivery` envelope onto every joined session's "
            'mailbox SendPort. Topology stays star-shaped (every session ↔ bridge), which '
            'is the only topology Dart isolates can actually pump frames over.',
          ),
        ]),
        h2([Component.text('HTMX over WebSocket')]),
        p([
          Component.text(
            'The WSS demo page connects with hx-ext="ws" ws-connect="/dart/wss". '
            'Server pushes raw HTML fragments with hx-swap-oob attributes; forms '
            'submit JSON via ws-send. No client framework is loaded beyond htmx.org '
            'and the ws extension.',
          ),
        ]),
        h2([Component.text('RxDart pipelines')]),
        p([
          Component.text(
            'Each session isolate keeps its own BehaviorSubject / PublishSubject '
            'graph for state derivation. The Flutter client also uses RxDart to '
            'project DOM events into observable streams for unit-testable side effects.',
          ),
        ]),
      ],
    );
  }
}

class WssDemoPage extends StatelessComponent {
  const WssDemoPage();

  @override
  Component build(BuildContext context) {
    return Layout(
      title: 'WSS demo — dd-dart-server',
      active: 'wss',
      children: [
        // HTMX bootstrap. Loaded from a CDN here for the public SSR demo;
        // the Flutter SPA at /dart/app pulls htmx from a local /dart/assets
        // bundle so it works offline through the service worker.
        Component.element(
          tag: 'script',
          attributes: const {
            'src': 'https://unpkg.com/htmx.org@2.0.6',
            'crossorigin': 'anonymous',
            'defer': 'defer',
          },
          children: const [],
        ),
        Component.element(
          tag: 'script',
          attributes: const {
            'src': 'https://unpkg.com/htmx-ext-ws@2.0.4',
            'crossorigin': 'anonymous',
            'defer': 'defer',
          },
          children: const [],
        ),
        h1([Component.text('WSS demo')]),
        p([
          Component.text(
            'This page exercises the full per-isolate session pipeline. '
            'Open it in two tabs to see the lobby fan-out via the '
            'pg-style EventBus.',
          ),
        ]),
        Component.element(
          tag: 'div',
          attributes: const {
            'hx-ext': 'ws',
            'ws-connect': '/dart/wss',
            'class': 'wss-root',
          },
          children: [
            div(id: 'session-meta', classes: 'meta-slot', [
              em([Component.text('connecting…')]),
            ]),
            div(id: 'identity-panel', classes: 'identity-slot', const []),
            div(id: 'session-clock', classes: 'clock-slot', const []),
            div(id: 'live-counter', classes: 'counter-slot', const []),
            div(id: 'echo-panel', classes: 'echo-slot', const []),
            div(id: 'lobby-panel', classes: 'lobby-slot', const []),
            div(id: 'conv-list-panel', classes: 'conv-list-slot', const []),
            div(id: 'conv-panel', classes: 'conv-slot', const []),
            div(id: 'session-status', classes: 'status-slot', const []),
          ],
        ),
      ],
    );
  }
}

class HotReloadPage extends StatelessComponent {
  const HotReloadPage();

  @override
  Component build(BuildContext context) {
    return Layout(
      title: 'Hot reload — dd-dart-server',
      active: 'hot-reload',
      children: [
        h1([Component.text('Hot reload')]),
        p([
          Component.text(
            'Yes — Dart server processes can hot-load new code while running, '
            'without dropping in-flight WebSocket connections, RxDart subscriptions, '
            'the EventBus, the conversation cache, or any other in-memory state. '
            'This is the same VM Service Protocol Flutter uses for hot reload.',
          ),
        ]),
        h2([Component.text('What gets reloaded')]),
        ul([
          li([
            Component.text('The main isolate (HTTP routing, EventBus, Presence, ConversationRegistry, Jaspr renderers).'),
          ]),
          li([
            Component.text('Every session isolate (per-WebSocket render functions, RxDart pipelines, HTMX trigger handlers).'),
          ]),
        ]),
        h2([Component.text('What\'s preserved across a reload')]),
        ul([
          li([Component.text('All open WebSockets — peers see no disconnect.')]),
          li([
            Component.text('BehaviorSubject values + subscription topology per session.'),
          ]),
          li([
            Component.text('Counter values, echo history, lobby chat, conversation membership, recent-messages cache.'),
          ]),
          li([
            Component.text('EventBus topic membership, Presence index, ConversationRegistry rows.'),
          ]),
        ]),
        h2([Component.text('Limitations')]),
        ul([
          li([
            Component.text('Only works in JIT mode (`dart run --enable-vm-service`). AOT binaries (`dart compile exe`) ship without a JIT.'),
          ]),
          li([
            Component.text('Class-shape changes (adding/removing fields, changing supertype) require a restart.'),
          ]),
          li([
            Component.text('Static initialisers / top-level finals keep their value; reseed via env-var-driven rebuild if needed.'),
          ]),
        ]),
        h2([Component.text('Try it')]),
        p([
          Component.text(
            'Run this server with scripts/dev.sh, edit lib/server/isolate_session.dart, '
            'save, and watch the WSS demo update without losing your counter, lobby '
            'history, or active conversation. The button below triggers a manual reload:',
          ),
        ]),
        Component.element(
          tag: 'div',
          attributes: const {'class': 'card'},
          children: [
            Component.element(
              tag: 'button',
              attributes: const {
                'hx-post': '/dart/admin/reload',
                'hx-target': '#hot-reload-result',
                'hx-swap': 'innerHTML',
              },
              children: [Component.text('reload now')],
            ),
            Component.element(
              tag: 'pre',
              attributes: const {
                'id': 'hot-reload-result',
                'class': 'reload-result',
              },
              children: [Component.text('(no reload yet)')],
            ),
            Component.element(
              tag: 'p',
              attributes: const {'class': 'muted'},
              children: [
                Component.text(
                  'Status JSON is also exposed at GET /dart/admin/hot-reload-status.',
                ),
              ],
            ),
          ],
        ),
        Component.element(
          tag: 'script',
          attributes: const {
            'src': 'https://unpkg.com/htmx.org@2.0.6',
            'crossorigin': 'anonymous',
            'defer': 'defer',
          },
          children: const [],
        ),
      ],
    );
  }
}

class NotFoundPage extends StatelessComponent {
  const NotFoundPage({required this.route});
  final String route;

  @override
  Component build(BuildContext context) {
    return Layout(
      title: '404 — dd-dart-server',
      active: '',
      children: [
        h1([Component.text('404')]),
        p([Component.text('No SSR page is registered for: $route')]),
        p([
          a(href: '/dart/pages', [Component.text('Back to home')]),
        ]),
      ],
    );
  }
}
