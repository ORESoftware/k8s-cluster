package com.oresoftware.dd.sparkpipeline.db;

import com.zaxxer.hikari.HikariConfig;
import com.zaxxer.hikari.HikariDataSource;
import org.jooq.DSLContext;
import org.jooq.SQLDialect;
import org.jooq.impl.DSL;
import org.slf4j.Logger;
import org.slf4j.LoggerFactory;

import java.net.URI;
import java.net.URISyntaxException;
import java.util.Objects;
import java.util.Optional;

/**
 * HikariCP-backed Postgres connection pool wrapped around a jOOQ {@link DSLContext}.
 *
 * <p>This is the single point in the service where we open database connections. Handlers and
 * the {@link com.oresoftware.dd.sparkpipeline.pipeline.JobService} ask {@link #context()} for the
 * jOOQ {@code DSLContext} and use the table/column constants generated into
 * {@code dd.pgdefs.jooq.Tables} by {@code remote/libs/pg-defs} for type-safe queries.
 *
 * <p>The service is intentionally tolerant of an unconfigured database: if the
 * {@code RDS_DATABASE_URL} environment variable is unset or blank, {@link #isAvailable()}
 * returns {@code false} and {@link #context()} returns {@link Optional#empty()}. The HTTP
 * surface then returns {@code 503} from any DB-dependent endpoint rather than crash-looping
 * the whole pod. This matches the pattern used by other dd-* services that treat their
 * Postgres dependency as optional during local development.
 */
public final class PgDb implements AutoCloseable {

  private static final Logger log = LoggerFactory.getLogger(PgDb.class);

  private static final String ENV_URL = "RDS_DATABASE_URL";
  private static final String ENV_USER = "RDS_DATABASE_USER";
  private static final String ENV_PASSWORD = "RDS_DATABASE_PASSWORD";
  private static final String ENV_POOL_SIZE = "PG_POOL_SIZE";

  private final HikariDataSource dataSource;
  private final DSLContext dsl;

  private PgDb(final HikariDataSource dataSource, final DSLContext dsl) {
    this.dataSource = dataSource;
    this.dsl = dsl;
  }

  /**
   * Build a {@link PgDb} from environment variables. Returns {@link Optional#empty()} if
   * {@code RDS_DATABASE_URL} is unset/blank — used by callers that treat the database as an
   * optional dependency in development.
   */
  public static Optional<PgDb> fromEnv() {
    final String url = trimOrNull(System.getenv(ENV_URL));
    if (url == null) {
      log.info("PgDb: {} unset, running without Postgres", ENV_URL);
      return Optional.empty();
    }

    final ParsedJdbcUrl parsed;
    try {
      parsed = ParsedJdbcUrl.from(
          url,
          trimOrNull(System.getenv(ENV_USER)),
          trimOrNull(System.getenv(ENV_PASSWORD)));
    } catch (URISyntaxException use) {
      log.error("PgDb: cannot parse {} as a Postgres URL: {}", ENV_URL, use.getMessage());
      return Optional.empty();
    }

    final HikariConfig cfg = new HikariConfig();
    cfg.setJdbcUrl(parsed.jdbcUrl());
    if (parsed.user() != null) {
      cfg.setUsername(parsed.user());
    }
    if (parsed.password() != null) {
      cfg.setPassword(parsed.password());
    }
    cfg.setPoolName("dd-spark-pipeline-server-pg");
    cfg.setMaximumPoolSize(parsePoolSize(System.getenv(ENV_POOL_SIZE), 8));
    cfg.setMinimumIdle(1);
    cfg.setConnectionTimeout(10_000);
    cfg.setIdleTimeout(60_000);
    cfg.setMaxLifetime(30 * 60_000L);
    cfg.addDataSourceProperty("ApplicationName", "dd-spark-pipeline-server");

    try {
      final HikariDataSource ds = new HikariDataSource(cfg);
      final DSLContext dsl = DSL.using(ds, SQLDialect.POSTGRES);
      log.info("PgDb: connected pool={} maxSize={}", cfg.getPoolName(), cfg.getMaximumPoolSize());
      return Optional.of(new PgDb(ds, dsl));
    } catch (RuntimeException e) {
      log.error("PgDb: failed to initialise HikariCP pool", e);
      return Optional.empty();
    }
  }

