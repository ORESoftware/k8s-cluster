/// Shared Jaspr layout used by every `/dart/pages/*` SSR page.
///
/// Keeps `<head>`, top navigation, and a tiny inline stylesheet in one
/// place so each individual page only worries about its body content.
library;

import 'package:jaspr/server.dart';
import 'package:jaspr/dom.dart';

class Layout extends StatelessComponent {
  const Layout({
    required this.title,
    required this.active,
    required this.children,
  });

  final String title;
  final String active;
  final List<Component> children;

  @override
  Component build(BuildContext context) {
    return Document(
      title: title,
      lang: 'en',
      meta: const {
        'viewport': 'width=device-width, initial-scale=1',
        'description':
            'Full-stack Dart deployment with per-connection isolate sessions, '
            'a pg-style cross-isolate EventBus, Jaspr SSR public pages, and '
            'a Flutter web SPA driven by HTMX over /dart/wss.',
        'theme-color': '#0d1117',
      },
      head: [
        Component.element(
          tag: 'link',
          attributes: const {
            'rel': 'icon',
            'type': 'image/png',
            'href': '/dart/assets/favicon.png',
          },
          children: const [],
        ),
        Component.element(
          tag: 'link',
          attributes: const {
            'rel': 'manifest',
            'href': '/dart/assets/manifest.json',
          },
          children: const [],
        ),
        // `style` doesn't take child components in jaspr 0.23 — its
        // entire content is one text node. We use Component.element for a
        // stable surface that doesn't depend on tag-specific helper quirks.
        Component.element(
          tag: 'style',
          attributes: const {},
          children: [Component.text(_inlineCss)],
        ),
      ],
      body: section(classes: 'page', [
        _Nav(active: active),
        main_(classes: 'main-content', children),
        const _Footer(),
      ]),
    );
  }
}

class _Nav extends StatelessComponent {
  const _Nav({required this.active});
  final String active;

  @override
  Component build(BuildContext context) {
    Component link(String slug, String label, String href) {
      final cls = active == slug ? 'nav-link active' : 'nav-link';
      return a(href: href, classes: cls, [Component.text(label)]);
    }

    return nav(classes: 'top-nav', [
      div(classes: 'brand', [
        a(href: '/dart/pages', [Component.text('dd-dart-server')]),
      ]),
      div(classes: 'links', [
        link('home', 'home', '/dart/pages'),
        link('architecture', 'architecture', '/dart/pages/architecture'),
        link('wss', 'wss demo', '/dart/pages/wss'),
        link('hot-reload', 'hot reload', '/dart/pages/hot-reload'),
        link('about', 'about', '/dart/pages/about'),
        a(
          href: '/dart/mobile/',
          classes: 'nav-link mobile-link',
          [Component.text('mobile')],
        ),
        a(
          href: '/dart/app',
          classes: 'nav-link cta',
          [Component.text('open Flutter SPA →')],
        ),
      ]),
    ]);
  }
}

class _Footer extends StatelessComponent {
  const _Footer();

  @override
  Component build(BuildContext context) {
    return footer(classes: 'site-footer', [
      span([
        Component.text(
          'SSR by Jaspr · WSS at /dart/wss · isolate-per-session',
        ),
      ]),
    ]);
  }
}

