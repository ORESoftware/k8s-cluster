import 'package:flutter/material.dart';
import 'package:rxdart/rxdart.dart';

import 'wss_client.dart';

void main() {
  runApp(const DartServerApp());
}

class DartServerApp extends StatelessWidget {
  const DartServerApp({super.key});

  @override
  Widget build(BuildContext context) {
    return MaterialApp(
      title: 'dd-dart-server SPA',
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
      home: const HomeShell(),
    );
  }
}

class HomeShell extends StatefulWidget {
  const HomeShell({super.key});

  @override
  State<HomeShell> createState() => _HomeShellState();
}

class _HomeShellState extends State<HomeShell> {
  late final WssClient _client;
  final _echoController = TextEditingController();
  final _lobbyController = TextEditingController();
  final _userIdController = TextEditingController();
  final _displayNameController = TextEditingController();
  final _newConvIdController = TextEditingController();
  final _newConvTitleController = TextEditingController();
  final _convMessageController = TextEditingController();

  @override
  void initState() {
    super.initState();
    final base = Uri.base;
    final scheme = base.scheme == 'https' ? 'wss' : 'ws';
    _client = WssClient(Uri.parse('$scheme://${base.host}:${base.port}/dart/wss'));
    _client.connect();
  }

  @override
  void dispose() {
    _echoController.dispose();
    _lobbyController.dispose();
    _userIdController.dispose();
    _displayNameController.dispose();
    _newConvIdController.dispose();
    _newConvTitleController.dispose();
    _convMessageController.dispose();
    _client.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      appBar: AppBar(
        title: const Text('dd-dart-server SPA'),
        actions: [
          StreamBuilder<Identity>(
            stream: _client.identity,
            initialData: _client.identity.value,
            builder: (context, snap) {
              final id = snap.data ?? const Identity(userId: '', displayName: '');
              final shown =
                  id.displayName.isNotEmpty ? id.displayName : id.userId;
              return Padding(
                padding: const EdgeInsets.only(right: 16),
                child: Chip(
                  avatar: const Icon(Icons.person, size: 14),
                  label: Text(shown.isEmpty ? 'connecting…' : shown),
                ),
              );
            },
          ),
          StreamBuilder<ConnectionState>(
            stream: _client.connection,
            initialData: ConnectionState.disconnected,
            builder: (context, snap) {
              final s = snap.data ?? ConnectionState.disconnected;
              final color = switch (s) {
                ConnectionState.connected => Colors.greenAccent,
                ConnectionState.connecting => Colors.amberAccent,
                ConnectionState.disconnected => Colors.redAccent,
              };
              return Padding(
                padding: const EdgeInsets.only(right: 16),
                child: Row(
                  children: [
                    Icon(Icons.circle, size: 12, color: color),
                    const SizedBox(width: 6),
                    Text(s.name),
                  ],
                ),
              );
            },
          ),
        ],
      ),
      body: LayoutBuilder(
        builder: (context, constraints) {
          final wide = constraints.maxWidth >= 760;
          final cards = <Widget>[
            _MetaCard(client: _client),
            _IdentityCard(
              client: _client,
              userIdController: _userIdController,
              displayNameController: _displayNameController,
            ),
            _CounterCard(client: _client),
            _EchoCard(client: _client, controller: _echoController),
            _LobbyCard(client: _client, controller: _lobbyController),
            _ConversationsCard(
              client: _client,
              newIdController: _newConvIdController,
              newTitleController: _newConvTitleController,
            ),
            _ConversationPanel(
              client: _client,
              controller: _convMessageController,
            ),
          ];
          if (wide) {
            return GridView.count(
              padding: const EdgeInsets.all(16),
              crossAxisCount: 2,
              mainAxisSpacing: 16,
              crossAxisSpacing: 16,
              childAspectRatio: 1.2,
              children: cards,
            );
          }
          return ListView.separated(
            padding: const EdgeInsets.all(16),
            itemCount: cards.length,
            separatorBuilder: (_, __) => const SizedBox(height: 16),
            itemBuilder: (_, i) => cards[i],
          );
        },
      ),
      bottomNavigationBar: _StatusBar(client: _client),
    );
  }
}

class _Card extends StatelessWidget {
  const _Card({required this.title, required this.child});
  final String title;
  final Widget child;

  @override
  Widget build(BuildContext context) {
    return Card(
      child: Padding(
        padding: const EdgeInsets.all(16),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Text(
              title.toUpperCase(),
              style: Theme.of(context).textTheme.labelSmall?.copyWith(
                    letterSpacing: 1.2,
                    color: Colors.grey,
                  ),
            ),
            const SizedBox(height: 8),
            Expanded(child: child),
          ],
        ),
      ),
    );
  }
}

class _MetaCard extends StatelessWidget {
  const _MetaCard({required this.client});
  final WssClient client;

