import 'package:dd_dart_server/server/wss_components.dart';
import 'package:test/test.dart';

void main() {
  group('renderFragment', () {
    test('IdentityPanel emits OOB wrapper + escapes tag-bearing input', () async {
      final html = await renderFragment(
        const IdentityPanel(
          userId: 'alice<script>',
          displayName: 'A & B',
        ),
      );

      expect(
        html,
        contains('<div id="identity-panel" hx-swap-oob="innerHTML">'),
      );
      expect(html, contains('class="identity"'));

      // No raw `<script>` survives the renderer; angle brackets are escaped
      // in text nodes.
      expect(html, isNot(contains('<script>')));
      expect(html, contains('alice&lt;script&gt;'));

      // Ampersands are escaped in text content (otherwise `&` would start an
      // entity reference).
      expect(html, contains('A &amp; B'));

      // The `ws-send` HTMX flag attribute is rendered as a bare attribute.
      expect(html, contains('<form class="identity-form" ws-send>'));
    });

    test('IdentityPanel escapes attribute-context user input', () async {
      // Construct a value that would break out of an `attribute="..."`
      // context if it weren't escaped. We fold it through `value:` on a
      // hidden input via the conv-row component, which is the only place we
      // currently set a user-controlled attribute value.
      final html = await renderFragment(const ConvList(
        conversations: [
          ConvSummary(
            id: 'evil"id',
            title: 'malicious "title"',
            memberCount: 0,
            messageCount: 0,
            lastActivityAtUs: 1,
          ),
        ],
        activeId: '',
      ));

      // The hidden input's `value` attribute carries the conv id — quote
      // chars in the id must NOT escape the attribute-quoting context.
      expect(html, isNot(contains('value="evil"id"')));
      expect(html, contains('value="evil&quot;id"'));

      // Quotes inside text nodes (h1/title) don't need escaping per the
      // HTML spec, so we only assert the dangerous attribute-context case.
    });

    test('Counter renders value + bump/reset forms', () async {
      final html = await renderFragment(const Counter(7));

      expect(html, contains('<div id="live-counter" hx-swap-oob="innerHTML">'));
      expect(html, contains('<span class="value">7</span>'));
      expect(html, contains('name="bump"'));
      expect(html, contains('name="reset"'));
    });

    test('LobbyPanel marks self vs other rows', () async {
      final html = await renderFragment(LobbyPanel(const [
        LobbyRow(name: 'alice', text: 'hi', self: true),
        LobbyRow(name: 'bob', text: '<b>hey</b>', self: false),
      ]));

      expect(html, contains('<li class="msg self">'));
      expect(html, contains('<li class="msg other">'));
      // Tag-like text in messages is escaped, not interpreted as markup.
      expect(html, contains('&lt;b&gt;hey&lt;/b&gt;'));
      expect(html, isNot(contains('<b>hey</b>')));
    });

    test('ConvList sorts by lastActivityAtUs descending', () async {
      final html = await renderFragment(const ConvList(
        conversations: [
          ConvSummary(
            id: 'old',
            title: 'OLD',
            memberCount: 1,
            messageCount: 1,
            lastActivityAtUs: 1,
          ),
          ConvSummary(
            id: 'new',
            title: 'NEW',
            memberCount: 2,
            messageCount: 5,
            lastActivityAtUs: 100,
          ),
        ],
        activeId: 'new',
      ));

      final newPos = html.indexOf('NEW');
      final oldPos = html.indexOf('OLD');
      expect(newPos, greaterThanOrEqualTo(0));
      expect(oldPos, greaterThan(newPos),
          reason: 'NEW (more recent) must render before OLD');

      // Selected row carries the right class.
      expect(html, contains('<li class="row selected">'));
    });

    test('ConvPanel empty state has the muted helper copy', () async {
      final html = await renderFragment(const ConvPanel(
        activeId: '',
        messages: <ConvMessage>[],
      ));
      expect(html, contains('<div id="conv-panel" hx-swap-oob="innerHTML">'));
      expect(html, contains('no conversation selected'));
    });
  });
}
