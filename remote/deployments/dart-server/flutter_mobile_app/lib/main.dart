/// Mobile-optimized Flutter web bundle for dd-dart-server.
///
/// Served at `/dart/mobile/`. This bundle is intentionally a tiny
/// landing surface: a vertical list of the Jaspr SSR pages with large
/// tap targets, and a stubbed "connect to /dart/wss" button. The full
/// interactive SPA still lives at `/dart/app/` (different, richer
/// bundle, two-pane layout for desktop).
///
/// The connect button uses RxDart subjects to mirror the structure of
/// `flutter_app/lib/wss_client.dart` without depending on it. Real
/// session adoption will land in a follow-up; today the button just
/// flips a `BehaviorSubject<MobileConnectState>` so the status pill
/// can render.
library;

import 'dart:async';
import 'dart:js_interop';

import 'package:flutter/cupertino.dart';
import 'package:flutter/material.dart';
import 'package:rxdart/rxdart.dart';

void main() {
  runApp(const DartMobileApp());
}

/// Top-level @JS hook so we can hand a tap off to a server-rendered
/// page without pulling in `package:url_launcher` or `package:web`.
/// Resolves to `globalThis.location.assign(url)` in the browser.
@JS('location.assign')
external void _locationAssign(String url);

class DartMobileApp extends StatelessWidget {
  const DartMobileApp({super.key});

  @override
  Widget build(BuildContext context) {
    return MaterialApp(
      title: 'dd-dart-server (mobile)',
      debugShowCheckedModeBanner: false,
      theme: ThemeData(
        colorScheme: ColorScheme.fromSeed(
          seedColor: const Color(0xFF58A6FF),
          brightness: Brightness.dark,
          surface: const Color(0xFF161B22),
        ),
        scaffoldBackgroundColor: const Color(0xFF0D1117),
        useMaterial3: true,
      ),
      home: const MobileShell(),
    );
  }
}

/// One-page mobile shell: a header, a vertical list of links, and a
/// stub connect button at the bottom. Locked to a single column.
class MobileShell extends StatefulWidget {
  const MobileShell({super.key});

  @override
  State<MobileShell> createState() => _MobileShellState();
}

class _MobileShellState extends State<MobileShell> {
  final BehaviorSubject<MobileConnectState> _connect =
      BehaviorSubject<MobileConnectState>.seeded(MobileConnectState.idle);
  Timer? _pendingTimer;

  @override
  void dispose() {
    _pendingTimer?.cancel();
    _connect.close();
    super.dispose();
  }

  void _onConnectPressed() {
    if (_connect.value == MobileConnectState.connecting) return;
    _connect.add(MobileConnectState.connecting);
    _pendingTimer?.cancel();
    // Stub: real WSS adoption happens in a follow-up. We keep the
    // button on a 600ms simulated round-trip so the status pill can
    // be exercised end-to-end during local dev.
    _pendingTimer = Timer(const Duration(milliseconds: 600), () {
      _connect.add(MobileConnectState.idle);
    });
  }

  @override
  Widget build(BuildContext context) {
    final width = MediaQuery.sizeOf(context).width;
    // Lock layout to a single readable column. On desktop we just
    // letterbox the column inside a max-width frame instead of
    // switching to a multi-pane shell.
    final columnMax = width.clamp(0.0, 520.0);

    return Scaffold(
      appBar: AppBar(
        title: const Text('dd-dart-server (mobile)'),
        actions: [
          StreamBuilder<MobileConnectState>(
            stream: _connect,
            initialData: _connect.value,
            builder: (context, snap) {
              final s = snap.data ?? MobileConnectState.idle;
              final color = switch (s) {
                MobileConnectState.idle => Colors.blueGrey,
                MobileConnectState.connecting => Colors.amberAccent,
                MobileConnectState.connected => Colors.greenAccent,
                MobileConnectState.error => Colors.redAccent,
              };
              return Padding(
                padding: const EdgeInsets.only(right: 16),
                child: Row(
                  children: [
                    Icon(CupertinoIcons.circle_fill, size: 12, color: color),
                    const SizedBox(width: 6),
                    Text(s.name),
                  ],
                ),
              );
            },
          ),
        ],
      ),
      body: Center(
        child: ConstrainedBox(
          constraints: BoxConstraints(maxWidth: columnMax),
          child: ListView(
            padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 24),
            children: [
              const _MobileHero(),
              const SizedBox(height: 24),
              const _SectionHeader(label: 'jaspr ssr pages'),
              const SizedBox(height: 8),
              for (final link in _ssrLinks)
                _LinkTile(link: link, onTap: () => _locationAssign(link.href)),
              const SizedBox(height: 24),
              const _SectionHeader(label: 'flutter spa'),
              const SizedBox(height: 8),
              _LinkTile(
                link: const _NavLink(
                  title: 'Open the desktop SPA',
                  subtitle: '/dart/app — two-pane WSS shell',
                  href: '/dart/app',
                  icon: CupertinoIcons.app_badge,
                ),
                onTap: () => _locationAssign('/dart/app'),
              ),
              const SizedBox(height: 24),
              const _SectionHeader(label: 'realtime'),
              const SizedBox(height: 8),
              StreamBuilder<MobileConnectState>(
                stream: _connect,
                initialData: _connect.value,
                builder: (context, snap) {
                  final s = snap.data ?? MobileConnectState.idle;
                  final connecting = s == MobileConnectState.connecting;
                  return SizedBox(
                    width: double.infinity,
                    child: FilledButton.icon(
                      onPressed: connecting ? null : _onConnectPressed,
                      icon: const Icon(CupertinoIcons.bolt_fill),
                      label: Padding(
                        padding: const EdgeInsets.symmetric(vertical: 18),
                        child: Text(
                          connecting
                              ? 'connecting…'
                              : 'Connect to /dart/wss',
                          style: const TextStyle(
                            fontSize: 16,
                            fontWeight: FontWeight.w600,
                          ),
                        ),
                      ),
                    ),
                  );
                },
              ),
              const SizedBox(height: 8),
              const Padding(
                padding: EdgeInsets.symmetric(horizontal: 4, vertical: 4),
                child: Text(
                  'Connect button is stubbed in this build — real session '
                  'adoption lands alongside the per-isolate WSS shell.',
                  style: TextStyle(color: Colors.grey, fontSize: 12),
                ),
              ),
              const SizedBox(height: 32),
              const _MobileFooter(),
            ],
          ),
        ),
      ),
    );
  }
}