  @override
  Widget build(BuildContext context) {
    return _Card(
      title: 'session metadata',
      child: StreamBuilder<Map<String, String>>(
        stream: client.meta,
        initialData: client.meta.value,
        builder: (context, snap) {
          final entries = (snap.data ?? const {}).entries.toList();
          if (entries.isEmpty) {
            return const Center(child: Text('waiting for first frame…'));
          }
          return ListView(
            children: [
              for (final e in entries)
                Padding(
                  padding: const EdgeInsets.symmetric(vertical: 2),
                  child: Row(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      SizedBox(
                        width: 110,
                        child: Text(
                          e.key,
                          style: const TextStyle(color: Colors.grey),
                        ),
                      ),
                      Expanded(
                        child: SelectableText(
                          e.value,
                          style: const TextStyle(fontFamily: 'monospace'),
                        ),
                      ),
                    ],
                  ),
                ),
              const SizedBox(height: 8),
              StreamBuilder<String>(
                stream: client.clock,
                initialData: client.clock.value,
                builder: (context, snap) => Text(
                  'server clock: ${snap.data ?? ""}',
                  style: const TextStyle(color: Colors.grey, fontSize: 12),
                ),
              ),
            ],
          );
        },
      ),
    );
  }
}

class _IdentityCard extends StatelessWidget {
  const _IdentityCard({
    required this.client,
    required this.userIdController,
    required this.displayNameController,
  });

  final WssClient client;
  final TextEditingController userIdController;
  final TextEditingController displayNameController;

  @override
  Widget build(BuildContext context) {
    return _Card(
      title: 'identity (presence index)',
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          StreamBuilder<Identity>(
            stream: client.identity,
            initialData: client.identity.value,
            builder: (context, snap) {
              final id = snap.data ?? const Identity(userId: '', displayName: '');
              return Wrap(
                spacing: 8,
                runSpacing: 4,
                crossAxisAlignment: WrapCrossAlignment.center,
                children: [
                  const Text('you are', style: TextStyle(color: Colors.grey)),
                  SelectableText(
                    id.userId.isEmpty ? '—' : id.userId,
                    style: const TextStyle(fontFamily: 'monospace'),
                  ),
                  if (id.displayName.isNotEmpty)
                    Chip(label: Text(id.displayName)),
                ],
              );
            },
          ),
          const SizedBox(height: 8),
          TextField(
            controller: userIdController,
            decoration: const InputDecoration(
              hintText: 'user id (e.g. alice)',
              isDense: true,
            ),
          ),
          const SizedBox(height: 8),
          TextField(
            controller: displayNameController,
            decoration: const InputDecoration(
              hintText: 'display name',
              isDense: true,
            ),
          ),
          const SizedBox(height: 8),
          Align(
            alignment: Alignment.centerLeft,
            child: FilledButton(
              onPressed: () {
                client.identify(
                  userId: userIdController.text,
                  displayName: displayNameController.text,
                );
              },
              child: const Text('identify'),
            ),
          ),
        ],
      ),
    );
  }
}

class _CounterCard extends StatelessWidget {
  const _CounterCard({required this.client});
  final WssClient client;

  @override
  Widget build(BuildContext context) {
    return _Card(
      title: 'live counter (per-isolate state)',
      child: Column(
        mainAxisAlignment: MainAxisAlignment.center,
        children: [
          StreamBuilder<int>(
            stream: client.counter,
            initialData: client.counter.value,
            builder: (context, snap) => Text(
              '${snap.data ?? 0}',
              style: const TextStyle(fontSize: 56, fontWeight: FontWeight.w600),
            ),
          ),
          const SizedBox(height: 12),
          Wrap(
            spacing: 8,
            children: [
              FilledButton(
                onPressed: () => client.send(triggerName: 'bump'),
                child: const Text('bump'),
              ),
              OutlinedButton(
                onPressed: () => client.send(triggerName: 'reset'),
                child: const Text('reset'),
              ),
            ],
          ),
        ],
      ),
    );
  }
}

class _EchoCard extends StatelessWidget {
  const _EchoCard({required this.client, required this.controller});
  final WssClient client;
  final TextEditingController controller;

  @override
  Widget build(BuildContext context) {
    return _Card(
      title: 'echo (per-session)',
      child: Column(
        children: [
          Expanded(
            child: StreamBuilder<List<String>>(
              stream: client.echo,
              initialData: client.echo.value,
              builder: (context, snap) {
                final rows = snap.data ?? const <String>[];
                if (rows.isEmpty) {
                  return const Center(child: Text('no messages yet'));
                }
                return ListView.builder(
                  itemCount: rows.length,
                  itemBuilder: (_, i) => ListTile(
                    dense: true,
                    title: Text(rows[i]),
                  ),
                );
              },
            ),
          ),
          const SizedBox(height: 8),
          Row(
            children: [
              Expanded(
                child: TextField(
                  controller: controller,
                  decoration: const InputDecoration(hintText: 'echo…'),
                  onSubmitted: (_) => _send(),
                ),
              ),
              const SizedBox(width: 8),
              FilledButton(onPressed: _send, child: const Text('echo')),
            ],
          ),
        ],
      ),
    );
  }

