# Generated JVM adapters

Two flavors live here:

- `jooq/` — A jOOQ `Tables.java` that can be referenced from any JVM stack (plain Java, Spring
  Boot, Vert.x, Micronaut, Scala via Java interop, Kotlin, etc.). The build script wires up the
  jOOQ runtime dependency so you can run-time `DSL.using(...)` immediately, and serves as a
  starting point for full `jooq-codegen` if you want everything — column-level constants live in
  the generated `Tables.java` already.
- `hibernate/` — One JPA-annotated entity class per canonical table. Drop these into a Spring Boot
  `@Repository`, a Vert.x Hibernate Reactive verticle, or any plain JPA app. Constraints that JPA
  cannot natively express (partial indexes, GIN indexes, JSONB CHECKs) are intentionally left to the
  database; this package never owns migrations.

Both directories ship a Gradle build file. Translating to Maven or sbt is mechanical: declare the
same jOOQ / Hibernate / Jakarta Persistence dependencies and point your build at
`src/main/java`.
