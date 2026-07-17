/// Inbound HTMX/WebSocket payload helpers.
///
/// HTMX over WebSockets is bidirectional with two distinct payload shapes:
///
///   1. **Outbound (server → browser)**: HTML strings. We produce these
///      via real Jaspr `StatelessComponent`s in
///      `lib/server/wss_components.dart` and render them with
///      `renderFragment`. **No string concatenation; no manual escaping.**
///
///   2. **Inbound (browser → server)**: JSON objects produced by `ws-send`.
///      HTMX serialises form fields into a JSON object and adds a
///      `HEADERS` map describing the trigger. [parseHtmxInboundJson]
///      decodes that shape into [HtmxInbound].
///
/// This file owns only inbound parsing now — the outbound side moved
/// entirely to Jaspr components after the "components, not strings"
/// refactor.
library;

import 'dart:convert';

/// Decoded view of an HTMX `ws-send` JSON payload.
///
/// HTMX serialises form data plus a `HEADERS` object that carries:
///
///   * HX-Request:        always "true"
///   * HX-Trigger:        id of the element that produced the event
///   * HX-Trigger-Name:   name attribute of that element
///   * HX-Target:         id of the hx-target (if any)
///   * HX-Current-URL:    page url at the moment of the send
final class HtmxInbound {
  const HtmxInbound({
    required this.fields,
    required this.trigger,
    required this.triggerName,
    required this.target,
    required this.currentUrl,
  });

  /// All non-`HEADERS` keys of the payload — i.e. the form fields.
  final Map<String, Object?> fields;

  final String? trigger;
  final String? triggerName;
  final String? target;
  final String? currentUrl;

  /// Reads a string field with a default when absent or non-string.
  String stringField(String name, [String fallback = '']) {
    final v = fields[name];
    if (v is String) return v;
    return fallback;
  }
}

/// Parse the JSON payload that HTMX sends over a `ws-send` form. Returns
/// `null` when the payload is not an object (e.g. heartbeat blanks).
HtmxInbound? parseHtmxInboundJson(String text) {
  if (text.isEmpty) return null;
  late final Object? decoded;
  try {
    decoded = jsonDecode(text);
  } catch (_) {
    return null;
  }
  if (decoded is! Map) return null;

  final headers = decoded['HEADERS'];
  final fields = <String, Object?>{};
  for (final entry in decoded.entries) {
    if (entry.key == 'HEADERS') continue;
    fields[entry.key as String] = entry.value;
  }

  String? hdr(String key) {
    if (headers is Map) {
      final v = headers[key];
      if (v is String && v.isNotEmpty) return v;
    }
    return null;
  }

  return HtmxInbound(
    fields: fields,
    trigger: hdr('HX-Trigger'),
    triggerName: hdr('HX-Trigger-Name'),
    target: hdr('HX-Target'),
    currentUrl: hdr('HX-Current-URL'),
  );
}