  void _send() {
    final txt = controller.text.trim();
    if (txt.isEmpty) return;
    client.send(triggerName: 'echo', fields: {'message': txt});
    controller.clear();
  }
}

class _LobbyCard extends StatelessWidget {
  const _LobbyCard({required this.client, required this.controller});
  final WssClient client;
  final TextEditingController controller;

  @override
  Widget build(BuildContext context) {
    return _Card(
      title: 'lobby (cross-isolate :pg fanout)',
      child: Column(
        children: [
          Expanded(
            child: StreamBuilder<List<LobbyEntry>>(
              stream: client.lobby,
              initialData: client.lobby.value,
              builder: (context, snap) {
                final rows = snap.data ?? const <LobbyEntry>[];
                if (rows.isEmpty) {
                  return const Center(child: Text('lobby is quiet'));
                }
                return ListView.builder(
                  itemCount: rows.length,
                  itemBuilder: (_, i) {
                    final e = rows[i];
                    return ListTile(
                      dense: true,
                      leading: CircleAvatar(
                        radius: 12,
                        backgroundColor:
                            e.self ? Colors.greenAccent : Colors.blueAccent,
                        child: const SizedBox.shrink(),
                      ),
                      title: Text(e.text),
                      subtitle: Text(
                        e.from,
                        style: const TextStyle(fontFamily: 'monospace'),
                      ),
                    );
                  },
                );
              },
            ),
          ),
          const SizedBox(height: 8),
          Row(
            children: [
              Expanded(
                child: TextField(
                  controller: controller,
                  decoration:
                      const InputDecoration(hintText: 'broadcast to lobby…'),
                  onSubmitted: (_) => _send(),
                ),
              ),
              const SizedBox(width: 8),
              FilledButton(onPressed: _send, child: const Text('say')),
            ],
          ),
        ],
      ),
    );
  }

  void _send() {
    final txt = controller.text.trim();
    if (txt.isEmpty) return;
    client.send(triggerName: 'say', fields: {'text': txt});
    controller.clear();
  }
}

class _ConversationsCard extends StatelessWidget {
  const _ConversationsCard({
    required this.client,
    required this.newIdController,
    required this.newTitleController,
  });
  final WssClient client;
  final TextEditingController newIdController;
  final TextEditingController newTitleController;

  @override
  Widget build(BuildContext context) {
    return _Card(
      title: 'conversations / topics / groups',
      child: Column(
        children: [
          Expanded(
            child: StreamBuilder<Map<String, Map<String, Object?>>>(
              stream: client.conversations,
              initialData: client.conversations.value,
              builder: (context, snap) {
                final dir = snap.data ?? const {};
                if (dir.isEmpty) {
                  return const Center(
                    child: Text('no conversations — open one below'),
                  );
                }
                final entries = dir.entries.toList();
                return StreamBuilder<String>(
                  stream: client.activeConversationId,
                  initialData: client.activeConversationId.value,
                  builder: (context, activeSnap) {
                    final active = activeSnap.data ?? '';
                    return ListView.builder(
                      itemCount: entries.length,
                      itemBuilder: (_, i) {
                        final id = entries[i].key;
                        final m = entries[i].value;
                        final title = (m['title'] as String?) ?? id;
                        final mc = m['memberCount'] ?? '?';
                        final msgs = m['messageCount'] ?? 0;
                        final isActive = id == active;
                        return Material(
                          color: isActive
                              ? Theme.of(context)
                                  .colorScheme
                                  .primary
                                  .withValues(alpha: 0.1)
                              : Colors.transparent,
                          child: InkWell(
                            onTap: () => client.switchConversation(id),
                            child: Padding(
                              padding: const EdgeInsets.symmetric(
                                  vertical: 6, horizontal: 4),
                              child: Row(
                                children: [
                                  Expanded(
                                    child: Column(
                                      crossAxisAlignment:
                                          CrossAxisAlignment.start,
                                      children: [
                                        Text(
                                          title,
                                          style: const TextStyle(
                                              fontWeight: FontWeight.w600),
                                        ),
                                        Text(
                                          '$mc members · $msgs msgs · $id',
                                          style: const TextStyle(
                                            color: Colors.grey,
                                            fontSize: 11,
                                            fontFamily: 'monospace',
                                          ),
                                        ),
                                      ],
                                    ),
                                  ),
                                  TextButton(
                                    onPressed: () =>
                                        client.joinConversation(id),
                                    child: const Text('join'),
                                  ),
                                  TextButton(
                                    onPressed: () =>
                                        client.leaveConversation(id),
                                    child: const Text('leave'),
                                  ),
                                ],
                              ),
                            ),
                          ),
                        );
                      },
                    );
                  },
                );
              },
            ),
          ),
          const SizedBox(height: 8),
          Row(
            children: [
              Expanded(
                child: TextField(
                  controller: newIdController,
                  decoration: const InputDecoration(
                    hintText: 'conv id (e.g. room-42)',
                    isDense: true,
                  ),
                ),
              ),
              const SizedBox(width: 8),
              Expanded(
                child: TextField(
                  controller: newTitleController,
                  decoration: const InputDecoration(
                    hintText: 'title',
                    isDense: true,
                  ),
                ),
              ),
              const SizedBox(width: 8),
              FilledButton(
                onPressed: () {
                  client.openConversation(
                    conversationId: newIdController.text,
                    title: newTitleController.text,
                  );
                  newIdController.clear();
                  newTitleController.clear();
                },
                child: const Text('open'),
              ),
            ],
          ),
        ],
      ),
    );
  }
}

