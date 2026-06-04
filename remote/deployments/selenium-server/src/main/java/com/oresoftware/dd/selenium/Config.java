package com.oresoftware.dd.selenium;

import org.slf4j.Logger;
import org.slf4j.LoggerFactory;

/**
 * Immutable runtime configuration read once from the environment.
 *
 * <p>The defaults mirror dd-browser-test-server's Selenium driver so the two services behave the
 * same way; only the env var prefix differs ({@code SELENIUM_*} instead of {@code BROWSER_TEST_*}).
 */
public final class Config {

  private static final Logger log = LoggerFactory.getLogger(Config.class);

  public final String httpHost;
  public final int httpPort;

  /** Shared dd-agent secret. Required unless {@link #allowUnauthenticated} is set. */
  public final String serverAuthSecret;
  public final boolean allowUnauthenticated;

  /** URL of the in-pod Selenium Grid (selenium/standalone-chromium). */
  public final String remoteUrl;

  public final int maxConcurrent;
  public final long defaultTimeoutMs;
  public final long maxTimeoutMs;
  public final long stepTimeoutMs;
  public final int maxSteps;
  public final int maxScreenshotBytes;
  public final boolean browserHeadless;

  /**
   * Arbitrary in-page script execution must be opt-in: this service sits behind the gateway and a
   * stolen auth header should not become a remote-code-execution primitive.
   */
  public final boolean allowEvaluate;

  private Config(
      final String httpHost,
      final int httpPort,
      final String serverAuthSecret,
      final boolean allowUnauthenticated,
      final String remoteUrl,
      final int maxConcurrent,
      final long defaultTimeoutMs,
      final long maxTimeoutMs,
      final long stepTimeoutMs,
      final int maxSteps,
      final int maxScreenshotBytes,
      final boolean browserHeadless,
      final boolean allowEvaluate) {
    this.httpHost = httpHost;
    this.httpPort = httpPort;
    this.serverAuthSecret = serverAuthSecret;
    this.allowUnauthenticated = allowUnauthenticated;
    this.remoteUrl = remoteUrl;
    this.maxConcurrent = maxConcurrent;
    this.defaultTimeoutMs = defaultTimeoutMs;
    this.maxTimeoutMs = maxTimeoutMs;
    this.stepTimeoutMs = stepTimeoutMs;
    this.maxSteps = maxSteps;
    this.maxScreenshotBytes = maxScreenshotBytes;
    this.browserHeadless = browserHeadless;
    this.allowEvaluate = allowEvaluate;
  }

  public static Config fromEnv() {
    final String secret = trimToNull(System.getenv("SERVER_AUTH_SECRET"));
    final boolean allowUnauthenticated = readBool("SELENIUM_ALLOW_UNAUTHENTICATED", false);
    if (secret == null && !allowUnauthenticated) {
      // Fail closed in the same spirit as dd-web-scraper / dd-browser-test-server: without a
      // secret every POST /run is rejected (see Auth), so surface the misconfiguration loudly.
      log.warn(
          "SERVER_AUTH_SECRET is unset and SELENIUM_ALLOW_UNAUTHENTICATED is false; "
              + "POST /run will reject every request until a secret is provided");
    }

    return new Config(
        System.getenv().getOrDefault("HTTP_HOST", "0.0.0.0"),
        readInt("HTTP_PORT", 8105),
        secret,
        allowUnauthenticated,
        System.getenv().getOrDefault("SELENIUM_REMOTE_URL", "http://localhost:4444"),
        readInt("SELENIUM_MAX_CONCURRENT", 2),
        readLong("SELENIUM_DEFAULT_TIMEOUT_MS", 30_000L),
        readLong("SELENIUM_MAX_TIMEOUT_MS", 180_000L),
        readLong("SELENIUM_STEP_TIMEOUT_MS", 15_000L),
        readInt("SELENIUM_MAX_STEPS", 64),
        readInt("SELENIUM_MAX_SCREENSHOT_BYTES", 1_500_000),
        readBool("SELENIUM_BROWSER_HEADLESS", true),
        readBool("SELENIUM_ALLOW_EVALUATE", false));
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

  private static long readLong(final String name, final long fallback) {
    final String raw = trimToNull(System.getenv(name));
    if (raw == null) {
      return fallback;
    }
    try {
      return Long.parseLong(raw);
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