  public boolean isAvailable() {
    return !dataSource.isClosed();
  }

  /**
   * @return the jOOQ {@link DSLContext} if the pool is open. Wrapped in {@link Optional} so
   *     callers can degrade gracefully when {@link #fromEnv()} produced no instance and the
   *     handler still wants to short-circuit cleanly.
   */
  public Optional<DSLContext> context() {
    if (!isAvailable()) {
      return Optional.empty();
    }
    return Optional.of(dsl);
  }

  @Override
  public void close() {
    if (!dataSource.isClosed()) {
      log.info("PgDb: closing connection pool");
      dataSource.close();
    }
  }

  // --- helpers ---

  private static String trimOrNull(final String s) {
    if (s == null) return null;
    final String t = s.trim();
    return t.isEmpty() ? null : t;
  }

  private static int parsePoolSize(final String raw, final int fallback) {
    if (raw == null || raw.isBlank()) return fallback;
    try {
      final int v = Integer.parseInt(raw.trim());
      return v > 0 ? v : fallback;
    } catch (NumberFormatException nfe) {
      return fallback;
    }
  }

  /**
   * Accept either:
   *
   * <ul>
   *   <li>a JDBC URL ({@code jdbc:postgresql://host:5432/db?user=...&password=...}) — passed
   *       through unchanged;</li>
   *   <li>a libpq / postgres URI ({@code postgres://user:pass@host:5432/db}) — rewritten to
   *       JDBC form with user/password lifted into HikariConfig.</li>
   * </ul>
   *
   * <p>This matches the convention used by the Gleam / Erlang / Node services in this repo,
   * which all consume the same {@code RDS_DATABASE_URL} secret but in libpq form.
   */
  /** Visible-for-tests. */
  public record ParsedJdbcUrl(String jdbcUrl, String user, String password) {

    public static ParsedJdbcUrl from(final String raw, final String envUser, final String envPassword)
        throws URISyntaxException {
      Objects.requireNonNull(raw, "raw");

      if (raw.startsWith("jdbc:")) {
        return new ParsedJdbcUrl(raw, envUser, envPassword);
      }

      // Tolerate the two libpq spellings.
      final String normalised;
      if (raw.startsWith("postgres://")) {
        normalised = "postgresql://" + raw.substring("postgres://".length());
      } else if (raw.startsWith("postgresql://")) {
        normalised = raw;
      } else {
        // Treat as bare "host:port/db" — rare in practice.
        normalised = "postgresql://" + raw;
      }

      final URI uri = new URI(normalised);

      final StringBuilder jdbc = new StringBuilder("jdbc:postgresql://");
      if (uri.getHost() != null) {
        jdbc.append(uri.getHost());
      }
      if (uri.getPort() > 0) {
        jdbc.append(':').append(uri.getPort());
      }
      if (uri.getPath() != null && !uri.getPath().isEmpty()) {
        jdbc.append(uri.getPath());
      }
      if (uri.getQuery() != null && !uri.getQuery().isEmpty()) {
        jdbc.append('?').append(uri.getQuery());
      }

      // Credentials precedence: anything embedded in the URI's userinfo wins over the env
      // RDS_DATABASE_USER / RDS_DATABASE_PASSWORD vars. This matches what HikariCP would do if
      // we passed the URL through verbatim, and matches the convention of the other dd-*
      // services that all read the same RDS_DATABASE_URL secret.
      String user = envUser;
      String password = envPassword;
      final String userInfo = uri.getUserInfo();
      if (userInfo != null && !userInfo.isEmpty()) {
        final int colon = userInfo.indexOf(':');
        if (colon >= 0) {
          user = userInfo.substring(0, colon);
          password = userInfo.substring(colon + 1);
        } else {
          user = userInfo;
        }
      }

      return new ParsedJdbcUrl(jdbc.toString(), user, password);
    }
  }
}