class _ConversationPanel extends StatelessWidget {
  const _ConversationPanel({required this.client, required this.controller});
  final WssClient client;
  final TextEditingController controller;

  @override
  Widget build(BuildContext context) {
    return _Card(
      title: 'active conversation',
      child: StreamBuilder<String>(
        stream: client.activeConversationId,
        initialData: client.activeConversationId.value,
        builder: (context, activeSnap) {
          final active = activeSnap.data ?? '';
          if (active.isEmpty) {
            return const Center(
              child: Text(
                'no conversation selected',
                style: TextStyle(color: Colors.grey),
              ),
            );
          }
          return Column(
            children: [
              Padding(
                padding: const EdgeInsets.only(bottom: 8),
                child: Row(
                  children: [
                    const Icon(Icons.tag, size: 16),
                    const SizedBox(width: 6),
                    Expanded(
                      child: SelectableText(
                        active,
                        style: const TextStyle(fontFamily: 'monospace'),
                      ),
                    ),
                  ],
                ),
              ),
              Expanded(
                child: StreamBuilder<Map<String, List<ConversationEntry>>>(
                  stream: client.conversationMessages,
                  initialData: client.conversationMessages.value,
                  builder: (context, snap) {
                    final rows = (snap.data ?? const {})[active] ??
                        const <ConversationEntry>[];
                    if (rows.isEmpty) {
                      return const Center(child: Text('no messages yet'));
                    }
                    return ListView.builder(
                      itemCount: rows.length,
                      itemBuilder: (_, i) {
                        final e = rows[i];
                        return ListTile(
                          dense: true,
                          leading: CircleAvatar(
                            radius: 12,
                            backgroundColor: e.self
                                ? Colors.greenAccent
                                : Colors.blueAccent,
                            child: const SizedBox.shrink(),
                          ),
                          title: Text(e.text),
                          subtitle: Text(
                            e.from,
                            style: const TextStyle(fontFamily: 'monospace'),
                          ),
                        );
                      },
                    );
                  },
                ),
              ),
              Row(
                children: [
                  Expanded(
                    child: TextField(
                      controller: controller,
                      decoration: InputDecoration(hintText: 'speak in $active'),
                      onSubmitted: (_) => _send(active),
                    ),
                  ),
                  const SizedBox(width: 8),
                  FilledButton(
                    onPressed: () => _send(active),
                    child: const Text('say'),
                  ),
                ],
              ),
            ],
          );
        },
      ),
    );
  }

  void _send(String active) {
    final txt = controller.text.trim();
    if (txt.isEmpty || active.isEmpty) return;
    client.sayInConversation(conversationId: active, text: txt);
    controller.clear();
  }
}

class _StatusBar extends StatelessWidget {
  const _StatusBar({required this.client});
  final WssClient client;

  @override
  Widget build(BuildContext context) {
    final stream = Rx.combineLatest3<String, String, Identity, String>(
      client.status,
      client.clock,
      client.identity,
      (s, c, id) {
        final who = id.displayName.isNotEmpty
            ? id.displayName
            : (id.userId.isEmpty ? '—' : id.userId);
        return '$who · $s · $c';
      },
    );
    return BottomAppBar(
      child: StreamBuilder<String>(
        stream: stream,
        initialData: '— · idle · ',
        builder: (context, snap) => Padding(
          padding: const EdgeInsets.symmetric(horizontal: 16),
          child: Text(
            snap.data ?? '',
            style: const TextStyle(fontFamily: 'monospace', fontSize: 12),
          ),
        ),
      ),
    );
  }
}
