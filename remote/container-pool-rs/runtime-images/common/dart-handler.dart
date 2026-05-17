import 'dart:convert';
import 'dart:io';

Future<void> main() async {
  final input = await stdin.transform(utf8.decoder).join();
  stdout.writeln(jsonEncode({
    'ok': true,
    'runtime': 'dart',
    'pid': pid,
    'receivedBytes': utf8.encode(input).length,
  }));
}
