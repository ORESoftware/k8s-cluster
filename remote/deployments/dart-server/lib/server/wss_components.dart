/// Jaspr `StatelessComponent`s for every WSS-demo out-of-band fragment.
///
/// Each per-connection isolate session pushes HTMX fragments to the browser
/// over `/dart/wss`. We used to assemble those fragments by string
/// concatenation (`'<div ...>' + htmlEscape(name) + '</div>'`), which gave
/// up everything Jaspr already does for us:
///
///   * automatic HTML/attribute escaping (no XSS foot-guns),
///   * a real component model so panels are composable and unit-testable,
///   * one mental model for both `/dart/pages/*` SSR and the WSS fragments.
///
/// This file is the "components, not strings" half of that change. Each
/// `StatelessComponent` here corresponds to one HTMX OOB target. The
/// session isolates instantiate them, hand them to [renderFragment] which
/// runs Jaspr's `renderComponent(... standalone: true)`, and ship the
/// resulting HTML over the WebSocket.
///
/// The wrapper is [OobWrap]: a `<div id hx-swap-oob>` that HTMX inspects
/// to decide which slot of `/dart/pages/wss` (or any other page that
/// declares the same ids) to swap into.
library;

import 'dart:convert';

import 'package:jaspr/server.dart';
import 'package:jaspr/dom.dart';

// ---------------------------------------------------------------------------
// Init + render plumbing.
// ---------------------------------------------------------------------------

/// Idempotent Jaspr boot. `renderComponent` requires `Jaspr.initializeApp`
/// to have been called once on the current isolate, and isolates have
/// independent global state, so each session isolate calls this lazily on
/// first render. The default [ServerOptions] is what we want — we don't have
/// any `@client` components, so `jaspr_builder` would generate an
/// equivalent `defaultServerOptions` constant anyway.
void ensureJasprInit() {
  if (Jaspr.isInitialized) return;
  Jaspr.initializeApp();
}

/// Render a single Jaspr [component] tree to an HTML string suitable for an
/// HTMX OOB swap. `standalone: true` skips the `<html><head><body>` wrapping
/// `renderComponent` would otherwise emit for a full page.
Future<String> renderFragment(Component component) async {
  ensureJasprInit();
  final result = await renderComponent(component, standalone: true);
  return utf8.decode(result.body);
}

/// Standard out-of-band wrapper. HTMX looks for `hx-swap-oob` on the root
/// element of every incoming fragment and swaps it into the matching id
/// slot in the live DOM. Default swap is `innerHTML`; switch to
/// `outerHTML` to replace the slot wrapper itself.
class OobWrap extends StatelessComponent {
  const OobWrap({
    required this.targetId,
    required this.child,
    this.swap = 'innerHTML',
  });

  /// `id` of the slot HTMX should swap into (e.g. `live-counter`).
  final String targetId;
  final Component child;
  final String swap;

  @override
  Component build(BuildContext context) {
    return div(
      [child],
      id: targetId,
      attributes: {'hx-swap-oob': swap},
    );
  }
}

// ---------------------------------------------------------------------------
// Session-level chrome.
// ---------------------------------------------------------------------------

class SessionMeta extends StatelessComponent {
  const SessionMeta({
    required this.sessionId,
    required this.remoteAddr,
    required this.handshakeAgeMs,
    required this.topics,
  });

  final String sessionId;
  final String remoteAddr;
  final double handshakeAgeMs;
  final List<String> topics;

  @override
  Component build(BuildContext context) {
    return OobWrap(
      targetId: 'session-meta',
      child: dl(classes: 'meta', [
        dt([Component.text('session-id')]),
        dd([code([Component.text(sessionId)])]),
        dt([Component.text('remote')]),
        dd([code([Component.text(remoteAddr)])]),
        dt([Component.text('handshake')]),
        dd([Component.text('${handshakeAgeMs.toStringAsFixed(2)} ms')]),
        dt([Component.text('topics')]),
        dd([
          for (var i = 0; i < topics.length; i++) ...[
            if (i > 0) Component.text(', '),
            code([Component.text(topics[i])]),
          ],
        ]),
      ]),
    );
  }
}

class StatusPill extends StatelessComponent {
  const StatusPill(this.message);
  final String message;

  @override
  Component build(BuildContext context) {
    return OobWrap(
      targetId: 'session-status',
      child: span([Component.text(message)]),
    );
  }
}

class Clock extends StatelessComponent {
  const Clock(this.nowUtcIso);
  final String nowUtcIso;

