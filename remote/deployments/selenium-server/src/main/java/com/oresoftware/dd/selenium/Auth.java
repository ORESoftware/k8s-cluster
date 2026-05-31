package com.oresoftware.dd.selenium;

import io.vertx.ext.web.RoutingContext;

import java.nio.charset.StandardCharsets;
import java.security.MessageDigest;

/**
 * Shared-secret gate for {@code POST /run}.
 *
 * <p>Mirrors dd-browser-test-server: accept the secret from {@code x-server-auth} or a
 * {@code Bearer} {@code authorization} header, compare in constant time, and fail closed when no
 * secret is configured (unless {@code SELENIUM_ALLOW_UNAUTHENTICATED=true}).
 */
public final class Auth {

  private final Config config;

  public Auth(final Config config) {
    this.config = config;
  }

  public boolean isAuthorized(final RoutingContext ctx) {
    if (config.allowUnauthenticated) {
      return true;
    }
    if (config.serverAuthSecret == null) {
      return false;
    }

    String candidate = ctx.request().getHeader("x-server-auth");
    if (candidate == null) {
      candidate = ctx.request().getHeader("authorization");
    }
    if (candidate == null) {
      candidate = ctx.request().getHeader("x-auth");
    }
    if (candidate == null) {
      return false;
    }

    final String provided = candidate.replaceFirst("(?i)^Bearer\\s+", "");
    return constantTimeEquals(provided, config.serverAuthSecret);
  }

  private static boolean constantTimeEquals(final String a, final String b) {
    final byte[] left = a.getBytes(StandardCharsets.UTF_8);
    final byte[] right = b.getBytes(StandardCharsets.UTF_8);
    return MessageDigest.isEqual(left, right);
  }
}
