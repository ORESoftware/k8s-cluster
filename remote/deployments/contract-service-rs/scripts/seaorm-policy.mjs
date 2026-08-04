const CONTRACT_KEYS = new Set([
  "$schema",
  "version",
  "policyIssue",
  "schemaAuthority",
  "migration",
  "rust",
  "consumer",
  "verification",
]);

export class SeaOrmPolicyError extends Error {
  constructor(errors) {
    super(`contract-service SeaORM policy failed:\n- ${errors.join("\n- ")}`);
    this.name = "SeaOrmPolicyError";
    this.errors = errors;
  }
}

function require(condition, message, errors) {
  if (!condition) errors.push(message);
}

function escapeRegularExpression(value) {
  // Escape only syntax characters that are special outside a character class.
  // Escaping `-` as `\-` is an invalid identity escape in Unicode-mode regular
  // expressions and caused every policy test involving `sea-orm` to throw a
  // SyntaxError before a SeaOrmPolicyError could be produced.
  return value.replace(/[.*+?^${}()|[\]\\]/gu, "\\$&");
}

function dependency(manifest, name) {
  const escapedName = escapeRegularExpression(name);
  const pattern = new RegExp(`^\\s*${escapedName}\\s*=`, "mu");
  return pattern.test(manifest);
}

export function validateSeaOrmPolicy({
  manifest,
  coordination,
  main,
  sharedContract,
  sharedCommit,
}) {
  const errors = [];

  require(typeof manifest === "string", "Cargo.toml must be text", errors);
  require(typeof coordination === "string", "coordination.rs must be text", errors);
  require(typeof main === "string", "main.rs must be text", errors);
  require(
    sharedContract !== null && typeof sharedContract === "object" && !Array.isArray(sharedContract),
    "shared Rust database contract must be an object",
    errors,
  );
  require(/^[0-9a-f]{40}$/u.test(sharedCommit ?? ""), "shared gitlink must be an immutable commit SHA", errors);

  if (typeof manifest === "string") {
    require(dependency(manifest, "sea-orm"), "Cargo.toml must depend on SeaORM", errors);
    require(
      dependency(manifest, "dd-pg-defs-sea-orm"),
      "Cargo.toml must consume the generated shared SeaORM adapter",
      errors,
    );
    require(!dependency(manifest, "sqlx"), "Cargo.toml must not directly depend on SQLx", errors);
    require(
      !dependency(manifest, "tokio-postgres"),
      "Cargo.toml must not directly depend on tokio-postgres",
      errors,
    );
    require(
      manifest.includes('path = "../../libs/pg-defs/generated/rust/sea-orm"'),
      "generated SeaORM adapter path must stay under remote/libs/pg-defs",
      errors,
    );
  }

  if (typeof coordination === "string") {
    for (const forbidden of [
      /\buse\s+sqlx\b/u,
      /\bsqlx::(?:query|query_as|query_scalar|migrate!)/u,
      /\bPgPool(?:Options)?\b/u,
      /\btokio_postgres\b/u,
      /\bMigrator::(?:up|down)\b/u,
    ]) {
      require(!forbidden.test(coordination), `coordination.rs contains forbidden persistence path ${forbidden}`, errors);
    }
    for (const required of [
      "DatabaseConnection",
      "DatabaseTransaction",
      "ConnectOptions",
      "OnceCell<DatabaseConnection>",
      "Statement::from_sql_and_values",
      "select pg_try_advisory_xact_lock($1) as acquired",
      "[advisory_key.into()]",
      ".begin().await",
      ".rollback().await",
      ".sqlx_logging(false)",
    ]) {
      require(coordination.includes(required), `coordination.rs is missing ${JSON.stringify(required)}`, errors);
    }
    require(
      coordination.includes("transaction: Option<DatabaseTransaction>"),
      "advisory lock lifetime must remain bound to the owned SeaORM transaction",
      errors,
    );
    require(
      coordination.includes("database: OnceCell<DatabaseConnection>"),
      "optional coordination must preserve lazy database initialization",
      errors,
    );
  }

  if (typeof main === "string") {
    require(
      main.includes("coordination::CoordinationState::from_env(rpc_client.clone())"),
      "main must initialize the coordination state through its reviewed boundary",
      errors,
    );
    require(
      !/sqlx::|PgPool|migrate!/u.test(main),
      "main must not own a SQLx pool or migration path",
      errors,
    );
  }

  if (sharedContract !== null && typeof sharedContract === "object") {
    for (const key of CONTRACT_KEYS) {
      require(Object.hasOwn(sharedContract, key), `shared contract is missing ${key}`, errors);
    }
    require(
      sharedContract.schemaAuthority?.repository === "ORESoftware/k8s-libs-and-shared-defs",
      "shared schema authority repository drifted",
      errors,
    );
    require(
      sharedContract.schemaAuthority?.path === "pg-defs/schema/schema.sql",
      "shared schema authority path drifted",
      errors,
    );
    require(sharedContract.rust?.applicationOrm === "SeaORM", "shared contract must require SeaORM", errors);
    require(
      sharedContract.rust?.directSqlxDependency === "forbidden",
      "shared contract must forbid direct SQLx",
      errors,
    );
    require(
      sharedContract.schemaAuthority?.serviceBootMigrations === false,
      "shared contract must forbid service boot migrations",
      errors,
    );
    require(sharedContract.migration?.tool === "dpm", "shared migration tool must remain dpm", errors);
    require(
      sharedContract.migration?.repository ===
        "declarative-migrations/declarative-postgres-migrate.rs",
      "shared migration repository drifted",
      errors,
    );
  }

  if (errors.length > 0) throw new SeaOrmPolicyError(errors);
  return {
    valid: true,
    service: "dd-contract-service",
    applicationOrm: "SeaORM",
    sharedCommit,
    directSqlx: false,
    bootMigrations: false,
    advisoryLockStatement: true,
  };
}