  @override
  Component build(BuildContext context) {
    return OobWrap(
      targetId: 'session-clock',
      child: Component.element(
        tag: 'time',
        attributes: {'datetime': nowUtcIso},
        children: [Component.text(nowUtcIso)],
      ),
    );
  }
}

// ---------------------------------------------------------------------------
// Per-session interactive panels.
// ---------------------------------------------------------------------------

class Counter extends StatelessComponent {
  const Counter(this.value);
  final int value;

  @override
  Component build(BuildContext context) {
    return OobWrap(
      targetId: 'live-counter',
      child: div(classes: 'counter', [
        span(classes: 'value', [Component.text('$value')]),
        const form(
          attributes: {'ws-send': ''},
          [
            button(
              [Component.text('bump')],
              attributes: {'name': 'bump', 'value': '1'},
            ),
          ],
        ),
        const form(
          attributes: {'ws-send': ''},
          [
            button(
              [Component.text('reset')],
              attributes: {'name': 'reset', 'value': '1'},
            ),
          ],
        ),
      ]),
    );
  }
}

class EchoPanel extends StatelessComponent {
  const EchoPanel(this.history);
  final List<String> history;

  @override
  Component build(BuildContext context) {
    return OobWrap(
      targetId: 'echo-panel',
      child: div(classes: 'echo', [
        ul(history.isEmpty
            ? [
                li(classes: 'muted', [Component.text('no messages yet')]),
              ]
            : [for (final row in history) li([Component.text(row)])]),
        const form(
          attributes: {'ws-send': ''},
          [
            input(
              name: 'message',
              attributes: {
                'placeholder': 'say something',
                'autocomplete': 'off',
              },
            ),
            button(
              [Component.text('echo')],
              attributes: {'name': 'echo', 'value': '1'},
            ),
          ],
        ),
      ]),
    );
  }
}

class LobbyPanel extends StatelessComponent {
  const LobbyPanel(this.rows);
  final List<LobbyRow> rows;

  @override
  Component build(BuildContext context) {
    return OobWrap(
      targetId: 'lobby-panel',
      child: div(classes: 'lobby', [
        ul(rows.isEmpty
            ? [
                li(classes: 'muted', [Component.text('lobby is quiet')]),
              ]
            : [for (final row in rows) _LobbyRow(row)]),
        const form(
          attributes: {'ws-send': ''},
          [
            input(
              name: 'text',
              attributes: {
                'placeholder': 'broadcast to lobby',
                'autocomplete': 'off',
              },
            ),
            button(
              [Component.text('say')],
              attributes: {'name': 'say', 'value': '1'},
            ),
          ],
        ),
      ]),
    );
  }
}

/// Plain DTO that the session isolate's lobby pipeline already produces.
class LobbyRow {
  const LobbyRow({required this.name, required this.text, required this.self});
  final String name;
  final String text;
  final bool self;
}

class _LobbyRow extends StatelessComponent {
  const _LobbyRow(this.row);
  final LobbyRow row;

  @override
  Component build(BuildContext context) {
    return li(classes: row.self ? 'msg self' : 'msg other', [
      code([Component.text(_truncate(row.name, 12))]),
      Component.text(' '),
      span([Component.text(row.text)]),
    ]);
  }
}

class IdentityPanel extends StatelessComponent {
  const IdentityPanel({required this.userId, required this.displayName});

  final String userId;
  final String displayName;

  @override
  Component build(BuildContext context) {
    final shown = displayName.isEmpty ? userId : displayName;
    return OobWrap(
      targetId: 'identity-panel',
      child: div(classes: 'identity', [
        span(classes: 'label', [Component.text('you are')]),
        code(classes: 'uid', [Component.text(_truncate(userId, 28))]),
        span(classes: 'display', [Component.text(shown)]),
        const form(
          classes: 'identity-form',
          attributes: {'ws-send': ''},
          [
            input(
              name: 'user_id',
              attributes: {
                'placeholder': 'user id (e.g. alice)',
                'autocomplete': 'off',
              },
            ),
            input(
              name: 'display_name',
              attributes: {
                'placeholder': 'display name',
                'autocomplete': 'off',
              },
            ),
            button(
              [Component.text('identify')],
              attributes: {'name': 'identify', 'value': '1'},
            ),
          ],
        ),
      ]),
    );
  }
}

// ---------------------------------------------------------------------------
// Conversation list + active-conversation panel.
// ---------------------------------------------------------------------------

class ConvList extends StatelessComponent {
  const ConvList({required this.conversations, required this.activeId});

  final List<ConvSummary> conversations;
  final String activeId;

