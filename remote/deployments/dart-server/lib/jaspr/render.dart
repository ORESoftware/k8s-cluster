/// Thin wrapper around `package:jaspr/server.dart`'s `renderComponent` that
/// returns plain UTF-8 strings.
///
/// All `/dart/pages/*` routes go through this single entry point. Routes
/// pick a component (see `pages.dart`), pass in any per-request props, and
/// the result is written straight onto the HTTP response body.
library;

import 'dart:convert';

import 'package:jaspr/server.dart';

import 'pages.dart';

/// One-time initialisation. We have no `@client` components, so
/// `Jaspr.initializeApp()` with default [ServerOptions] is the entire
/// configuration we need — `jaspr_builder` would generate an equivalent
/// `defaultServerOptions` constant if a `.server.dart` entrypoint existed.
void _ensureInit() {
  if (Jaspr.isInitialized) return;
  Jaspr.initializeApp();
}

/// Render the SSR page identified by [route]. The route is the URL path
/// after `/dart/pages` — `/`, `/about`, `/architecture`, etc.
Future<String> renderJasprPage(String route, {Map<String, String> query = const {}}) async {
  _ensureInit();
  final component = pickPage(route, query: query);
  final result = await renderComponent(component);
  // jaspr 0.21+: `body` is a Uint8List of UTF-8 bytes.
  return utf8.decode(result.body);
}
