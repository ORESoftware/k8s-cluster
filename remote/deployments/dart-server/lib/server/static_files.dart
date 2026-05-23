/// Static file server backing `/dart/assets/*` and `/dart/app/*`.
///
/// Reads from a base directory configured by the `STATIC_DIR` environment
/// variable (default `./public`), which the Dockerfile points at the
/// Flutter web `build/web` output. Path traversal is blocked by resolving
/// every request through `path.normalize` and rejecting anything that
/// escapes the base.
library;

import 'dart:async';
import 'dart:io';

import 'package:mime/mime.dart';
import 'package:path/path.dart' as p;

class StaticFileServer {
  StaticFileServer(this.baseDir);

  final Directory baseDir;

  /// Serve `requestPath` (already stripped of any URL prefix). Returns
  /// `false` when nothing matched so the caller can return a 404 itself.
  Future<bool> tryServe(
    HttpRequest req, {
    required String requestPath,
    String fallbackHtml = 'index.html',
  }) async {
    var rel = requestPath;
    if (rel.startsWith('/')) rel = rel.substring(1);
    if (rel.isEmpty) rel = fallbackHtml;

    final base = baseDir.absolute.path;
    final resolved = p.normalize(p.join(base, rel));
    if (!p.isWithin(base, resolved) && resolved != base) {
      req.response
        ..statusCode = HttpStatus.forbidden
        ..headers.contentType = ContentType.text
        ..write('forbidden\n');
      await req.response.close();
      return true;
    }

    var file = File(resolved);
    if (!await file.exists()) {
      // SPA fallback: serve the bundle's index.html for any path that
      // doesn't map to a real file. Lets the Flutter app handle its own
      // client-side routing.
      final fallback = File(p.join(base, fallbackHtml));
      if (await fallback.exists()) {
        file = fallback;
      } else {
        return false;
      }
    }

    final stat = await file.stat();
    if (stat.type == FileSystemEntityType.directory) {
      final dirIndex = File(p.join(file.path, fallbackHtml));
      if (!await dirIndex.exists()) return false;
      file = dirIndex;
    }

    final mime = lookupMimeType(file.path) ?? 'application/octet-stream';
    req.response.statusCode = HttpStatus.ok;
    req.response.headers
      ..contentType = ContentType.parse(mime)
      ..set('cache-control', _cacheControlFor(file.path))
      // The Flutter SW relies on a same-origin scope for /dart/app/. Make
      // sure we always emit that header to avoid stale cross-origin denies
      // when the page is served behind a TLS termination proxy.
      ..set('service-worker-allowed', '/dart/app/');

    await req.response.addStream(file.openRead());
    await req.response.close();
    return true;
  }
}

String _cacheControlFor(String filePath) {
  final base = p.basename(filePath);
  // Flutter web fingerprints `main.dart.js`, `flutter_bootstrap.js`, and the
  // canvaskit/skwasm bundles via querystrings, so they're safe to cache hard.
  // The service worker file itself MUST NOT be cached, otherwise users get
  // pinned to an outdated bundle indefinitely.
  if (base == 'flutter_service_worker.js') {
    return 'no-cache, no-store, must-revalidate';
  }
  if (base == 'index.html' ||
      base == 'manifest.json' ||
      base == 'version.json') {
    return 'no-cache';
  }
  return 'public, max-age=31536000, immutable';
}