  @override
  Component build(BuildContext context) {
    final sorted = [...conversations]
      ..sort((lhs, rhs) => rhs.lastActivityAtUs.compareTo(lhs.lastActivityAtUs));

    return OobWrap(
      targetId: 'conv-list-panel',
      child: div(classes: 'convlist', [
        h4([Component.text('conversations')]),
        ul(
          classes: 'rows',
          sorted.isEmpty
              ? [
                  li(classes: 'muted',
                      [Component.text('no conversations — open one below')]),
                ]
              : [
                  for (final c in sorted)
                    _ConvRow(c, selected: c.id == activeId),
                ],
        ),
        const form(
          classes: 'open-form',
          attributes: {'ws-send': ''},
          [
            input(
              name: 'conversation_id',
              attributes: {
                'placeholder': 'conv id (e.g. room-42)',
                'autocomplete': 'off',
              },
            ),
            input(
              name: 'title',
              attributes: {
                'placeholder': 'title',
                'autocomplete': 'off',
              },
            ),
            button(
              [Component.text('open')],
              attributes: {'name': 'open-conv', 'value': '1'},
            ),
          ],
        ),
      ]),
    );
  }
}

class ConvSummary {
  const ConvSummary({
    required this.id,
    required this.title,
    required this.memberCount,
    required this.messageCount,
    required this.lastActivityAtUs,
  });

  final String id;
  final String title;
  final int? memberCount;
  final int messageCount;
  final int lastActivityAtUs;
}

class _ConvRow extends StatelessComponent {
  const _ConvRow(this.conv, {required this.selected});

  final ConvSummary conv;
  final bool selected;

  @override
  Component build(BuildContext context) {
    final mc = conv.memberCount?.toString() ?? '?';
    return li(classes: selected ? 'row selected' : 'row', [
      form(
        classes: 'row-actions',
        attributes: const {'ws-send': ''},
        [
          input(
            type: InputType.hidden,
            name: 'conversation_id',
            value: conv.id,
          ),
          button(
            [
              strong([Component.text(conv.title)]),
              small([Component.text('$mc members · ${conv.messageCount} msgs')]),
            ],
            classes: 'row-pick',
            attributes: const {'name': 'switch-conv', 'value': '1'},
          ),
          button(
            [Component.text('join')],
            classes: 'row-join',
            attributes: const {'name': 'join-conv', 'value': '1'},
          ),
          button(
            [Component.text('leave')],
            classes: 'row-leave',
            attributes: const {'name': 'leave-conv', 'value': '1'},
          ),
        ],
      ),
    ]);
  }
}

class ConvPanel extends StatelessComponent {
  const ConvPanel({required this.activeId, required this.messages});

  final String activeId;

  /// Pre-shaped messages — the session isolate builds these so the
  /// component stays pure.
  final List<ConvMessage> messages;

  @override
  Component build(BuildContext context) {
    if (activeId.isEmpty) {
      return OobWrap(
        targetId: 'conv-panel',
        child: div(classes: 'convpanel empty', [
          p([Component.text('no conversation selected')]),
          p(classes: 'muted',
              [Component.text('open or pick one in the directory.')]),
        ]),
      );
    }

    return OobWrap(
      targetId: 'conv-panel',
      child: div(classes: 'convpanel', [
        h4([
          Component.text('conversation '),
          code([Component.text(activeId)]),
        ]),
        ul(messages.isEmpty
            ? [
                li(classes: 'muted', [Component.text('no messages yet')]),
              ]
            : [for (final m in messages) _ConvMsg(m)]),
        form(
          attributes: const {'ws-send': ''},
          [
            input(
              type: InputType.hidden,
              name: 'conversation_id',
              value: activeId,
            ),
            input(
              name: 'text',
              attributes: {
                'placeholder': 'speak in $activeId',
                'autocomplete': 'off',
              },
            ),
            button(
              [Component.text('say')],
              attributes: const {'name': 'say-conv', 'value': '1'},
            ),
          ],
        ),
      ]),
    );
  }
}

class ConvMessage {
  const ConvMessage({required this.name, required this.text, required this.self});
  final String name;
  final String text;
  final bool self;
}

class _ConvMsg extends StatelessComponent {
  const _ConvMsg(this.msg);
  final ConvMessage msg;

  @override
  Component build(BuildContext context) {
    return li(classes: msg.self ? 'msg self' : 'msg other', [
      code([Component.text(_truncate(msg.name, 12))]),
      Component.text(' '),
      span([Component.text(msg.text)]),
    ]);
  }
}

// ---------------------------------------------------------------------------
// Tiny shared helpers.
// ---------------------------------------------------------------------------

String _truncate(String s, int n) => s.length <= n ? s : s.substring(0, n);
