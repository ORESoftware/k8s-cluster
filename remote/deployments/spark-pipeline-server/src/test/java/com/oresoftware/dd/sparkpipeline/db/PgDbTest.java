package com.oresoftware.dd.sparkpipeline.db;

import org.junit.jupiter.api.Test;

import java.net.URISyntaxException;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNull;

class PgDbTest {

  @Test
  void passesThroughJdbcUrls() throws URISyntaxException {
    final var p = PgDb.ParsedJdbcUrl.from(
        "jdbc:postgresql://rds.example:5432/dd_pg_defs?ssl=true", null, null);
    assertEquals("jdbc:postgresql://rds.example:5432/dd_pg_defs?ssl=true", p.jdbcUrl());
    assertNull(p.user());
    assertNull(p.password());
  }

  @Test
  void rewritesPostgresUriIntoJdbc() throws URISyntaxException {
    final var p = PgDb.ParsedJdbcUrl.from(
        "postgres://alice:s3cret@rds.example:5432/dd_pg_defs?ssl=true", null, null);
    assertEquals("jdbc:postgresql://rds.example:5432/dd_pg_defs?ssl=true", p.jdbcUrl());
    assertEquals("alice", p.user());
    assertEquals("s3cret", p.password());
  }

  @Test
  void rewritesPostgresqlUriIntoJdbc() throws URISyntaxException {
    final var p = PgDb.ParsedJdbcUrl.from(
        "postgresql://bob@db.local/dd_pg_defs", null, null);
    assertEquals("jdbc:postgresql://db.local/dd_pg_defs", p.jdbcUrl());
    assertEquals("bob", p.user());
    assertNull(p.password());
  }

  @Test
  void envCredentialsTakePrecedenceWhenUriHasNone() throws URISyntaxException {
    final var p = PgDb.ParsedJdbcUrl.from(
        "postgresql://db.local/dd_pg_defs", "env-user", "env-pw");
    assertEquals("env-user", p.user());
    assertEquals("env-pw", p.password());
  }

  @Test
  void uriCredentialsTakePrecedenceOverEnv() throws URISyntaxException {
    // If both are set, the one in the URI wins — matches what HikariCP would do if we passed
    // the URL through verbatim, and matches the convention of the other dd-* services.
    final var p = PgDb.ParsedJdbcUrl.from(
        "postgresql://uri-user:uri-pw@db.local/dd_pg_defs", "env-user", "env-pw");
    assertEquals("uri-user", p.user());
    assertEquals("uri-pw", p.password());
  }
}
