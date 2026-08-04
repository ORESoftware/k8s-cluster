const ROOT_KEYS = new Set([
  "$schema",
  "version",
  "policyIssue",
  "schemaAuthority",
  "migration",
  "rust",
  "consumer",
  "verification",
]);
const SCHEMA_KEYS = new Set([
  "repository",
  "path",
  "generatedAdaptersAreMigrationSources",
  "serviceBootMigrations",
  "humanReviewRequired",
]);
const MIGRATION_KEYS = new Set([
  "tool",
  "repository",
  "minimumVersion",
  "installerCommit",
  "script",
  "defaultCommand",
  "allowedCommands",
  "applyRequiresHumanReview",
  "destructiveConsents",
]);
const RUST_KEYS = new Set([
  "applicationOrm",
  "directSqlxDependency",
  "rawTokioPostgresDependency",
  "connectionApi",
  "statementApi",
  "generatedCrate",
  "sanctionedStatementCases",
  "listenerEscape",
]);
const GENERATED_KEYS = new Set(["package", "manifest", "source"]);
const CONSUMER_KEYS = new Set([
  "distribution",
  "repository",
  "mount",
  "cargoPath",
  "schemaPath",
  "pinMustBeImmutable",
]);

const REQUIRED_COMMANDS = ["diff", "verify", "review", "apply", "bootstrap"];
const REQUIRED_CONSENTS = ["--allow-destructive-sql", "--allow-destructive-ops"];
const REQUIRED_STATEMENT_CASES = [
  "advisory-locks",
  "data-modifying-ctes",
  "excluded-expression-upserts",
  "for-update-skip-locked",
  "postgres-casts-and-operators",
  "server-clock-writes",
];
const REQUIRED_VERIFICATION = [
  "baseline-build-before-migration",
  "cargo-check-all-targets",
  "cargo-test-all-targets",
  "cargo-clippy-warnings-denied",
  "direct-sqlx-grep-clean",
  "no-boot-time-migration-path",
  "dpm-verify-converges",
];
const SHA40 = /^[0-9a-f]{40}$/u;

export class RustServerContractError extends Error {
  constructor(errors) {
    super(`Rust server database contract is invalid:\n- ${errors.join("\n- ")}`);
    this.name = "RustServerContractError";
    this.errors = errors;
  }
}

function isObject(value) {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

function exactKeys(value, expected, location, errors) {
  if (!isObject(value)) {
    errors.push(`${location}: must be an object`);
    return;
  }
  for (const key of Object.keys(value)) {
    if (!expected.has(key)) errors.push(`${location}.${key}: unknown property`);
  }
  for (const key of expected) {
    if (!Object.hasOwn(value, key)) errors.push(`${location}.${key}: missing property`);
  }
}

function exactSet(actual, expected, location, errors) {
  if (!Array.isArray(actual)) {
    errors.push(`${location}: must be an array`);
    return;
  }
  if (actual.length !== expected.length || new Set(actual).size !== actual.length) {
    errors.push(`${location}: must contain exactly ${expected.join(", ")}`);
    return;
  }
  for (const value of expected) {
    if (!actual.includes(value)) errors.push(`${location}: missing ${value}`);
  }
}

function requireEqual(actual, expected, location, errors) {
  if (actual !== expected) {
    errors.push(`${location}: must equal ${JSON.stringify(expected)}`);
  }
}

function safeRelative(value) {
  return (
    typeof value === "string" &&
    value.length > 0 &&
    value.length <= 300 &&
    !value.startsWith("/") &&
    !value.split("/").includes("..") &&
    !/[\u0000-\u001f\u007f]/u.test(value)
  );
}

function parsePackageName(manifest) {
  const match = /^name\s*=\s*"([^"]+)"\s*$/mu.exec(manifest);
  return match?.[1] ?? null;
}

