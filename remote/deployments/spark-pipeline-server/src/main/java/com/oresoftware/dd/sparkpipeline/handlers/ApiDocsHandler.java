package com.oresoftware.dd.sparkpipeline.handlers;

import io.vertx.core.Handler;
import io.vertx.ext.web.RoutingContext;

import java.io.IOException;
import java.io.InputStream;
import java.nio.charset.StandardCharsets;

/**
 * Serves generated API documentation embedded in the shaded jar.
 */
public final class ApiDocsHandler {

  private static final String API_DOCS_HTML = readResource("/api-docs.html");
  private static final String API_DOCS_JSON = readResource("/api-docs.json");

  private ApiDocsHandler() {
  }

  public static Handler<RoutingContext> html() {
    return ctx -> serve(ctx, "text/html; charset=utf-8", API_DOCS_HTML);
  }

  public static Handler<RoutingContext> json() {
    return ctx -> serve(ctx, "application/json; charset=utf-8", API_DOCS_JSON);
  }

  private static void serve(final RoutingContext ctx, final String contentType, final String body) {
    ctx.response()
        .putHeader("content-type", contentType)
        .end(body);
  }

  private static String readResource(final String path) {
    try (InputStream stream = ApiDocsHandler.class.getResourceAsStream(path)) {
      if (stream == null) {
        throw new IllegalStateException("missing generated API docs resource: " + path);
      }
      return new String(stream.readAllBytes(), StandardCharsets.UTF_8);
    } catch (IOException error) {
      throw new IllegalStateException("failed to read generated API docs resource: " + path, error);
    }
  }
}
