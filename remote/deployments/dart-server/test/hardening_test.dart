import 'package:dd_dart_server/server/conversation_registry.dart';
import 'package:dd_dart_server/server/presence.dart';
import 'package:test/test.dart';

void main() {
  group('Presence display-name lifecycle', () {
    test('drops the display name once the user has no live sessions', () {
      final presence = Presence();
      presence.bind('s1', 'anon-s1', displayName: 'anon');
      expect(presence.displayNameFor('anon-s1'), 'anon');

      presence.unbind('s1');

      // After the last session leaves, the name entry is released so the
      // `_displayNames` map can't leak one row per connection. `displayNameFor`
      // falls back to the raw user id.
      expect(presence.displayNameFor('anon-s1'), 'anon-s1');
    });

    test('keeps the display name while another session is still bound', () {
      final presence = Presence();
      presence.bind('s1', 'alice', displayName: 'Alice');
      presence.bind('s2', 'alice', displayName: 'Alice');

      presence.unbind('s1');

      // alice still has s2 online, so the friendly name survives.
      expect(presence.displayNameFor('alice'), 'Alice');
      expect(presence.isOnline('alice'), isTrue);

      presence.unbind('s2');
      expect(presence.displayNameFor('alice'), 'alice');
      expect(presence.isOnline('alice'), isFalse);
    });
  });

  group('ConversationRegistry member cap', () {
    test('refuses members past the per-conversation cap', () {
      final reg = ConversationRegistry(maxMembersPerConversation: 2);
      reg.upsert(
        conversationId: 'c1',
        title: 'c1',
        kind: 'chat',
        createdByUserId: 'u0',
      );

      expect(reg.addMember('c1', 'u1'), isTrue);
      expect(reg.addMember('c1', 'u2'), isTrue);
      // Third distinct member is over the cap → refused, no growth.
      expect(reg.addMember('c1', 'u3'), isFalse);
      expect(reg.members('c1'), hasLength(2));
      expect(reg.members('c1'), isNot(contains('u3')));
      // The refused user gains no reverse-index entry either.
      expect(reg.conversationsForUser('u3'), isEmpty);
    });

    test('re-adding an existing member is an idempotent no-op, not a refusal',
        () {
      final reg = ConversationRegistry(maxMembersPerConversation: 1);
      expect(reg.addMember('c1', 'u1'), isTrue);
      // Same member again: returns false (already present), set unchanged.
      expect(reg.addMember('c1', 'u1'), isFalse);
      expect(reg.members('c1'), hasLength(1));
    });
  });
}