export function validateRustServerContract(contractInput, filesInput) {
  const contract = structuredClone(contractInput);
  const files = structuredClone(filesInput);
  const errors = [];

  exactKeys(contract, ROOT_KEYS, "contract", errors);
  if (!isObject(contract)) throw new RustServerContractError(errors);
  requireEqual(contract.$schema, "./rust-server-contract.schema.json", "contract.$schema", errors);
  requireEqual(contract.version, 1, "contract.version", errors);
  requireEqual(contract.policyIssue, "fleet-sqlx-to-seaorm", "contract.policyIssue", errors);

  exactKeys(contract.schemaAuthority, SCHEMA_KEYS, "contract.schemaAuthority", errors);
  if (isObject(contract.schemaAuthority)) {
    requireEqual(
      contract.schemaAuthority.repository,
      "ORESoftware/k8s-libs-and-shared-defs",
      "contract.schemaAuthority.repository",
      errors,
    );
    requireEqual(
      contract.schemaAuthority.path,
      "pg-defs/schema/schema.sql",
      "contract.schemaAuthority.path",
      errors,
    );
    requireEqual(
      contract.schemaAuthority.generatedAdaptersAreMigrationSources,
      false,
      "contract.schemaAuthority.generatedAdaptersAreMigrationSources",
      errors,
    );
    requireEqual(
      contract.schemaAuthority.serviceBootMigrations,
      false,
      "contract.schemaAuthority.serviceBootMigrations",
      errors,
    );
    requireEqual(
      contract.schemaAuthority.humanReviewRequired,
      true,
      "contract.schemaAuthority.humanReviewRequired",
      errors,
    );
  }

  exactKeys(contract.migration, MIGRATION_KEYS, "contract.migration", errors);
  if (isObject(contract.migration)) {
    requireEqual(contract.migration.tool, "dpm", "contract.migration.tool", errors);
    requireEqual(
      contract.migration.repository,
      "declarative-migrations/declarative-postgres-migrate.rs",
      "contract.migration.repository",
      errors,
    );
    requireEqual(
      contract.migration.minimumVersion,
      "0.3.2",
      "contract.migration.minimumVersion",
      errors,
    );
    if (!SHA40.test(contract.migration.installerCommit ?? "")) {
      errors.push("contract.migration.installerCommit: must be a lowercase 40-character commit SHA");
    }
    requireEqual(
      contract.migration.script,
      "pg-defs/scripts/dpm.sh",
      "contract.migration.script",
      errors,
    );
    requireEqual(contract.migration.defaultCommand, "diff", "contract.migration.defaultCommand", errors);
    requireEqual(
      contract.migration.applyRequiresHumanReview,
      true,
      "contract.migration.applyRequiresHumanReview",
      errors,
    );
    exactSet(contract.migration.allowedCommands, REQUIRED_COMMANDS, "contract.migration.allowedCommands", errors);
    exactSet(
      contract.migration.destructiveConsents,
      REQUIRED_CONSENTS,
      "contract.migration.destructiveConsents",
      errors,
    );
  }

  exactKeys(contract.rust, RUST_KEYS, "contract.rust", errors);
  if (isObject(contract.rust)) {
    requireEqual(contract.rust.applicationOrm, "SeaORM", "contract.rust.applicationOrm", errors);
    requireEqual(
      contract.rust.directSqlxDependency,
      "forbidden",
      "contract.rust.directSqlxDependency",
      errors,
    );
    requireEqual(
      contract.rust.rawTokioPostgresDependency,
      "forbidden",
      "contract.rust.rawTokioPostgresDependency",
      errors,
    );
    requireEqual(
      contract.rust.connectionApi,
      "sea_orm::Database::connect",
      "contract.rust.connectionApi",
      errors,
    );
    requireEqual(
      contract.rust.statementApi,
      "sea_orm::Statement",
      "contract.rust.statementApi",
      errors,
    );
    requireEqual(
      contract.rust.listenerEscape,
      "sea_orm::sqlx::postgres::PgListener",
      "contract.rust.listenerEscape",
      errors,
    );
    exactSet(
      contract.rust.sanctionedStatementCases,
      REQUIRED_STATEMENT_CASES,
      "contract.rust.sanctionedStatementCases",
      errors,
    );
    exactKeys(contract.rust.generatedCrate, GENERATED_KEYS, "contract.rust.generatedCrate", errors);
    if (isObject(contract.rust.generatedCrate)) {
      requireEqual(
        contract.rust.generatedCrate.package,
        "dd-pg-defs-sea-orm",
        "contract.rust.generatedCrate.package",
        errors,
      );
      requireEqual(
        contract.rust.generatedCrate.manifest,
        "pg-defs/generated/rust/sea-orm/Cargo.toml",
        "contract.rust.generatedCrate.manifest",
        errors,
      );
      requireEqual(
        contract.rust.generatedCrate.source,
        "pg-defs/generated/rust/sea-orm/src/lib.rs",
        "contract.rust.generatedCrate.source",
        errors,
      );
    }
  }

  exactKeys(contract.consumer, CONSUMER_KEYS, "contract.consumer", errors);
  if (isObject(contract.consumer)) {
    requireEqual(contract.consumer.distribution, "git-submodule", "contract.consumer.distribution", errors);
    requireEqual(
      contract.consumer.repository,
      "ORESoftware/k8s-libs-and-shared-defs",
      "contract.consumer.repository",
      errors,
    );
    for (const key of ["mount", "cargoPath", "schemaPath"]) {
      if (!safeRelative(contract.consumer[key])) {
        errors.push(`contract.consumer.${key}: must be a bounded repository-relative path`);
      }
    }
    requireEqual(contract.consumer.pinMustBeImmutable, true, "contract.consumer.pinMustBeImmutable", errors);
  }
  exactSet(contract.verification, REQUIRED_VERIFICATION, "contract.verification", errors);

  if (!isObject(files)) {
    errors.push("files: must be an object containing the authoritative repository files");
  } else {
    const schema = files.schemaSql;
    if (typeof schema !== "string" || !/create\s+(?:table|schema)/iu.test(schema)) {
      errors.push("files.schemaSql: must contain the canonical PostgreSQL schema");
    }

    const generatedManifest = files.generatedManifest;
    if (typeof generatedManifest !== "string") {
      errors.push("files.generatedManifest: must contain the generated SeaORM Cargo manifest");
    } else if (parsePackageName(generatedManifest) !== contract.rust?.generatedCrate?.package) {
      errors.push("files.generatedManifest: package name does not match the contract");
    }

    const generatedSource = files.generatedSource;
    if (typeof generatedSource !== "string") {
      errors.push("files.generatedSource: must contain the generated SeaORM source");
    } else {
      for (const marker of [
        "Generated by @dd/pg-defs. Do not edit by hand.",
        "SOURCE OF TRUTH: schema/schema.sql defines the database contract.",
        "use sea_orm::entity::prelude::*;",
        "DeriveEntityModel",
      ]) {
        if (!generatedSource.includes(marker)) {
          errors.push(`files.generatedSource: missing generated adapter marker ${JSON.stringify(marker)}`);
        }
      }
      if (/sqlx::(?:query|query_as|migrate!)/u.test(generatedSource)) {
        errors.push("files.generatedSource: generated SeaORM adapter must not execute direct SQLx queries or migrations");
      }
    }

    const dpmScript = files.dpmScript;
    if (typeof dpmScript !== "string") {
      errors.push("files.dpmScript: must contain the canonical DPM wrapper");
    } else {
      for (const marker of [
        'cmd="${1:-diff}"',
        'schema_sql="$pg_defs_dir/schema/schema.sql"',
        "SHADOW_DATABASE_URL is required",
        "Never apply migrations automatically; a human reviews the SQL first.",
        "--allow-destructive-sql / --allow-destructive-ops",
      ]) {
        if (!dpmScript.includes(marker)) {
          errors.push(`files.dpmScript: missing safety marker ${JSON.stringify(marker)}`);
        }
      }
      if (!dpmScript.includes(contract.migration?.installerCommit ?? "<missing>")) {
        errors.push("files.dpmScript: installer pin does not match the contract");
      }
      if (/sqlx\s+migrate|sea-orm-cli\s+migrate/iu.test(dpmScript)) {
        errors.push("files.dpmScript: ORM-owned migration execution is prohibited");
      }
    }
  }

  if (errors.length > 0) throw new RustServerContractError(errors);
  return {
    valid: true,
    schemaAuthority: contract.schemaAuthority.repository,
    schemaPath: contract.schemaAuthority.path,
    applicationOrm: contract.rust.applicationOrm,
    migrationTool: contract.migration.tool,
    minimumDpmVersion: contract.migration.minimumVersion,
    generatedCrate: contract.rust.generatedCrate.package,
    sanctionedStatementCases: contract.rust.sanctionedStatementCases.length,
    verificationGates: contract.verification.length,
  };
}