const _inlineCss = '''
:root {
  color-scheme: dark;
  --bg: #0d1117;
  --surface: #161b22;
  --border: #30363d;
  --text: #c9d1d9;
  --muted: #8b949e;
  --accent: #58a6ff;
  --accent-strong: #79c0ff;
  --self: #238636;
  --other: #1f6feb;
}
* { box-sizing: border-box; }
body { margin: 0; background: var(--bg); color: var(--text); font: 14px/1.5 ui-sans-serif, system-ui, sans-serif; }
a { color: var(--accent); text-decoration: none; }
a:hover { color: var(--accent-strong); text-decoration: underline; }
code { background: var(--surface); padding: 1px 4px; border-radius: 3px; font-size: 12px; }
.page { max-width: 880px; margin: 0 auto; padding: 24px; }
.top-nav { display: flex; justify-content: space-between; align-items: center; padding: 12px 0; border-bottom: 1px solid var(--border); margin-bottom: 24px; }
.top-nav .brand a { font-weight: 600; }
.top-nav .links { display: flex; gap: 16px; flex-wrap: wrap; }
.nav-link.active { color: var(--accent-strong); font-weight: 600; }
.nav-link.cta { padding: 4px 8px; border: 1px solid var(--accent); border-radius: 4px; }
.nav-link.mobile-link { font-size: 12px; color: var(--muted); }
.nav-link.mobile-link:hover { color: var(--accent-strong); }
.main-content { padding: 8px 0 32px; }
.hero { padding: 24px 0; border-bottom: 1px solid var(--border); margin-bottom: 24px; }
.card { background: var(--surface); border: 1px solid var(--border); border-radius: 6px; padding: 16px; margin: 16px 0; }
.nav-cards { display: grid; grid-template-columns: repeat(auto-fit, minmax(180px, 1fr)); gap: 12px; padding: 0; list-style: none; }
.nav-cards li a { display: block; padding: 12px; background: var(--surface); border: 1px solid var(--border); border-radius: 6px; }
.site-footer { color: var(--muted); border-top: 1px solid var(--border); padding-top: 12px; margin-top: 32px; font-size: 12px; }
.wss-root { display: grid; grid-template-columns: 1fr 1fr; gap: 16px; }
.wss-root > div { background: var(--surface); border: 1px solid var(--border); border-radius: 6px; padding: 12px; min-height: 64px; }
.wss-root .meta-slot, .wss-root .status-slot, .wss-root .identity-slot { grid-column: 1 / -1; }
.wss-root .conv-list-slot, .wss-root .conv-slot { grid-column: span 1; }
.identity { display: flex; flex-wrap: wrap; align-items: center; gap: 8px; }
.identity .label { color: var(--muted); font-size: 12px; text-transform: uppercase; letter-spacing: 0.04em; }
.identity .uid { background: var(--bg); padding: 2px 6px; border-radius: 4px; }
.identity .display { font-weight: 600; }
.identity-form { display: flex; flex: 1; gap: 6px; min-width: 280px; }
.identity-form input { flex: 1; padding: 4px 6px; background: var(--bg); color: var(--text); border: 1px solid var(--border); border-radius: 4px; font-size: 12px; }
.identity-form button { background: var(--accent); color: var(--bg); border: 0; border-radius: 4px; padding: 4px 12px; cursor: pointer; }
.convlist h4, .convpanel h4 { margin: 0 0 8px; font-size: 12px; color: var(--muted); text-transform: uppercase; letter-spacing: 0.04em; }
.convlist .rows { padding: 0; margin: 0 0 8px; list-style: none; max-height: 220px; overflow-y: auto; }
.convlist .row { display: flex; align-items: stretch; gap: 4px; padding: 4px 0; border-bottom: 1px dashed var(--border); }
.convlist .row.selected { background: rgba(88, 166, 255, 0.1); border-radius: 4px; padding: 4px; }
.convlist .row-actions { display: flex; flex: 1; gap: 4px; }
.convlist .row-pick { flex: 1; text-align: left; background: transparent; color: var(--text); border: 0; padding: 4px 6px; cursor: pointer; display: flex; flex-direction: column; }
.convlist .row-pick small { color: var(--muted); font-size: 11px; }
.convlist .row-pick:hover { background: var(--bg); border-radius: 4px; }
.convlist .row-join, .convlist .row-leave { background: transparent; color: var(--accent); border: 1px solid var(--border); border-radius: 4px; padding: 0 8px; font-size: 12px; cursor: pointer; }
.convlist .row-join:hover, .convlist .row-leave:hover { color: var(--accent-strong); border-color: var(--accent); }
.convlist .open-form { display: flex; gap: 6px; }
.convlist .open-form input { flex: 1; padding: 6px 8px; background: var(--bg); color: var(--text); border: 1px solid var(--border); border-radius: 4px; }
.convlist .open-form button { background: var(--accent); color: var(--bg); border: 0; border-radius: 4px; padding: 6px 12px; cursor: pointer; }
.convpanel { display: flex; flex-direction: column; gap: 8px; min-height: 220px; }
.convpanel.empty { color: var(--muted); }
.convpanel ul { padding: 0; margin: 0; list-style: none; max-height: 220px; overflow-y: auto; }
.convpanel li { padding: 4px 0; border-bottom: 1px dashed var(--border); }
.convpanel form { display: flex; gap: 6px; }
.convpanel input { flex: 1; padding: 6px 8px; background: var(--bg); color: var(--text); border: 1px solid var(--border); border-radius: 4px; }
.convpanel button { background: var(--accent); color: var(--bg); border: 0; border-radius: 4px; padding: 6px 12px; cursor: pointer; }
.msg { display: flex; gap: 6px; align-items: baseline; }
.msg.self code { background: var(--self); color: white; }
.msg.other code { background: var(--other); color: white; }
.reload-result { background: var(--bg); padding: 12px; border-radius: 4px; max-height: 240px; overflow: auto; white-space: pre-wrap; word-wrap: break-word; font-size: 12px; }
.meta { display: grid; grid-template-columns: 120px 1fr; row-gap: 4px; column-gap: 8px; margin: 0; }
.meta dt { color: var(--muted); }
.meta dd { margin: 0; }
.counter { display: flex; align-items: center; gap: 12px; }
.counter .value { font-size: 32px; font-weight: 600; min-width: 48px; text-align: right; }
.counter form { display: inline; }
.counter button, .echo button, .lobby button { background: var(--accent); color: var(--bg); border: 0; border-radius: 4px; padding: 6px 12px; cursor: pointer; }
.counter button:hover, .echo button:hover, .lobby button:hover { background: var(--accent-strong); }
.echo, .lobby { display: flex; flex-direction: column; gap: 8px; }
.echo ul, .lobby ul { padding: 0; margin: 0; list-style: none; max-height: 220px; overflow-y: auto; }
.echo li, .lobby li { padding: 4px 0; border-bottom: 1px dashed var(--border); }
.muted { color: var(--muted); }
.lobby .msg { display: flex; gap: 6px; align-items: baseline; }
.lobby .msg.self code { background: var(--self); color: white; }
.lobby .msg.other code { background: var(--other); color: white; }
.echo input, .lobby input { flex: 1; padding: 6px 8px; background: var(--bg); color: var(--text); border: 1px solid var(--border); border-radius: 4px; }
.echo form, .lobby form { display: flex; gap: 8px; }

/* ----- About-page manifesto + comparison tables ------------------------ */
.page { max-width: 1080px; }
.manifesto-hero { padding: 32px 0 24px; border-bottom: 1px solid var(--border); margin-bottom: 24px; }
.manifesto-hero .eyebrow { color: var(--accent-strong); font-size: 12px; letter-spacing: 0.18em; text-transform: uppercase; margin: 0 0 8px; font-weight: 600; }
.manifesto-hero h1 { font-size: 36px; line-height: 1.15; margin: 0 0 12px; letter-spacing: -0.01em; }
.manifesto-hero .lede { color: var(--text); font-size: 16px; line-height: 1.55; margin: 0 0 18px; max-width: 760px; }
.pill-row { display: flex; flex-wrap: wrap; gap: 6px; padding: 0; margin: 0; list-style: none; }
.pill-row li { background: var(--surface); border: 1px solid var(--border); border-radius: 999px; padding: 4px 10px; font-size: 12px; color: var(--muted); }
.pill-row li:nth-child(odd) { color: var(--accent); border-color: rgba(88, 166, 255, 0.4); }
.callout { margin: 24px 0; }
.callout blockquote { background: var(--surface); border-left: 3px solid var(--accent); border-radius: 0 6px 6px 0; padding: 14px 18px; margin: 0; color: var(--text); }
.callout blockquote p { margin: 0; font-style: italic; }
.manifesto { list-style: none; padding: 0; margin: 24px 0 32px; counter-reset: none; }
.manifesto-item { display: grid; grid-template-columns: 64px 1fr; gap: 18px; padding: 18px 0; border-bottom: 1px solid var(--border); }
.manifesto-item .badge { display: flex; align-items: flex-start; justify-content: center; font-family: ui-monospace, "SFMono-Regular", monospace; font-size: 22px; font-weight: 700; color: var(--accent); padding-top: 4px; }
.manifesto-item .body h3 { margin: 0 0 8px; font-size: 18px; color: var(--text); }
.manifesto-item .body p { margin: 0 0 10px; max-width: 760px; }
.manifesto-item .body p.tiny { color: var(--muted); font-size: 12px; }
.manifesto-item .body ul { margin: 8px 0 12px 20px; padding: 0; }
.manifesto-item .body ul li { margin: 4px 0; }
.manifesto-item .body ul.compare { list-style: none; margin-left: 0; }
.manifesto-item .body ul.compare li { display: flex; align-items: center; gap: 8px; flex-wrap: wrap; padding: 4px 0; border-bottom: 1px dashed var(--border); }
.manifesto-item .body ul.compare li:last-child { border-bottom: none; }
.manifesto-item .body .arrow { color: var(--accent-strong); font-weight: 700; }
.compare-headers { display: grid; grid-template-columns: 1fr auto 1fr; gap: 12px; align-items: center; margin: 12px 0 6px; }
.compare-headers .compare-left, .compare-headers .compare-right { font-size: 11px; text-transform: uppercase; letter-spacing: 0.08em; color: var(--muted); }
.compare-headers .compare-right { text-align: right; color: var(--accent-strong); }
.compare-headers .compare-arrow { color: var(--muted); font-size: 11px; text-align: center; }
.stat-grid { display: grid; grid-template-columns: repeat(auto-fit, minmax(160px, 1fr)); gap: 12px; margin: 16px 0; }
.stat { background: var(--surface); border: 1px solid var(--border); border-radius: 6px; padding: 14px 16px; display: flex; flex-direction: column; gap: 4px; }
.stat .value { font-family: ui-monospace, "SFMono-Regular", monospace; font-size: 22px; font-weight: 700; color: var(--accent-strong); }
.stat .label { color: var(--muted); font-size: 11px; text-transform: uppercase; letter-spacing: 0.05em; }
.table-scroll { overflow-x: auto; margin: 12px 0 24px; border: 1px solid var(--border); border-radius: 6px; }
table.cmp, table.perf { width: 100%; border-collapse: collapse; font-size: 13px; min-width: 720px; }
table.cmp th, table.cmp td, table.perf th, table.perf td { border-bottom: 1px solid var(--border); padding: 10px 12px; text-align: left; vertical-align: top; }
table.cmp thead th, table.perf thead th { background: var(--surface); color: var(--muted); font-size: 11px; text-transform: uppercase; letter-spacing: 0.06em; position: sticky; top: 0; }
table.cmp tbody th, table.perf tbody th { font-weight: 600; color: var(--text); white-space: nowrap; }
table.cmp tbody tr:nth-child(odd), table.perf tbody tr:nth-child(odd) { background: rgba(255, 255, 255, 0.015); }
table.cmp td, table.perf td { color: var(--text); }
table.perf tbody td:first-of-type { color: var(--accent-strong); font-family: ui-monospace, "SFMono-Regular", monospace; }
ul.caveats { list-style: none; padding: 0; margin: 16px 0 24px; display: grid; gap: 12px; }
ul.caveats li { background: var(--surface); border: 1px solid var(--border); border-left: 3px solid var(--muted); border-radius: 4px; padding: 12px 14px; }
ul.caveats li strong { color: var(--accent-strong); }
.cta { background: linear-gradient(135deg, rgba(88,166,255,0.08), rgba(31,111,235,0.04)); border: 1px solid var(--border); border-radius: 8px; padding: 24px; margin: 32px 0; }
.cta h2 { margin-top: 0; }
ul.cta-row { display: grid; grid-template-columns: repeat(auto-fit, minmax(220px, 1fr)); gap: 12px; padding: 0; margin: 0; list-style: none; }
ul.cta-row li a { display: flex; flex-direction: column; gap: 6px; padding: 14px; background: var(--surface); border: 1px solid var(--border); border-radius: 6px; }
ul.cta-row li a:hover { border-color: var(--accent); text-decoration: none; }
ul.cta-row li a strong { color: var(--accent-strong); }
ul.cta-row li a .small { color: var(--muted); font-size: 12px; line-height: 1.4; }
@media (max-width: 720px) {
  .manifesto-hero h1 { font-size: 28px; }
  .manifesto-item { grid-template-columns: 40px 1fr; gap: 12px; }
  .manifesto-item .badge { font-size: 18px; }
}
''';
