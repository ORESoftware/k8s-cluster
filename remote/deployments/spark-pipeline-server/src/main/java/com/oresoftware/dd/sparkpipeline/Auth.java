package com.oresoftware.dd.sparkpipeline;

import io.vertx.ext.web.RoutingContext;

import java.nio.charset.StandardCharsets;
import java.security.MessageDigest;

/**
 * Shared-secret gate for pipeline API routes.
 *
 * <p>Health and metrics endpoints stay unauthenticated for Kubernetes probes and Prometheus.
 * Pipeline job and repo endpoints require the same gateway/server secret used by neighboring
 * dd-* services unless {@code SPARK_PIPELINE_ALLOW_UNAUTHENTICATED=true} is set for local tests.
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
      candidate = ctx.request().getHeader("auth");
    }
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

  static boolean constantTimeEquals(final String a, final String b) {
    final byte[] left = a.getBytes(StandardCharsets.UTF_8);
    final byte[] right = b.getBytes(StandardCharsets.UTF_8);
    return MessageDigest.isEqual(left, right);
  }
}