class _MobileHero extends StatelessWidget {
  const _MobileHero();

  @override
  Widget build(BuildContext context) {
    return Card(
      child: Padding(
        padding: const EdgeInsets.all(16),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            const Text(
              'dd-dart-server',
              style: TextStyle(fontSize: 22, fontWeight: FontWeight.w700),
            ),
            const SizedBox(height: 4),
            Text(
              'mobile front-end',
              style: TextStyle(
                fontSize: 12,
                letterSpacing: 1.2,
                color: Theme.of(context).colorScheme.primary,
              ),
            ),
            const SizedBox(height: 12),
            const Text(
              'Tap a page to open the server-rendered Jaspr surface, '
              'or open the Flutter SPA for the full WSS demo.',
              style: TextStyle(fontSize: 14, height: 1.4),
            ),
          ],
        ),
      ),
    );
  }
}

class _SectionHeader extends StatelessWidget {
  const _SectionHeader({required this.label});
  final String label;

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.symmetric(horizontal: 4),
      child: Text(
        label.toUpperCase(),
        style: const TextStyle(
          color: Colors.grey,
          fontSize: 11,
          letterSpacing: 1.4,
          fontWeight: FontWeight.w600,
        ),
      ),
    );
  }
}

class _LinkTile extends StatelessWidget {
  const _LinkTile({required this.link, required this.onTap});

  final _NavLink link;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 4),
      child: Material(
        color: Theme.of(context).colorScheme.surface,
        borderRadius: BorderRadius.circular(8),
        child: InkWell(
          borderRadius: BorderRadius.circular(8),
          onTap: onTap,
          child: Padding(
            padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 18),
            child: Row(
              children: [
                Icon(
                  link.icon,
                  size: 22,
                  color: Theme.of(context).colorScheme.primary,
                ),
                const SizedBox(width: 14),
                Expanded(
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      Text(
                        link.title,
                        style: const TextStyle(
                          fontSize: 16,
                          fontWeight: FontWeight.w600,
                        ),
                      ),
                      const SizedBox(height: 2),
                      Text(
                        link.subtitle,
                        style: const TextStyle(
                          color: Colors.grey,
                          fontSize: 12,
                        ),
                      ),
                    ],
                  ),
                ),
                const Icon(
                  CupertinoIcons.chevron_right,
                  size: 18,
                  color: Colors.grey,
                ),
              ],
            ),
          ),
        ),
      ),
    );
  }
}

class _MobileFooter extends StatelessWidget {
  const _MobileFooter();

  @override
  Widget build(BuildContext context) {
    return const Padding(
      padding: EdgeInsets.symmetric(horizontal: 4),
      child: Text(
        'served from /dart/mobile · jaspr ssr at /dart/pages · spa at /dart/app',
        textAlign: TextAlign.center,
        style: TextStyle(color: Colors.grey, fontSize: 11),
      ),
    );
  }
}

class _NavLink {
  const _NavLink({
    required this.title,
    required this.subtitle,
    required this.href,
    required this.icon,
  });

  final String title;
  final String subtitle;
  final String href;
  final IconData icon;
}

const List<_NavLink> _ssrLinks = [
  _NavLink(
    title: 'Home',
    subtitle: '/dart/pages — server-rendered landing',
    href: '/dart/pages',
    icon: CupertinoIcons.house_fill,
  ),
  _NavLink(
    title: 'About',
    subtitle: '/dart/pages/about — manifesto + comparison',
    href: '/dart/pages/about',
    icon: CupertinoIcons.info_circle_fill,
  ),
  _NavLink(
    title: 'Architecture',
    subtitle: '/dart/pages/architecture — isolates, EventBus, RxDart',
    href: '/dart/pages/architecture',
    icon: CupertinoIcons.square_stack_3d_up_fill,
  ),
  _NavLink(
    title: 'WSS demo',
    subtitle: '/dart/pages/wss — HTMX over WebSocket',
    href: '/dart/pages/wss',
    icon: CupertinoIcons.dot_radiowaves_left_right,
  ),
  _NavLink(
    title: 'Hot reload',
    subtitle: '/dart/pages/hot-reload — VM-service code swap',
    href: '/dart/pages/hot-reload',
    icon: CupertinoIcons.arrow_2_circlepath,
  ),
];

/// Surface state for the stubbed `/dart/wss` connect button. Mirrors
/// the shape of `flutter_app/lib/wss_client.dart::ConnectionState`
/// without depending on it.
enum MobileConnectState { idle, connecting, connected, error }
