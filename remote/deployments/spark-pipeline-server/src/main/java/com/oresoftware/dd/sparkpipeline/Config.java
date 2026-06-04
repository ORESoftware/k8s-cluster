package com.oresoftware.dd.sparkpipeline;

import org.slf4j.Logger;
import org.slf4j.LoggerFactory;

/**
 * Immutable runtime configuration read once from the environment.
 */
public final class Config {

  private static final Logger log = LoggerFactory.getLogger(Config.class);

  public final String httpHost;
  public final int httpPort;
  public final String serverAuthSecret;
  public final boolean allowUnauthenticated;

  private Config(final String httpHost,
                 final int httpPort,
                 final String serverAuthSecret,
                 final boolean allowUnauthenticated) {
    this.httpHost = httpHost;
    this.httpPort = httpPort;
    this.serverAuthSecret = serverAuthSecret;
    this.allowUnauthenticated = allowUnauthenticated;
  }

  public static Config fromEnv() {
    final String secret = trimToNull(System.getenv("SERVER_AUTH_SECRET"));
    final boolean allowUnauthenticated =
        readBool("SPARK_PIPELINE_ALLOW_UNAUTHENTICATED", false);

    if (secret == null && !allowUnauthenticated) {
      log.warn(
          "SERVER_AUTH_SECRET is unset and SPARK_PIPELINE_ALLOW_UNAUTHENTICATED is false; "
              + "non-probe routes will reject every request until a secret is provided");
    }

    return new Config(
        System.getenv().getOrDefault("HTTP_HOST", "0.0.0.0"),
        readInt("HTTP_PORT", 8085),
        secret,
        allowUnauthenticated);
  }

  private static String trimToNull(final String raw) {
    if (raw == null) {
      return null;
    }
    final String trimmed = raw.trim();
    return trimmed.isEmpty() ? null : trimmed;
  }

  private static int readInt(final String name, final int fallback) {
    final String raw = trimToNull(System.getenv(name));
    if (raw == null) {
      return fallback;
    }
    try {
      return Integer.parseInt(raw);
    } catch (NumberFormatException nfe) {
      log.warn("invalid {}={}; using {}", name, raw, fallback);
      return fallback;
    }
  }

  private static boolean readBool(final String name, final boolean fallback) {
    final String raw = trimToNull(System.getenv(name));
    if (raw == null) {
      return fallback;
    }
    return raw.equalsIgnoreCase("true") || raw.equals("1") || raw.equalsIgnoreCase("yes");
  }
}
