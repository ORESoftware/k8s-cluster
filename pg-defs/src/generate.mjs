// IMPORTANT FOR CODING AGENTS:
// - schema/schema.sql is the final source of truth for database shape.
// - Generated ORM/client files are adapters only; never treat them as migration authority.
// - Never run or apply migrations automatically. Use report-only RDS drift checks for migration
//   planning; do not generate .sql migration files from adapters.
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

import { loadSqlContract } from './sql-contract.mjs';

const packageRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');

async function main() {
  const { contract: schema, sourceSql } = await loadSqlContract(packageRoot);
  const args = new Set(process.argv.slice(2));

  if (args.has('--print-sql')) {
    process.stdout.write(sourceSql.endsWith('\n') ? sourceSql : `${sourceSql}\n`);
    return;
  }

  const outputs = renderOutputs(schema, sourceSql);

  // Every output must resolve inside generated/. Table/class names are
  // parser-constrained to [A-Za-z0-9_] today, but nothing at the write site
  // enforced that — this guard makes path escape structurally impossible.
  const generatedRoot = path.join(packageRoot, 'generated') + path.sep;
  const resolveOutputPath = (relativePath) => {
    const absolutePath = path.join(packageRoot, relativePath);
    if (!absolutePath.startsWith(generatedRoot)) {
      throw new Error(`generated output path escapes generated/: ${relativePath}`);
    }
    return absolutePath;
  };

  if (args.has('--check')) {
    const stale = [];
    for (const [relativePath, contents] of outputs) {
      const absolutePath = resolveOutputPath(relativePath);
      let existing = '';
      try {
        existing = await readFile(absolutePath, 'utf8');
      } catch {
        stale.push(relativePath);
        continue;
      }
      if (existing !== contents) {
        stale.push(relativePath);
      }
    }

    if (stale.length > 0) {
      console.error(
        `pg-defs generated outputs are stale:\n${stale.map((file) => `  - ${file}`).join('\n')}`,
      );
      process.exitCode = 1;
      return;
    }

    console.log('pg-defs generated outputs are up to date.');
    return;
  }

  for (const [relativePath, contents] of outputs) {
    const absolutePath = resolveOutputPath(relativePath);
    await mkdir(path.dirname(absolutePath), { recursive: true });
    await writeFile(absolutePath, contents);
  }

  console.log(`Generated ${outputs.size} pg-defs adapter files.`);
}

function renderOutputs(schema, sourceSql) {
  const outputs = new Map();
  const add = (relativePath, contents) => {
    if (outputs.has(relativePath)) {
      throw new Error(`Duplicate generated output path: ${relativePath}`);
    }
    outputs.set(relativePath, contents);
  };

  add('generated/typescript/index.ts', renderTypeScriptIndex());
  add('generated/typescript/drizzle.ts', renderDrizzleTypeScript(schema));
  add('generated/typescript/typeorm.ts', renderTypeOrmTypeScript(schema));
  add('generated/prisma/schema.prisma', renderPrisma(schema));
  add('generated/python/sqlalchemy_models.py', renderPythonSqlAlchemy(schema));
  add('generated/python-django/models.py', renderDjangoModels(schema));
  add('generated/python-django/__init__.py', renderDjangoInit());
  add('generated/python-django/apps.py', renderDjangoApps());
  add('generated/ruby-activerecord/lib/dd_pg_defs.rb', renderActiveRecord(schema));
  add('generated/ruby-activerecord/dd_pg_defs.gemspec', renderActiveRecordGemspec());
  add('generated/php-eloquent/src/DdPgDefs.php', renderEloquent(schema));
  add('generated/php-eloquent/composer.json', renderEloquentComposerJson());
  add('generated/csharp-efcore/DdPgDefs.cs', renderEfCore(schema));
  add('generated/csharp-efcore/DdPgDefs.csproj', renderEfCoreCsproj());
  add('generated/kotlin-exposed/src/main/kotlin/dd/pgdefs/PgDefs.kt', renderExposed(schema));
  add('generated/kotlin-exposed/build.gradle.kts', renderExposedBuildGradle());
  add('generated/haskell/src/DdPgDefs.hs', renderHaskell(schema));
  add('generated/haskell/dd-pg-defs.cabal', renderHaskellCabal());
  add('generated/ocaml/lib/dd_pg_defs.ml', renderOcaml(schema));
  add('generated/ocaml/lib/dune', renderOcamlDune());
  add('generated/ocaml/dune-project', renderOcamlDuneProject());
  add('generated/fsharp/DdPgDefs.fs', renderFSharp(schema));
  add('generated/fsharp/DdPgDefs.fsproj', renderFSharpProject());
  add('generated/cpp/dd_pg_defs.hpp', renderCpp(schema));
  add('generated/cpp/CMakeLists.txt', renderCppCMake());
  add('generated/zig/dd_pg_defs.zig', renderZig(schema));
  add('generated/zig/build.zig', renderZigBuild());
  add('generated/typescript/sequelize.ts', renderSequelize(schema));
  add('generated/python-peewee/models.py', renderPeewee(schema));
  add('generated/python-peewee/__init__.py', renderPeeweeInit());
  for (const [path, contents] of renderDoctrineEntityFiles(schema)) {
    add(path, contents);
  }
  add('generated/php-doctrine/composer.json', renderDoctrineComposerJson());
  add('generated/go/gorm/go.mod', renderGoGormMod());
  add('generated/go/gorm/pg_defs.go', renderGoGorm(schema));
  add('generated/go/bun/go.mod', renderGoBunMod());
  add('generated/go/bun/pg_defs.go', renderGoBun(schema));
  add('generated/go/ent/go.mod', renderGoEntMod());
  add('generated/go/ent/schema/doc.go', renderEntDocGo());
  for (const [path, contents] of renderEntSchemaFiles(schema)) {
    add(path, contents);
  }
  add('generated/go/sqlc/sqlc.yaml', renderSqlcYaml());
  add('generated/go/sqlc/schema.sql', renderSqlcSchemaSql(sourceSql));
  add('generated/go/sqlc/query.sql', renderSqlcQuerySql(schema));
  add('generated/go/sqlc/readme.md', renderSqlcReadme());
  add('generated/dart/pubspec.yaml', renderDartPubspec());
  add('generated/dart/lib/pg_defs.dart', renderDart(schema));
  add('generated/dart-drift/pubspec.yaml', renderDriftPubspec());
  add('generated/dart-drift/lib/pg_defs_drift.dart', renderDriftDart(schema));
  add('generated/dart-objectbox/pubspec.yaml', renderObjectBoxPubspec());
  add('generated/dart-objectbox/lib/pg_defs_objectbox.dart', renderObjectBoxDart(schema));
  add('generated/rust/Cargo.toml', renderRustCargo());
  add('generated/rust/src/lib.rs', renderRust(schema));
  add('generated/rust/diesel/Cargo.toml', renderDieselCargo());
  add('generated/rust/diesel/src/lib.rs', renderDieselRust(schema));
  add('generated/rust/sea-orm/Cargo.toml', renderSeaOrmCargo());
  add('generated/rust/sea-orm/src/lib.rs', renderSeaOrmRust(schema));
  add('generated/gleam/gleam.toml', renderGleamToml());
  add('generated/gleam/src/pg_defs.gleam', renderGleam(schema));
  add('generated/erlang/src/pg_defs.erl', renderErlang(schema));
  add('generated/erlang/src/pg_defs_mnesia.erl', renderMnesiaErlang(schema));
  add('generated/elixir/mix.exs', renderEctoMixExs());
  add('generated/elixir/lib/dd_pg_defs.ex', renderEctoIndex(schema));
  for (const [path, contents] of renderEctoSchemaFiles(schema)) {
    add(path, contents);
  }
  add('generated/jvm/readme.md', renderJvmReadme());
  add('generated/jvm/jooq/build.gradle', renderJooqBuildGradle());
  add('generated/jvm/jooq/src/main/java/dd/pgdefs/jooq/Tables.java', renderJooqTablesJava(schema));
  add('generated/jvm/hibernate/build.gradle', renderHibernateBuildGradle());
  add(
    'generated/jvm/hibernate/src/main/java/dd/pgdefs/hibernate/package-info.java',
    renderHibernatePackageInfoJava(),
  );
  for (const [path, contents] of renderHibernateEntityFiles(schema)) {
    add(path, contents);
  }

  return outputs;
}

function generatedNotice(prefix) {
  return [
    `${prefix} Generated by @dd/pg-defs. Do not edit by hand.`,
    `${prefix} SOURCE OF TRUTH: schema/schema.sql defines the database contract.`,
    `${prefix} Generated ORM/client code is an adapter only; do not infer migrations from it.`,
    `${prefix} MIGRATION SAFETY: never run or apply migrations automatically. Require explicit human review and approval before any database write.`,
  ];
}

function renderTypeScriptIndex() {
  return `${[
    ...generatedNotice('//'),
    '// Import explicit adapters to avoid forcing every Node service to install every ORM peer.',
    '// Available files: ./drizzle.js and ./typeorm.js. Prisma is generated at ../prisma/schema.prisma.',
  ].join('\n')}\n`;
}

function renderDrizzleTypeScript(contract) {
  // Tables that live in a non-public Postgres schema use Drizzle's `pgSchema("x").table(...)`
  // builder; `pgSchema` is only imported when such a schema is present, so a public-only
  // contract emits the exact same import line as before.
  const customSchemas = [
    ...new Set(contract.tables.map((table) => table.schema).filter((schema) => schema && schema !== 'public')),
  ];
  const pgCoreImports = ['bigint', 'bigserial', 'boolean', 'check', 'index', 'integer', 'jsonb'];
  if (customSchemas.length > 0) {
    pgCoreImports.push('pgSchema');
  }
  pgCoreImports.push('pgTable', 'smallint', 'text', 'timestamp', 'uniqueIndex', 'uuid', 'varchar');
  // Floating-point column builders, imported only when the schema actually uses them so we
  // never emit an unused import.
  const usedSqlTypes = new Set(contract.tables.flatMap((table) => table.columns.map((col) => col.sqlType)));
  if (usedSqlTypes.has('double')) {
    pgCoreImports.push('doublePrecision');
  }
  if (usedSqlTypes.has('real')) {
    pgCoreImports.push('real');
  }

  const lines = [
    ...generatedNotice('//'),
    'import { sql } from "drizzle-orm";',
    `import { ${pgCoreImports.join(', ')} } from "drizzle-orm/pg-core";`,
    'import { z } from "zod";',
    '',
    'const textEncoder = new TextEncoder();',
    'const byteLength = (value: string) => textEncoder.encode(value).length;',
    'const jsonObjectSchema = z.record(z.string(), z.unknown());',
    'const jsonArraySchema = z.array(z.unknown());',
    '',
  ];

  for (const schema of customSchemas) {
    lines.push(`export const ${camel(schema)}Schema = pgSchema(${JSON.stringify(schema)});`);
  }
  if (customSchemas.length > 0) {
    lines.push('');
  }

  for (const table of contract.tables) {
    const tableVar = table.names?.typescript ?? camel(table.name);
    const tableBuilder =
      table.schema && table.schema !== 'public' ? `${camel(table.schema)}Schema.table` : 'pgTable';
    const baseName = table.names?.rust ?? pascal(table.name);
    const enumColumns = table.columns.filter((column) => column.kind === 'enum');
    const literalColumns = table.columns.filter((column) => column.validation?.literal);

    for (const column of enumColumns) {
      const schemaName = `${camel(baseName)}${pascal(column.name)}Schema`;
      const valuesName = `${camel(baseName)}${pascal(column.name)}Values`;
      lines.push(`export const ${valuesName} = ${JSON.stringify(column.enumValues)} as const;`);
      lines.push(`export const ${schemaName} = z.enum(${valuesName});`);
      lines.push(`export type ${baseName}${pascal(column.name)} = z.infer<typeof ${schemaName}>;`);
      lines.push('');
    }

    for (const column of literalColumns) {
      const constantName = `${screaming(table.name)}_${screaming(column.name)}_DEFAULT`;
      lines.push(`export const ${constantName} = ${JSON.stringify(column.validation.literal)};`);
    }
    if (literalColumns.length > 0) {
      lines.push('');
    }

    lines.push(`export const ${tableVar} = ${tableBuilder}(`);
    lines.push(`  ${JSON.stringify(table.name)},`);
    lines.push('  {');
    for (const column of table.columns) {
      lines.push(`    ${camel(column.name)}: ${drizzleColumn(column)},`);
    }
    lines.push('  },');
    lines.push('  (table) => ({');
    for (const checkConstraint of table.checks ?? []) {
      lines.push(
        `    ${camel(checkConstraint.name)}: check(${JSON.stringify(checkConstraint.name)}, sql.raw(${JSON.stringify(checkConstraint.sql)})),`,
      );
    }
    for (const tableIndex of table.indexes ?? []) {
      lines.push(`    ${camel(tableIndex.name)}: ${drizzleIndex(tableIndex)},`);
    }
    lines.push('  }),');
    lines.push(');');
    lines.push('');

    lines.push(`export const ${camel(baseName)}RowSchema = z.object({`);
    for (const column of table.columns) {
      lines.push(`  ${camel(column.name)}: ${zodColumn(table, column, { insert: false })},`);
    }
    lines.push('});');
    lines.push('');

    lines.push(`export const ${camel(baseName)}InsertSchema = z.object({`);
    for (const column of table.columns) {
      lines.push(`  ${camel(column.name)}: ${zodColumn(table, column, { insert: true })},`);
    }
    lines.push('});');
    lines.push('');
    lines.push(
      `export const ${camel(baseName)}UpdateSchema = ${camel(baseName)}InsertSchema.partial();`,
    );
    lines.push(`export type ${baseName}Row = z.infer<typeof ${camel(baseName)}RowSchema>;`);
    lines.push(`export type ${baseName}Insert = z.infer<typeof ${camel(baseName)}InsertSchema>;`);
    lines.push(`export type ${baseName}Update = z.infer<typeof ${camel(baseName)}UpdateSchema>;`);
    lines.push('');
  }

  return `${lines.join('\n').trimEnd()}\n`;
}

function renderTypeOrmTypeScript(contract) {
  const lines = [
    ...generatedNotice('//'),
    'import { Column, Entity, Index, PrimaryColumn, PrimaryGeneratedColumn } from "typeorm";',
    '',
  ];

  for (const table of contract.tables) {
    const entityName = `${table.names?.rust ?? pascal(table.name)}Entity`;

    for (const tableIndex of table.indexes ?? []) {
      if (tableIndex.method || tableIndex.columns.some((column) => typeof column !== 'string' && column.order)) {
        lines.push(
          `// ${tableIndex.name} lives in schema.sql because TypeORM decorators cannot fully model its method/order.`,
        );
        continue;
      }
      const properties = tableIndex.columns
        .map((column) => JSON.stringify(camel(typeof column === 'string' ? column : column.name)))
        .join(', ');
      const options = [];
      if (tableIndex.unique) {
        options.push('unique: true');
      }
      if (tableIndex.where) {
        options.push(`where: ${JSON.stringify(tableIndex.where)}`);
      }
      const optionsSql = options.length > 0 ? `, { ${options.join(', ')} }` : '';
      lines.push(`@Index(${JSON.stringify(tableIndex.name)}, [${properties}]${optionsSql})`);
    }

    const entityOptions =
      table.schema && table.schema !== 'public'
        ? `{ schema: ${JSON.stringify(table.schema)}, name: ${JSON.stringify(table.name)} }`
        : `{ name: ${JSON.stringify(table.name)} }`;
    lines.push(`@Entity(${entityOptions})`);
    lines.push(`export class ${entityName} {`);
    for (const column of table.columns) {
      lines.push(`  ${typeOrmDecorator(column)}`);
      lines.push(`  ${camel(column.name)}!: ${typeScriptType(column)};`);
      lines.push('');
    }
    lines.push('}');
    lines.push('');
  }

  return `${lines.join('\n').trimEnd()}\n`;
}

function renderPrisma(contract) {
  const lines = [
    ...generatedNotice('//'),
    '',
    'generator client {',
    '  provider = "prisma-client-js"',
    '}',
    '',
    'datasource db {',
    '  provider = "postgresql"',
    '  url      = env("DATABASE_URL")',
    '}',
    '',
  ];

  for (const table of contract.tables) {
    const modelName = table.names?.rust ?? pascal(table.name);
    lines.push(`model ${modelName} {`);
    for (const column of table.columns) {
      lines.push(`  ${prismaField(column)}`);
    }

    for (const tableIndex of table.indexes ?? []) {
      if (tableIndex.where || tableIndex.method) {
        lines.push(
          `  // ${tableIndex.name} is defined in schema.sql because Prisma cannot fully model ${tableIndex.method ? `${tableIndex.method} ` : ''}partial/expression indexes.`,
        );
        continue;
      }
      const fields = tableIndex.columns
        .map((column) => camel(typeof column === 'string' ? column : column.name))
        .join(', ');
      const indexKind = tableIndex.unique ? '@@unique' : '@@index';
      lines.push(`  ${indexKind}([${fields}], map: ${JSON.stringify(tableIndex.name)})`);
    }

    if ((table.checks ?? []).length > 0) {
      lines.push(
        '  // Check constraints live in schema.sql and must be migrated from the SQL contract.',
      );
    }
    lines.push(`  @@map(${JSON.stringify(table.name)})`);
    lines.push('}');
    lines.push('');
  }

  return `${lines.join('\n').trimEnd()}\n`;
}

function typeOrmDecorator(column) {
  if (column.primaryKey && column.defaultSql === 'gen_random_uuid()') {
    return `@PrimaryGeneratedColumn("uuid", { name: ${JSON.stringify(column.name)} })`;
  }
  if (column.primaryKey && column.sqlType === 'bigserial') {
    return `@PrimaryGeneratedColumn("increment", { name: ${JSON.stringify(column.name)}, type: "bigint" })`;
  }
  if (column.primaryKey) {
    return `@PrimaryColumn(${typeOrmOptions(column)})`;
  }
  return `@Column(${typeOrmOptions(column)})`;
}

function typeOrmOptions(column) {
  const options = [
    `name: ${JSON.stringify(column.name)}`,
    `type: ${JSON.stringify(typeOrmColumnType(column))}`,
  ];
  if (column.maxLength) {
    options.push(`length: ${column.maxLength}`);
  }
  if (!column.notNull) {
    options.push('nullable: true');
  }
  if (column.defaultSql && !column.primaryKey) {
    options.push(`default: () => ${JSON.stringify(column.defaultSql)}`);
  }
  return `{ ${options.join(', ')} }`;
}

function typeOrmColumnType(column) {
  if (column.sqlType === 'timestamptz') {
    return 'timestamptz';
  }
  if (column.sqlType === 'double') {
    return 'double precision';
  }
  return column.sqlType;
}

function typeScriptType(column) {
  let baseType;
  switch (column.kind) {
    case 'integer':
    case 'bigint':
    case 'float':
      baseType = 'number';
      break;
    case 'boolean':
      baseType = 'boolean';
      break;
    case 'jsonObject':
      baseType = 'Record<string, unknown>';
      break;
    case 'jsonArray':
      baseType = 'unknown[]';
      break;
    case 'timestamp':
      baseType = 'Date';
      break;
    default:
      baseType = 'string';
      break;
  }
  return column.notNull ? baseType : `${baseType} | null`;
}

function prismaField(column) {
  const fieldName = camel(column.name);
  const attributes = [];
  if (fieldName !== column.name) {
    attributes.push(`@map(${JSON.stringify(column.name)})`);
  }
  if (column.primaryKey) {
    attributes.push('@id');
  }
  const defaultAttribute = prismaDefault(column);
  if (defaultAttribute) {
    attributes.push(defaultAttribute);
  }
  const dbAttribute = prismaDbAttribute(column);
  if (dbAttribute) {
    attributes.push(dbAttribute);
  }
  return `${fieldName} ${prismaType(column)} ${attributes.join(' ')}`.trimEnd();
}

function prismaType(column) {
  let baseType;
  switch (column.kind) {
    case 'integer':
      baseType = 'Int';
      break;
    case 'bigint':
      baseType = 'BigInt';
      break;
    case 'float':
      baseType = 'Float';
      break;
    case 'boolean':
      baseType = 'Boolean';
      break;
    case 'jsonObject':
    case 'jsonArray':
      baseType = 'Json';
      break;
    case 'timestamp':
      baseType = 'DateTime';
      break;
    default:
      baseType = 'String';
      break;
  }
  return column.notNull ? baseType : `${baseType}?`;
}

function prismaDbAttribute(column) {
  if (column.sqlType === 'uuid') {
    return '@db.Uuid';
  }
  if (column.sqlType === 'varchar') {
    return `@db.VarChar(${column.maxLength})`;
  }
  if (column.sqlType === 'text') {
    return '@db.Text';
  }
  if (column.sqlType === 'bigint' || column.sqlType === 'bigserial') {
    return '@db.BigInt';
  }
  if (column.sqlType === 'real') {
    // Prisma's Float defaults to `double precision`; pin float4 columns explicitly.
    return '@db.Real';
  }
  if (column.sqlType === 'timestamptz') {
    return '@db.Timestamptz(6)';
  }
  return undefined;
}

function prismaDefault(column) {
  if (column.sqlType === 'bigserial') {
    return '@default(autoincrement())';
  }
  if (!column.defaultSql) {
    return undefined;
  }
  if (column.defaultSql === 'gen_random_uuid()') {
    return '@default(dbgenerated("gen_random_uuid()"))';
  }
  if (column.defaultSql === 'now()') {
    return '@default(now())';
  }
  if (column.kind === 'jsonObject') {
    return '@default("{}")';
  }
  if (column.kind === 'jsonArray') {
    return '@default("[]")';
  }
  if (typeof column.defaultValue === 'string') {
    return `@default(${JSON.stringify(column.defaultValue)})`;
  }
  if (typeof column.defaultValue === 'number' || typeof column.defaultValue === 'boolean') {
    return `@default(${column.defaultValue})`;
  }
  return `@default(dbgenerated(${JSON.stringify(column.defaultSql)}))`;
}

function renderPythonSqlAlchemy(contract) {
  const lines = [
    ...generatedNotice('#'),
    'from __future__ import annotations',
    '',
    'from datetime import datetime',
    'from typing import Any, Literal',
    'from uuid import UUID',
    '',
    'from pydantic import BaseModel, ConfigDict, Field, field_validator',
    'from sqlalchemy import BigInteger, Boolean, CheckConstraint, DateTime, Index, Integer, SmallInteger, String, Text, text',
    'from sqlalchemy.dialects.postgresql import DOUBLE_PRECISION, JSONB, REAL, UUID as PgUUID',
    'from sqlalchemy.orm import DeclarativeBase, Mapped, mapped_column',
    '',
    '',
    'class Base(DeclarativeBase):',
    '    pass',
    '',
  ];

  for (const table of contract.tables) {
    const baseName = table.names?.rust ?? pascal(table.name);
    const rowName = `${baseName}Row`;
    const insertName = `${baseName}Insert`;

    for (const column of table.columns.filter((column) => column.kind === 'enum')) {
      const alias = `${baseName}${pascal(column.name)}`;
      lines.push(`${alias} = Literal[${column.enumValues.map(pyString).join(', ')}]`);
    }
    if (table.columns.some((column) => column.kind === 'enum')) {
      lines.push('');
    }

    lines.push(`class ${baseName}(Base):`);
    lines.push(`    __tablename__ = ${pyString(table.name)}`);
    lines.push('    __table_args__ = (');
    for (const checkConstraint of table.checks ?? []) {
      lines.push(
        `        CheckConstraint(${pyString(checkConstraint.sql)}, name=${pyString(checkConstraint.name)}),`,
      );
    }
    for (const tableIndex of table.indexes ?? []) {
      lines.push(`        ${pythonSqlAlchemyIndex(tableIndex)},`);
    }
    // A trailing dict in __table_args__ carries table kwargs; use it to place the table in its
    // Postgres schema (e.g. benefactor) instead of the default search_path.
    if (table.schema && table.schema !== 'public') {
      lines.push(`        {"schema": ${pyString(table.schema)}},`);
    }
    lines.push('    )');
    lines.push('');
    for (const column of table.columns) {
      lines.push(`    ${column.name}: Mapped[${pythonOrmType(column)}] = ${pythonMappedColumn(column)}`);
    }
    lines.push('');

    lines.push(`class ${rowName}(BaseModel):`);
    lines.push('    model_config = ConfigDict(from_attributes=True)');
    lines.push('');
    for (const column of table.columns) {
      lines.push(`    ${camel(column.name)}: ${pythonPydanticRowType(table, column)}${pythonPydanticField(column, false)}`);
    }
    lines.push(...pythonValidators(table));
    lines.push('');

    lines.push(`class ${insertName}(BaseModel):`);
    lines.push('    model_config = ConfigDict(extra="forbid")');
    lines.push('');
    for (const column of table.columns) {
      lines.push(`    ${camel(column.name)}: ${pythonPydanticInsertType(table, column)}${pythonPydanticField(column, true)}`);
    }
    lines.push(...pythonValidators(table));
    lines.push('');
  }

  return `${lines.join('\n').trimEnd()}\n`;
}

function renderGoGormMod() {
  return `${[
    'module dd-pg-defs-gorm',
    '',
    'go 1.23',
    '',
    'require (',
    '\tgithub.com/google/uuid v1.6.0',
    '\tgorm.io/datatypes v1.2.7',
    '\tgorm.io/gorm v1.31.1',
    ')',
  ].join('\n')}\n`;
}

function renderGoGorm(contract) {
  const lines = [
    ...generatedNotice('//'),
    'package pgdefs',
    '',
    'import (',
    '\t"encoding/json"',
    '\t"errors"',
    '\t"regexp"',
    '\t"time"',
    '',
    '\t"github.com/google/uuid"',
    '\t"gorm.io/datatypes"',
    ')',
    '',
    ...renderGoRegexConstants(contract),
  ];

  for (const table of contract.tables) {
    const baseName = table.names?.rust ?? pascal(table.name);
    lines.push(`const ${baseName}Table = ${JSON.stringify(physicalName(table))}`);
    lines.push(`const ${baseName}SelectSQL = ${goRawString(renderSelectSql(table))}`);
    lines.push('');

    for (const column of table.columns.filter((column) => column.kind === 'enum')) {
      lines.push(`var ${baseName}${pascal(column.name)}Values = []string{${column.enumValues.map(JSON.stringify).join(', ')}}`);
    }
    if (table.columns.some((column) => column.kind === 'enum')) {
      lines.push('');
    }

    lines.push(`type ${baseName}Gorm struct {`);
    for (const column of table.columns) {
      lines.push(`\t${pascal(column.name)} ${goType(column)} \`${goGormTag(column)} json:"${camel(column.name)}${column.notNull ? '' : ',omitempty'}"\``);
    }
    lines.push('}');
    lines.push('');
    lines.push(`func (${baseName}Gorm) TableName() string { return ${baseName}Table }`);
    lines.push('');
    lines.push(`func (value ${baseName}Gorm) Validate() error {`);
    lines.push(...goValidationStatements(table, 'value', 'gorm'));
    lines.push('\treturn nil');
    lines.push('}');
    lines.push('');
  }

  lines.push('func validateJSONString(value datatypes.JSON) bool {');
  lines.push('\tif len(value) == 0 {');
  lines.push('\t\treturn true');
  lines.push('\t}');
  lines.push('\treturn json.Valid(value)');
  lines.push('}');
  lines.push('');
  lines.push('func containsString(values []string, value string) bool {');
  lines.push('\tfor _, item := range values {');
  lines.push('\t\tif item == value {');
  lines.push('\t\t\treturn true');
  lines.push('\t\t}');
  lines.push('\t}');
  lines.push('\treturn false');
  lines.push('}');

  return `${lines.join('\n').trimEnd()}\n`;
}

function renderGoBunMod() {
  return `${[
    'module dd-pg-defs-bun',
    '',
    'go 1.23',
    '',
    'require (',
    '\tgithub.com/google/uuid v1.6.0',
    '\tgithub.com/uptrace/bun v1.2.16',
    ')',
  ].join('\n')}\n`;
}

function renderGoBun(contract) {
  const lines = [
    ...generatedNotice('//'),
    'package pgdefs',
    '',
    'import (',
    '\t"encoding/json"',
    '\t"errors"',
    '\t"regexp"',
    '\t"time"',
    '',
    '\t"github.com/google/uuid"',
    '\t"github.com/uptrace/bun"',
    ')',
    '',
    ...renderGoRegexConstants(contract),
  ];

  for (const table of contract.tables) {
    const baseName = table.names?.rust ?? pascal(table.name);
    lines.push(`const ${baseName}Table = ${JSON.stringify(physicalName(table))}`);
    lines.push(`const ${baseName}SelectSQL = ${goRawString(renderSelectSql(table))}`);
    lines.push('');
    for (const column of table.columns.filter((column) => column.kind === 'enum')) {
      lines.push(`var ${baseName}${pascal(column.name)}Values = []string{${column.enumValues.map(JSON.stringify).join(', ')}}`);
    }
    if (table.columns.some((column) => column.kind === 'enum')) {
      lines.push('');
    }

    lines.push(`type ${baseName}Bun struct {`);
    lines.push(`\tbun.BaseModel \`bun:"table:${physicalName(table)}"\``);
    for (const column of table.columns) {
      lines.push(`\t${pascal(column.name)} ${goBunType(column)} \`${goBunTag(column)} json:"${camel(column.name)}${column.notNull ? '' : ',omitempty'}"\``);
    }
    lines.push('}');
    lines.push('');
    lines.push(`func (value ${baseName}Bun) Validate() error {`);
    lines.push(...goValidationStatements(table, 'value', 'bun'));
    lines.push('\treturn nil');
    lines.push('}');
    lines.push('');
  }

  lines.push('func validateRawJSON(value json.RawMessage) bool {');
  lines.push('\tif len(value) == 0 {');
  lines.push('\t\treturn true');
  lines.push('\t}');
  lines.push('\treturn json.Valid(value)');
  lines.push('}');
  lines.push('');
  lines.push('func containsString(values []string, value string) bool {');
  lines.push('\tfor _, item := range values {');
  lines.push('\t\tif item == value {');
  lines.push('\t\t\treturn true');
  lines.push('\t\t}');
  lines.push('\t}');
  lines.push('\treturn false');
  lines.push('}');

  return `${lines.join('\n').trimEnd()}\n`;
}

function renderDriftPubspec() {
  return `${[
    'name: dd_pg_defs_drift',
    'description: Generated Drift table definitions for remote Postgres definitions.',
    'version: 0.1.0',
    'publish_to: none',
    '',
    'environment:',
    "  sdk: '>=3.3.0 <4.0.0'",
    '',
    'dependencies:',
    '  drift: ^2.20.0',
    '  drift_postgres: ^1.5.0',
    '',
    'dev_dependencies:',
    '  build_runner: ^2.4.0',
    '  drift_dev: ^2.20.0',
  ].join('\n')}\n`;
}

function renderDriftDart(contract) {
  // Drift table definitions render every column as a `Column` getter so consumers can run
  // `dart run build_runner build` to generate accompanying DAOs without re-modelling the schema.
  // We deliberately use the explicit `customConstraint` fallback for things Drift cannot natively
  // express (jsonb defaults, partial indexes, FKs to non-rowid tables) so the database remains
  // the source of truth and Drift never silently drops a constraint.
  const lines = [
    ...generatedNotice('//'),
    '',
    "import 'package:drift/drift.dart';",
    '',
  ];

  for (const table of contract.tables) {
    const className = table.names?.rust ?? pascal(table.name);
    const hasIntegerPk = table.columns.some(
      (column) => column.primaryKey && (column.sqlType === 'bigserial' || column.sqlType === 'integer'),
    );
    lines.push('@DataClassName(' + JSON.stringify(`${className}Data`) + ')');
    lines.push(`class ${className}Table extends Table {`);
    lines.push(`  @override String get tableName => ${JSON.stringify(table.name)};`);
    lines.push('');
    // Drift uses an implicit rowid/id when withoutRowId is false. Tables whose PK is a UUID need
    // `withoutRowId = true` so Drift treats the explicit id column as the primary key; tables with
    // a bigserial PK keep the default rowid behavior so the column maps to Drift's auto-increment.
    if (!hasIntegerPk) {
      lines.push('  @override bool get withoutRowId => true;');
      lines.push('');
    }
    for (const column of table.columns) {
      lines.push(`  ${driftColumn(column)}`);
    }
    const primaryKeyColumns = table.columns
      .filter((column) => column.primaryKey)
      .map((column) => `        ${camel(column.name)},`);
    if (primaryKeyColumns.length > 0) {
      lines.push('');
      lines.push('  @override');
      lines.push('  Set<Column> get primaryKey => {');
      lines.push(...primaryKeyColumns);
      lines.push('  };');
    }
    lines.push('}');
    lines.push('');
  }

  lines.push('// Drift annotation users should re-export the table classes via:');
  lines.push("// @DriftDatabase(tables: [...registeredDriftTables])");
  lines.push('const List<Type> registeredDriftTables = <Type>[');
  for (const table of contract.tables) {
    lines.push(`  ${table.names?.rust ?? pascal(table.name)}Table,`);
  }
  lines.push('];');

  return `${lines.join('\n').trimEnd()}\n`;
}

function driftColumn(column) {
  const fieldName = camel(column.name);
  const builder = driftColumnBuilder(column);
  let chain = builder;
  if (column.maxLength && column.kind === 'string') {
    chain += `.withLength(max: ${column.maxLength})`;
  }
  // Skip clientDefault for server-generated columns (uuid PKs, bigserial PKs, server-default
  // timestamps). Drift's clientDefault runs on the Dart side and would shadow the server-side
  // default with a stale or wrong value.
  if (column.defaultSql && !column.generated && !isServerGeneratedDefault(column)) {
    const expression = driftDefaultExpression(column);
    if (expression !== undefined) {
      chain += `.clientDefault(() => ${expression})`;
    }
  }
  if (!column.notNull) {
    chain += '.nullable()';
  }
  // Drift cannot express partial / GIN / FK constraints in code; surface them via customConstraint
  // so migrations driven from drift_dev still write the right SQL.
  const constraint = driftCustomConstraint(column);
  if (constraint) {
    chain += `.customConstraint(${JSON.stringify(constraint)})`;
  }
  return `${driftColumnType(column)} get ${fieldName} => ${chain}();`;
}

function isServerGeneratedDefault(column) {
  if (!column.defaultSql) {
    return false;
  }
  const sql = column.defaultSql.toLowerCase();
  return (
    sql.includes('gen_random_uuid')
    || sql === 'now()'
    || column.sqlType === 'bigserial'
  );
}

function driftColumnBuilder(column) {
  const named = `named(${JSON.stringify(column.name)})`;
  switch (column.kind) {
    case 'integer':
      return `integer().${named}`;
    case 'bigint':
      return `int64().${named}`;
    case 'float':
      return `real().${named}`;
    case 'boolean':
      return `boolean().${named}`;
    case 'jsonObject':
    case 'jsonArray':
      return `text().${named}`;
    case 'timestamp':
      return `dateTime().${named}`;
    default:
      return `text().${named}`;
  }
}

function driftColumnType(column) {
  switch (column.kind) {
    case 'integer':
      return 'IntColumn';
    case 'bigint':
      return 'Int64Column';
    case 'float':
      return 'RealColumn';
    case 'boolean':
      return 'BoolColumn';
    case 'timestamp':
      return 'DateTimeColumn';
    default:
      return 'TextColumn';
  }
}

function driftDefaultExpression(column) {
  if (column.kind === 'boolean') {
    return column.defaultValue ? 'true' : 'false';
  }
  if (column.kind === 'integer' || column.kind === 'bigint' || column.kind === 'float') {
    if (column.defaultValue === undefined) {
      return undefined;
    }
    return Number(column.defaultValue).toString();
  }
  if (column.kind === 'jsonObject') {
    return "'{}'";
  }
  if (column.kind === 'jsonArray') {
    return "'[]'";
  }
  if (column.kind === 'timestamp') {
    return undefined;
  }
  if (typeof column.defaultValue === 'string') {
    return dartLiteralString(column.defaultValue);
  }
  return undefined;
}

function dartLiteralString(value) {
  // Use single-quoted Dart string literals so multi-line content stays compact. Backslashes,
  // single quotes, and `$` need escaping; double quotes pass through.
  const escaped = value
    .replace(/\\/g, '\\\\')
    .replace(/'/g, "\\'")
    .replace(/\$/g, '\\$');
  return `'${escaped}'`;
}

function driftCustomConstraint(column) {
  const parts = [];
  if (column.sqlType === 'uuid') {
    parts.push('UUID');
  }
  if (column.sqlType === 'jsonb') {
    parts.push('JSONB');
  }
  if (column.sqlType === 'timestamptz') {
    parts.push('TIMESTAMPTZ');
  }
  if (column.sqlType === 'bigserial') {
    parts.push('BIGSERIAL');
  }
  if (column.foreignKey) {
    parts.push(
      `REFERENCES ${column.foreignKey.table} (${column.foreignKey.column})`,
    );
  }
  return parts.join(' ');
}

function renderObjectBoxPubspec() {
  return `${[
    'name: dd_pg_defs_objectbox',
    'description: Generated ObjectBox entity classes mirroring the canonical Postgres rows.',
    'version: 0.1.0',
    'publish_to: none',
    '',
    'environment:',
    "  sdk: '>=3.3.0 <4.0.0'",
    '',
    'dependencies:',
    '  objectbox: ^4.0.0',
    '  objectbox_flutter_libs: ^4.0.0',
    '',
    'dev_dependencies:',
    '  build_runner: ^2.4.0',
    '  objectbox_generator: ^4.0.0',
  ].join('\n')}\n`;
}

function renderObjectBoxDart(contract) {
  // ObjectBox stores rows in an embedded NoSQL key/value store, but downstream Flutter clients
  // still want strongly-typed mirrors of every server table for offline-first caching. We map
  // Postgres UUIDs to ObjectBox `String` fields with `@Unique()` rather than ObjectBox `Id`s
  // because changing the canonical id type silently in clients is dangerous.
  const lines = [
    ...generatedNotice('//'),
    '',
    "import 'dart:convert';",
    "import 'package:objectbox/objectbox.dart';",
    '',
  ];

  for (const table of contract.tables) {
    const className = table.names?.rust ?? pascal(table.name);
    lines.push('@Entity()');
    lines.push(`class ${className}ObjectBox {`);
    lines.push('  @Id()');
    lines.push('  int obxId = 0;');
    lines.push('');
    for (const column of table.columns) {
      lines.push(...objectBoxField(column));
    }
    lines.push('');
    lines.push(`  ${className}ObjectBox({`);
    for (const column of table.columns) {
      // Required mirrors `not null` directly so the constructor matches the field type. JSON
      // columns always get a non-empty default ("{}" / "[]") at the SQL layer, so they qualify
      // as required just like any other not-null column.
      const required = column.notNull;
      lines.push(`    ${required ? 'required ' : ''}this.${camel(column.name)},`);
    }
    lines.push('  });');
    lines.push('');
    lines.push(`  Map<String, Object?> toJson() => <String, Object?>{`);
    for (const column of table.columns) {
      lines.push(`    ${JSON.stringify(camel(column.name))}: ${objectBoxToJsonExpression(column)},`);
    }
    lines.push('  };');
    lines.push('');
    lines.push(`  static ${className}ObjectBox fromJson(Map<String, Object?> json) {`);
    lines.push(`    return ${className}ObjectBox(`);
    for (const column of table.columns) {
      lines.push(`      ${camel(column.name)}: ${objectBoxFromJsonExpression(column)},`);
    }
    lines.push('    );');
    lines.push('  }');
    lines.push('}');
    lines.push('');
  }

  return `${lines.join('\n').trimEnd()}\n`;
}

function objectBoxFieldKind(column) {
  return column.kind;
}

function objectBoxField(column) {
  const lines = [];
  const fieldName = camel(column.name);
  const baseType = objectBoxDartType(column);
  const declaration = column.notNull ? baseType : `${baseType}?`;

  // Only mark the actual primary key column as unique. Other UUID columns (e.g. created_by /
  // updated_by audit fields) can repeat across rows, so `@Unique()` would silently break inserts.
  if (column.primaryKey) {
    lines.push('  @Unique()');
  }
  if (column.kind === 'jsonObject' || column.kind === 'jsonArray') {
    lines.push("  // Stored as JSON-encoded string because ObjectBox lacks a native jsonb type.");
  }
  lines.push(`  ${declaration} ${fieldName};`);
  lines.push('');
  return lines;
}

function objectBoxDartType(column) {
  switch (column.kind) {
    case 'integer':
      return 'int';
    case 'bigint':
      return 'int';
    case 'float':
      return 'double';
    case 'boolean':
      return 'bool';
    case 'jsonObject':
    case 'jsonArray':
      return 'String';
    case 'timestamp':
      return 'String';
    default:
      return 'String';
  }
}

function objectBoxFromJsonExpression(column) {
  const key = JSON.stringify(camel(column.name));
  const required = column.notNull;
  if (column.kind === 'integer' || column.kind === 'bigint') {
    return required
      ? `(json[${key}] as num).toInt()`
      : `json[${key}] == null ? null : (json[${key}] as num).toInt()`;
  }
  if (column.kind === 'float') {
    return required
      ? `(json[${key}] as num).toDouble()`
      : `json[${key}] == null ? null : (json[${key}] as num).toDouble()`;
  }
  if (column.kind === 'boolean') {
    return required ? `json[${key}] as bool` : `json[${key}] as bool?`;
  }
  if (column.kind === 'jsonObject' || column.kind === 'jsonArray') {
    return `json[${key}] is String ? json[${key}] as String : jsonEncode(json[${key}])`;
  }
  return required ? `json[${key}] as String` : `json[${key}] as String?`;
}

function objectBoxToJsonExpression(column) {
  const fieldName = camel(column.name);
  if (column.kind === 'jsonObject' || column.kind === 'jsonArray') {
    return `jsonDecode(${fieldName})`;
  }
  return fieldName;
}

function renderDartPubspec() {
  return `${[
    'name: dd_pg_defs',
    'description: Generated Dart models and SQL constants for remote Postgres definitions.',
    'version: 0.1.0',
    'publish_to: none',
    '',
    'environment:',
    "  sdk: '>=3.3.0 <4.0.0'",
  ].join('\n')}\n`;
}

function renderDart(contract) {
  const lines = [
    ...generatedNotice('//'),
    '',
    "import 'dart:convert';",
    '',
  ];

  for (const table of contract.tables) {
    const baseName = table.names?.rust ?? pascal(table.name);
    lines.push(`const ${camel(baseName)}Table = ${JSON.stringify(physicalName(table))};`);
    lines.push(`const ${camel(baseName)}SelectSql = ${JSON.stringify(renderSelectSql(table, { jsonAsText: true }))};`);
    lines.push('');

    for (const column of table.columns.filter((column) => column.kind === 'enum')) {
      lines.push(`const ${camel(baseName)}${pascal(column.name)}Values = <String>[${column.enumValues.map(JSON.stringify).join(', ')}];`);
    }
    if (table.columns.some((column) => column.kind === 'enum')) {
      lines.push('');
    }

    lines.push(`class ${baseName}Row {`);
    lines.push(`  const ${baseName}Row({`);
    for (const column of table.columns) {
      lines.push(`    ${column.notNull ? 'required ' : ''}this.${camel(column.name)},`);
    }
    lines.push('  });');
    lines.push('');
    for (const column of table.columns) {
      lines.push(`  final ${dartType(column)} ${camel(column.name)};`);
    }
    lines.push('');
    lines.push(`  factory ${baseName}Row.fromJson(Map<String, Object?> json) {`);
    lines.push(`    return ${baseName}Row(`);
    for (const column of table.columns) {
      lines.push(`      ${camel(column.name)}: ${dartReadJson(column, camel(column.name))},`);
    }
    lines.push('    );');
    lines.push('  }');
    lines.push('');
    lines.push('  Map<String, Object?> toJson() => <String, Object?>{');
    for (const column of table.columns) {
      lines.push(`    ${JSON.stringify(camel(column.name))}: ${dartWriteJson(column, camel(column.name))},`);
    }
    lines.push('  };');
    lines.push('');
    lines.push('  List<String> validate() {');
    lines.push('    final errors = <String>[];');
    lines.push(...dartValidationStatements(table));
    lines.push('    return errors;');
    lines.push('  }');
    lines.push('}');
    lines.push('');
  }

  lines.push('String _readRequiredString(Map<String, Object?> json, String key) {');
  lines.push('  final value = json[key];');
  lines.push('  if (value is String) return value;');
  lines.push("  throw FormatException('Expected string for $key');");
  lines.push('}');
  lines.push('');
  lines.push('String? _readOptionalString(Map<String, Object?> json, String key) {');
  lines.push('  final value = json[key];');
  lines.push('  if (value == null) return null;');
  lines.push('  if (value is String) return value;');
  lines.push("  throw FormatException('Expected optional string for $key');");
  lines.push('}');
  lines.push('');
  lines.push('int _readRequiredInt(Map<String, Object?> json, String key) {');
  lines.push('  final value = json[key];');
  lines.push('  if (value is int) return value;');
  lines.push("  throw FormatException('Expected int for $key');");
  lines.push('}');
  lines.push('');
  lines.push('int? _readOptionalInt(Map<String, Object?> json, String key) {');
  lines.push('  final value = json[key];');
  lines.push('  if (value == null) return null;');
  lines.push('  if (value is int) return value;');
  lines.push("  throw FormatException('Expected optional int for $key');");
  lines.push('}');
  lines.push('');
  lines.push('double _readRequiredDouble(Map<String, Object?> json, String key) {');
  lines.push('  final value = json[key];');
  lines.push('  if (value is num) return value.toDouble();');
  lines.push("  throw FormatException('Expected double for $key');");
  lines.push('}');
  lines.push('');
  lines.push('double? _readOptionalDouble(Map<String, Object?> json, String key) {');
  lines.push('  final value = json[key];');
  lines.push('  if (value == null) return null;');
  lines.push('  if (value is num) return value.toDouble();');
  lines.push("  throw FormatException('Expected optional double for $key');");
  lines.push('}');
  lines.push('');
  lines.push('bool _readRequiredBool(Map<String, Object?> json, String key) {');
  lines.push('  final value = json[key];');
  lines.push('  if (value is bool) return value;');
  lines.push("  throw FormatException('Expected bool for $key');");
  lines.push('}');
  lines.push('');
  lines.push('bool? _readOptionalBool(Map<String, Object?> json, String key) {');
  lines.push('  final value = json[key];');
  lines.push('  if (value == null) return null;');
  lines.push('  if (value is bool) return value;');
  lines.push("  throw FormatException('Expected optional bool for $key');");
  lines.push('}');
  lines.push('');
  lines.push('Map<String, Object?> _readRequiredObject(Map<String, Object?> json, String key) {');
  lines.push('  final value = json[key];');
  lines.push('  if (value is Map) return value.cast<String, Object?>();');
  lines.push('  if (value is String) {');
  lines.push('    final decoded = jsonDecode(value);');
  lines.push('    if (decoded is Map) return decoded.cast<String, Object?>();');
  lines.push('  }');
  lines.push("  throw FormatException('Expected JSON object for $key');");
  lines.push('}');
  lines.push('');
  lines.push('List<Object?> _readRequiredArray(Map<String, Object?> json, String key) {');
  lines.push('  final value = json[key];');
  lines.push('  if (value is List) return value.cast<Object?>();');
  lines.push('  if (value is String) {');
  lines.push('    final decoded = jsonDecode(value);');
  lines.push('    if (decoded is List) return decoded.cast<Object?>();');
  lines.push('  }');
  lines.push("  throw FormatException('Expected JSON array for $key');");
  lines.push('}');

  return `${lines.join('\n').trimEnd()}\n`;
}

function drizzleColumn(column) {
  let builder;
  switch (column.sqlType) {
    case 'uuid':
      builder = `uuid(${JSON.stringify(column.name)})`;
      break;
    case 'varchar':
      builder = `varchar(${JSON.stringify(column.name)}, { length: ${column.maxLength} })`;
      break;
    case 'text':
      builder = `text(${JSON.stringify(column.name)})`;
      break;
    case 'smallint':
      builder = `smallint(${JSON.stringify(column.name)})`;
      break;
    case 'integer':
      builder = `integer(${JSON.stringify(column.name)})`;
      break;
    case 'bigint':
      builder = `bigint(${JSON.stringify(column.name)}, { mode: "number" })`;
      break;
    case 'bigserial':
      builder = `bigserial(${JSON.stringify(column.name)}, { mode: "number" })`;
      break;
    case 'double':
      builder = `doublePrecision(${JSON.stringify(column.name)})`;
      break;
    case 'real':
      builder = `real(${JSON.stringify(column.name)})`;
      break;
    case 'boolean':
      builder = `boolean(${JSON.stringify(column.name)})`;
      break;
    case 'jsonb':
      builder = `jsonb(${JSON.stringify(column.name)})`;
      break;
    case 'timestamptz':
      builder = `timestamp(${JSON.stringify(column.name)}, { withTimezone: true, mode: "string" })`;
      break;
    default:
      throw new Error(`Unsupported SQL type for Drizzle: ${column.sqlType}`);
  }

  if (column.defaultSql) {
    builder += `.default(sql\`${escapeTemplate(column.defaultSql)}\`)`;
  }
  if (column.primaryKey) {
    builder += '.primaryKey()';
  }
  if (column.notNull && !column.primaryKey) {
    builder += '.notNull()';
  }
  return builder;
}

function drizzleIndex(tableIndex) {
  const factory = tableIndex.unique ? 'uniqueIndex' : 'index';
  const parts = [`${factory}(${JSON.stringify(tableIndex.name)})`];
  const columns = tableIndex.columns.map((column) => {
    if (typeof column === 'string') {
      return `table.${camel(column)}`;
    }
    const base = `table.${camel(column.name)}`;
    return column.order === 'desc' ? `${base}.desc()` : base;
  });

  if (tableIndex.method) {
    parts.push(`using(${JSON.stringify(tableIndex.method)}, ${columns.join(', ')})`);
  } else {
    parts.push(`on(${columns.join(', ')})`);
  }

  if (tableIndex.where) {
    parts.push(`where(sql.raw(${JSON.stringify(tableIndex.where)}))`);
  }

  return parts.join('.');
}

function zodColumn(table, column, options) {
  let schemaExpression;
  switch (column.kind) {
    case 'uuid':
      schemaExpression = 'z.string().uuid()';
      break;
    case 'string':
      schemaExpression = 'z.string()';
      break;
    case 'enum':
      schemaExpression = `${camel(table.names?.rust ?? pascal(table.name))}${pascal(column.name)}Schema`;
      break;
    case 'integer':
    case 'bigint':
      schemaExpression = 'z.number().int()';
      break;
    case 'float':
      schemaExpression = 'z.number()';
      break;
    case 'boolean':
      schemaExpression = 'z.boolean()';
      break;
    case 'jsonObject':
      schemaExpression = 'jsonObjectSchema';
      break;
    case 'jsonArray':
      schemaExpression = 'jsonArraySchema';
      break;
    case 'timestamp':
      schemaExpression = 'z.string().datetime()';
      break;
    default:
      throw new Error(`Unsupported validation kind: ${column.kind}`);
  }

  const validation = column.validation ?? {};
  if (validation.literal) {
    schemaExpression = `z.literal(${screaming(table.name)}_${screaming(column.name)}_DEFAULT)`;
  } else {
    if (column.kind === 'string' && validation.minLength) {
      schemaExpression += `.min(${validation.minLength})`;
    }
    if (column.kind === 'string' && validation.maxLength) {
      schemaExpression += `.max(${validation.maxLength})`;
    }
    if (column.kind === 'string' && validation.regex) {
      schemaExpression += `.regex(new RegExp(${JSON.stringify(validation.regex)}))`;
    }
    if (column.kind === 'string' && validation.maxBytes) {
      schemaExpression += `.refine((value) => byteLength(value) <= ${validation.maxBytes}, "Must be at most ${validation.maxBytes} bytes")`;
    }
    if (validation.min !== undefined) {
      schemaExpression += `.min(${validation.min})`;
    }
    if (validation.max !== undefined) {
      schemaExpression += `.max(${validation.max})`;
    }
  }

  if (!column.notNull) {
    schemaExpression += '.nullable()';
  }

  if (options.insert && (column.generated || column.defaultSql || !column.notNull)) {
    schemaExpression += '.optional()';
  }

  if (options.insert && column.defaultValue !== undefined && column.notNull) {
    schemaExpression += `.default(${JSON.stringify(column.defaultValue)})`;
  }

  return schemaExpression;
}

function pythonSqlAlchemyIndex(tableIndex) {
  const args = [pyString(tableIndex.name)];
  for (const column of tableIndex.columns) {
    if (typeof column === 'string') {
      args.push(pyString(column));
    } else if (column.order) {
      args.push(`text(${pyString(`${column.name} ${column.order}`)})`);
    } else {
      args.push(pyString(column.name));
    }
  }
  const options = [];
  if (tableIndex.unique) {
    options.push('unique=True');
  }
  if (tableIndex.method) {
    options.push(`postgresql_using=${pyString(tableIndex.method)}`);
  }
  if (tableIndex.where) {
    options.push(`postgresql_where=text(${pyString(tableIndex.where)})`);
  }
  return `Index(${[...args, ...options].join(', ')})`;
}

function pythonOrmType(column) {
  const baseType = pythonBaseType(column);
  return column.notNull ? baseType : `${baseType} | None`;
}

function pythonBaseType(column) {
  switch (column.kind) {
    case 'uuid':
      return 'UUID';
    case 'integer':
    case 'bigint':
      return 'int';
    case 'float':
      return 'float';
    case 'boolean':
      return 'bool';
    case 'jsonObject':
      return 'dict[str, Any]';
    case 'jsonArray':
      return 'list[Any]';
    case 'timestamp':
      return 'datetime';
    default:
      return 'str';
  }
}

function pythonMappedColumn(column) {
  const args = [pythonSqlAlchemyType(column)];
  const options = [];
  if (column.primaryKey) {
    options.push('primary_key=True');
  }
  if (!column.notNull) {
    options.push('nullable=True');
  } else if (!column.primaryKey) {
    options.push('nullable=False');
  }
  if (column.defaultSql) {
    options.push(`server_default=text(${pyString(column.defaultSql)})`);
  }
  return `mapped_column(${[...args, ...options].join(', ')})`;
}

function pythonSqlAlchemyType(column) {
  switch (column.sqlType) {
    case 'uuid':
      return 'PgUUID(as_uuid=True)';
    case 'varchar':
      return `String(${column.maxLength})`;
    case 'text':
      return 'Text()';
    case 'smallint':
      return 'SmallInteger()';
    case 'integer':
      return 'Integer()';
    case 'bigint':
    case 'bigserial':
      return 'BigInteger()';
    case 'double':
      return 'DOUBLE_PRECISION()';
    case 'real':
      return 'REAL()';
    case 'boolean':
      return 'Boolean()';
    case 'jsonb':
      return 'JSONB()';
    case 'timestamptz':
      return 'DateTime(timezone=True)';
    default:
      return 'Text()';
  }
}

function pythonPydanticType(table, column) {
  if (column.kind === 'enum') {
    return `${table.names?.rust ?? pascal(table.name)}${pascal(column.name)}`;
  }
  return pythonBaseType(column);
}

function pythonPydanticRowType(table, column) {
  const baseType = pythonPydanticType(table, column);
  return column.notNull ? baseType : `${baseType} | None`;
}

function pythonPydanticInsertType(table, column) {
  const baseType = pythonPydanticType(table, column);
  if (column.generated || column.defaultSql || !column.notNull) {
    return `${baseType} | None`;
  }
  return baseType;
}

function pythonPydanticField(column, insertMode) {
  const validation = column.validation ?? {};
  const options = [];
  if (column.kind === 'string') {
    if (validation.minLength) {
      options.push(`min_length=${validation.minLength}`);
    }
    if (validation.maxLength) {
      options.push(`max_length=${validation.maxLength}`);
    }
    if (validation.regex) {
      options.push(`pattern=${pyString(validation.regex)}`);
    }
  }
  if (column.kind === 'integer' || column.kind === 'float') {
    if (validation.min !== undefined) {
      options.push(`ge=${validation.min}`);
    }
    if (validation.max !== undefined) {
      options.push(`le=${validation.max}`);
    }
  }

  const defaultExpression = pythonPydanticDefault(column, insertMode);
  if (defaultExpression?.startsWith('default_factory=')) {
    return ` = Field(${[defaultExpression, ...options].join(', ')})`;
  }
  if (options.length === 0) {
    return defaultExpression ? ` = ${defaultExpression}` : '';
  }
  return ` = Field(${[defaultExpression ?? '...', ...options].join(', ')})`;
}

function pythonPydanticDefault(column, insertMode) {
  if (!insertMode) {
    if (!column.notNull) {
      return 'None';
    }
    return undefined;
  }
  if (column.kind === 'jsonObject' && column.defaultSql) {
    return 'default_factory=dict';
  }
  if (column.kind === 'jsonArray' && column.defaultSql) {
    return 'default_factory=list';
  }
  if (column.defaultValue !== undefined) {
    return pyValue(column.defaultValue);
  }
  if (column.generated || !column.notNull || column.defaultSql) {
    return 'None';
  }
  return undefined;
}

function pythonValidators(table) {
  const lines = [];
  for (const column of table.columns) {
    if (!column.validation?.maxBytes && !column.validation?.literal) {
      continue;
    }
    const fieldName = camel(column.name);
    const validatorName = `validate_${column.name}`;
    lines.push('');
    lines.push(`    @field_validator(${pyString(fieldName)})`);
    lines.push(`    @classmethod`);
    lines.push(`    def ${validatorName}(cls, value):`);
    if (column.validation.literal) {
      lines.push(`        if value is not None and value != ${pyString(column.validation.literal)}:`);
      lines.push(`            raise ValueError(${pyString(`${table.name}.${column.name} must use the managed value`)})`);
    }
    if (column.validation.maxBytes) {
      lines.push(`        if value is not None and len(value.encode("utf-8")) > ${column.validation.maxBytes}:`);
      lines.push(`            raise ValueError(${pyString(`${table.name}.${column.name} exceeds ${column.validation.maxBytes} bytes`)})`);
    }
    lines.push('        return value');
  }
  return lines;
}

function goType(column) {
  const baseType = goBaseType(column, 'gorm');
  return column.notNull ? baseType : `*${baseType}`;
}

function goBunType(column) {
  const baseType = goBaseType(column, 'bun');
  return column.notNull ? baseType : `*${baseType}`;
}

function goBaseType(column, flavor) {
  switch (column.kind) {
    case 'uuid':
      return 'uuid.UUID';
    case 'integer':
      return 'int32';
    case 'bigint':
      return 'int64';
    case 'float':
      return column.sqlType === 'real' ? 'float32' : 'float64';
    case 'boolean':
      return 'bool';
    case 'jsonObject':
    case 'jsonArray':
      return flavor === 'gorm' ? 'datatypes.JSON' : 'json.RawMessage';
    case 'timestamp':
      return 'time.Time';
    default:
      return 'string';
  }
}

function goGormTag(column) {
  const parts = [`column:${column.name}`, `type:${goPgType(column)}`];
  if (column.primaryKey) {
    parts.push('primaryKey');
  }
  if (column.defaultSql) {
    parts.push(`default:${goStructTagValue(column.defaultSql)}`);
  }
  if (column.notNull && !column.primaryKey) {
    parts.push('not null');
  }
  return `gorm:"${parts.join(';')}"`;
}

function goBunTag(column) {
  const parts = [column.name, `type:${goPgType(column)}`];
  if (column.primaryKey) {
    parts.push('pk');
  }
  if (column.defaultSql) {
    parts.push(`default:${goStructTagValue(column.defaultSql)}`);
  }
  if (!column.notNull) {
    parts.push('nullzero');
  }
  return `bun:"${parts.join(',')}"`;
}

function goPgType(column) {
  if (column.sqlType === 'varchar') {
    return `varchar(${column.maxLength})`;
  }
  if (column.sqlType === 'double') {
    return 'double precision';
  }
  return column.sqlType;
}

function goStructTagValue(value) {
  return value.replace(/\\/g, '\\\\').replace(/"/g, '\\"');
}

function goValidationStatements(table, binding, flavor) {
  const lines = [];
  for (const column of table.columns) {
    const field = `${binding}.${pascal(column.name)}`;
    const validation = column.validation ?? {};
    const derefField = column.notNull ? field : `*${field}`;
    const nilGuardStart = column.notNull ? null : `\tif ${field} != nil {`;
    const nestedLines = [];

    if (validation.regex) {
      const patternConst = goRegexConstName(table, column);
      nestedLines.push(
        `if !${patternConst}.MatchString(${derefField}) { return errors.New(${JSON.stringify(`${table.name}.${column.name} does not match the required pattern`)}) }`,
      );
    }
    if (column.kind === 'enum') {
      nestedLines.push(`if !containsString(${table.names?.rust ?? pascal(table.name)}${pascal(column.name)}Values, ${derefField}) { return errors.New(${JSON.stringify(`unsupported ${table.name}.${column.name}`)}) }`);
    }
    if (validation.literal) {
      nestedLines.push(`if ${derefField} != ${JSON.stringify(validation.literal)} { return errors.New(${JSON.stringify(`${table.name}.${column.name} must use the managed value`)}) }`);
    }
    if (validation.maxBytes) {
      nestedLines.push(`if len([]byte(${derefField})) > ${validation.maxBytes} { return errors.New(${JSON.stringify(`${table.name}.${column.name} exceeds ${validation.maxBytes} bytes`)}) }`);
    }
    if (validation.minBytes) {
      nestedLines.push(`if len([]byte(${derefField})) < ${validation.minBytes} { return errors.New(${JSON.stringify(`${table.name}.${column.name} is below ${validation.minBytes} bytes`)}) }`);
    }
    if (column.kind === 'jsonObject' || column.kind === 'jsonArray') {
      const validator = flavor === 'bun' ? 'validateRawJSON' : 'validateJSONString';
      nestedLines.push(`if !${validator}(${derefField}) { return errors.New(${JSON.stringify(`${table.name}.${column.name} must be valid JSON`)}) }`);
    }
    if (column.kind === 'integer' || column.kind === 'bigint' || column.kind === 'float') {
      if (validation.min !== undefined) {
        nestedLines.push(`if ${derefField} < ${validation.min} { return errors.New(${JSON.stringify(`${table.name}.${column.name} is below the minimum`)}) }`);
      }
      if (validation.max !== undefined) {
        nestedLines.push(`if ${derefField} > ${validation.max} { return errors.New(${JSON.stringify(`${table.name}.${column.name} is above the maximum`)}) }`);
      }
    }

    if (nestedLines.length === 0) {
      continue;
    }
    if (nilGuardStart) {
      lines.push(nilGuardStart);
      lines.push(...nestedLines.map((line) => `\t\t${line}`));
      lines.push('\t}');
    } else {
      lines.push(...nestedLines.map((line) => `\t${line}`));
    }
  }
  return lines;
}

function renderGoRegexConstants(contract) {
  const lines = [];
  let emitted = 0;
  for (const table of contract.tables) {
    for (const column of table.columns) {
      const pattern = column.validation?.regex;
      if (!pattern) {
        continue;
      }
      lines.push(
        `var ${goRegexConstName(table, column)} = regexp.MustCompile(${goRawString(pattern)})`,
      );
      emitted += 1;
    }
  }
  if (emitted > 0) {
    lines.push('');
  }
  return lines;
}

function goRegexConstName(table, column) {
  return `${camel(table.names?.rust ?? pascal(table.name))}${pascal(column.name)}Pattern`;
}

function dartType(column) {
  let baseType;
  switch (column.kind) {
    case 'integer':
    case 'bigint':
      baseType = 'int';
      break;
    case 'float':
      baseType = 'double';
      break;
    case 'boolean':
      baseType = 'bool';
      break;
    case 'jsonObject':
      baseType = 'Map<String, Object?>';
      break;
    case 'jsonArray':
      baseType = 'List<Object?>';
      break;
    default:
      baseType = 'String';
      break;
  }
  return column.notNull ? baseType : `${baseType}?`;
}

function dartReadJson(column, fieldName) {
  const key = JSON.stringify(fieldName);
  if (!column.notNull && column.kind !== 'jsonObject' && column.kind !== 'jsonArray') {
    if (column.kind === 'integer' || column.kind === 'bigint') {
      return `_readOptionalInt(json, ${key})`;
    }
    if (column.kind === 'float') {
      return `_readOptionalDouble(json, ${key})`;
    }
    if (column.kind === 'boolean') {
      return `_readOptionalBool(json, ${key})`;
    }
    return `_readOptionalString(json, ${key})`;
  }
  switch (column.kind) {
    case 'integer':
    case 'bigint':
      return `_readRequiredInt(json, ${key})`;
    case 'float':
      return `_readRequiredDouble(json, ${key})`;
    case 'boolean':
      return `_readRequiredBool(json, ${key})`;
    case 'jsonObject':
      return `_readRequiredObject(json, ${key})`;
    case 'jsonArray':
      return `_readRequiredArray(json, ${key})`;
    default:
      return `_readRequiredString(json, ${key})`;
  }
}

function dartWriteJson(column, fieldName) {
  if (column.kind === 'jsonObject' || column.kind === 'jsonArray') {
    return fieldName;
  }
  return fieldName;
}

function dartValidationStatements(table) {
  const lines = [];
  for (const column of table.columns) {
    const field = camel(column.name);
    const validation = column.validation ?? {};
    const guardPrefix = column.notNull ? '' : `${field} != null && `;
    const accessor = `${field}${column.notNull ? '' : '!'}`;
    if (validation.regex) {
      const message = validation.regex.startsWith('^[a-z0-9]')
        ? `${table.name}.${column.name} must be a lowercase slug`
        : `${table.name}.${column.name} does not match the required pattern`;
      lines.push(
        `    if (${guardPrefix}!RegExp(r${dartRawRegexString(validation.regex)}).hasMatch(${accessor})) {`,
      );
      lines.push(`      errors.add(${JSON.stringify(message)});`);
      lines.push('    }');
    }
    if (column.kind === 'enum') {
      lines.push(`    if (${guardPrefix}!${camel(table.names?.rust ?? pascal(table.name))}${pascal(column.name)}Values.contains(${accessor})) {`);
      lines.push(`      errors.add(${JSON.stringify(`unsupported ${table.name}.${column.name}`)});`);
      lines.push('    }');
    }
    if (validation.literal) {
      lines.push(`    if (${guardPrefix}${accessor} != ${JSON.stringify(validation.literal)}) {`);
      lines.push(`      errors.add(${JSON.stringify(`${table.name}.${column.name} must use the managed value`)});`);
      lines.push('    }');
    }
    if (validation.maxBytes) {
      lines.push(`    if (${guardPrefix}utf8.encode(${accessor}).length > ${validation.maxBytes}) {`);
      lines.push(`      errors.add(${JSON.stringify(`${table.name}.${column.name} exceeds ${validation.maxBytes} bytes`)});`);
      lines.push('    }');
    }
    if (validation.minBytes) {
      lines.push(`    if (${guardPrefix}utf8.encode(${accessor}).length < ${validation.minBytes}) {`);
      lines.push(`      errors.add(${JSON.stringify(`${table.name}.${column.name} is below ${validation.minBytes} bytes`)});`);
      lines.push('    }');
    }
    if (column.kind === 'integer') {
      if (validation.min !== undefined) {
        lines.push(`    if (${guardPrefix}${accessor} < ${validation.min}) {`);
        lines.push(`      errors.add(${JSON.stringify(`${table.name}.${column.name} is below the minimum`)});`);
        lines.push('    }');
      }
      if (validation.max !== undefined) {
        lines.push(`    if (${guardPrefix}${accessor} > ${validation.max}) {`);
        lines.push(`      errors.add(${JSON.stringify(`${table.name}.${column.name} is above the maximum`)});`);
        lines.push('    }');
      }
    }
  }
  return lines;
}

function dartRawRegexString(value) {
  // Dart raw strings (`r'...'`) cannot contain the delimiter unescaped, so we cycle through
  // candidates until we find one that does not appear in the regex source.
  const candidates = ["'", '"', "'''", '"""'];
  for (const delim of candidates) {
    if (!value.includes(delim)) {
      return `${delim}${value}${delim}`;
    }
  }
  // Fallback: emit a non-raw string with backslashes escaped so the regex still parses.
  return JSON.stringify(value);
}

function renderRustCargo() {
  return `${[
    '[package]',
    'name = "dd-pg-defs"',
    'version = "0.1.0"',
    'edition = "2021"',
    '',
    '[dependencies]',
    'serde = { version = "1", features = ["derive"] }',
    'serde_json = "1"',
    'sqlx = { version = "0.8", default-features = false, features = ["postgres", "runtime-tokio-rustls", "json"], optional = true }',
    '',
    '[features]',
    'default = []',
    'sqlx = ["dep:sqlx"]',
  ].join('\n')}\n`;
}

function renderRust(contract) {
  const lines = [
    ...generatedNotice('//'),
    'use serde::{Deserialize, Serialize};',
    'use serde_json::Value;',
    '',
  ];

  for (const table of contract.tables) {
    const baseName = table.names?.rust ?? pascal(table.name);
    const tableConst = `${screaming(table.name)}_TABLE`;
    const selectConst = `${screaming(table.name)}_SELECT_SQL`;
    const columnsConst = `${screaming(table.name)}_COLUMNS`;

    lines.push(`pub const ${tableConst}: &str = ${JSON.stringify(physicalName(table))};`);
    lines.push(
      `pub const ${columnsConst}: &[&str] = &[${table.columns.map((column) => JSON.stringify(column.name)).join(', ')}];`,
    );
    lines.push(`pub const ${selectConst}: &str = ${rustRawString(renderSelectSql(table))};`);
    lines.push('');

    for (const column of table.columns.filter((item) => item.kind === 'enum')) {
      lines.push(...renderRustEnum(baseName, column));
      lines.push('');
    }

    lines.push('#[derive(Clone, Debug, Serialize, Deserialize)]');
    lines.push('#[cfg_attr(feature = "sqlx", derive(sqlx::FromRow))]');
    lines.push('#[serde(rename_all = "camelCase")]');
    lines.push(`pub struct ${baseName}Row {`);
    for (const column of table.columns) {
      lines.push(`    pub ${column.name}: ${rustType(column)},`);
    }
    lines.push('}');
    lines.push('');

    lines.push('#[derive(Clone, Debug, Default, Serialize, Deserialize)]');
    lines.push('#[serde(rename_all = "camelCase")]');
    lines.push(`pub struct ${baseName}Insert {`);
    for (const column of table.columns) {
      lines.push(`    pub ${column.name}: ${rustInsertType(column)},`);
    }
    lines.push('}');
    lines.push('');

    const rowValidationLines = renderRustValidationStatements(table, 'value', false);
    const rowValueBinding = rowValidationLines.length > 0 ? 'value' : '_value';
    lines.push(
      `pub fn validate_${table.name}_row(${rowValueBinding}: &${baseName}Row) -> Result<(), String> {`,
    );
    lines.push(...rowValidationLines);
    lines.push('    Ok(())');
    lines.push('}');
    lines.push('');

    const insertValidationLines = renderRustValidationStatements(table, 'value', true);
    const insertValueBinding = insertValidationLines.length > 0 ? 'value' : '_value';
    lines.push(
      `pub fn validate_${table.name}_insert(${insertValueBinding}: &${baseName}Insert) -> Result<(), String> {`,
    );
    lines.push(...insertValidationLines);
    lines.push('    Ok(())');
    lines.push('}');
    lines.push('');
  }

  lines.push(
    'fn validate_string_length(field: &str, value: &str, min: Option<usize>, max: Option<usize>) -> Result<(), String> {',
  );
  lines.push('    let count = value.chars().count();');
  lines.push('    if let Some(min) = min {');
  lines.push('        if count < min {');
  lines.push('            return Err(format!("{field} must be at least {min} characters"));');
  lines.push('        }');
  lines.push('    }');
  lines.push('    if let Some(max) = max {');
  lines.push('        if count > max {');
  lines.push('            return Err(format!("{field} must be at most {max} characters"));');
  lines.push('        }');
  lines.push('    }');
  lines.push('    Ok(())');
  lines.push('}');
  lines.push('');
  lines.push('fn validate_slug(field: &str, value: &str) -> Result<(), String> {');
  lines.push('    let bytes = value.as_bytes();');
  lines.push('    if bytes.len() < 3 || bytes.len() > 120 {');
  lines.push('        return Err(format!("{field} must be 3-120 bytes"));');
  lines.push('    }');
  lines.push(
    '    let Some(first) = bytes.first() else { return Err(format!("{field} is required")); };',
  );
  lines.push(
    '    let Some(last) = bytes.last() else { return Err(format!("{field} is required")); };',
  );
  lines.push('    if !first.is_ascii_lowercase() && !first.is_ascii_digit() {');
  lines.push(
    '        return Err(format!("{field} must start with a lowercase letter or digit"));',
  );
  lines.push('    }');
  lines.push('    if !last.is_ascii_lowercase() && !last.is_ascii_digit() {');
  lines.push('        return Err(format!("{field} must end with a lowercase letter or digit"));');
  lines.push('    }');
  lines.push(
    "    if bytes.iter().any(|byte| !byte.is_ascii_lowercase() && !byte.is_ascii_digit() && *byte != b'-') {",
  );
  lines.push(
    '        return Err(format!("{field} may contain only lowercase letters, digits, and dashes"));',
  );
  lines.push('    }');
  lines.push('    Ok(())');
  lines.push('}');

  return `${lines.join('\n').trimEnd()}\n`;
}

// Postgres enum labels may start with a digit (e.g. the `3mf` model format),
// but Rust identifiers may not. Those get a `V` prefix plus an explicit
// `#[serde(rename)]` so the wire value stays the raw label — the enum's
// `rename_all = "lowercase"` would otherwise serialize the prefixed name.
function rustEnumVariant(value) {
  const name = pascal(value);
  return /^[0-9]/.test(name) ? `V${name}` : name;
}

function renderRustEnum(baseName, column) {
  const enumName = `${baseName}${pascal(column.name)}`;
  const lines = [
    '#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]',
    '#[serde(rename_all = "lowercase")]',
    `pub enum ${enumName} {`,
  ];
  for (const value of column.enumValues) {
    if (rustEnumVariant(value) !== pascal(value)) {
      lines.push(`    #[serde(rename = ${JSON.stringify(value)})]`);
    }
    lines.push(`    ${rustEnumVariant(value)},`);
  }
  lines.push('}');
  lines.push('');
  lines.push(`impl ${enumName} {`);
  lines.push(
    `    pub const VALUES: &'static [&'static str] = &[${column.enumValues.map((value) => JSON.stringify(value)).join(', ')}];`,
  );
  lines.push('');
  lines.push("    pub fn as_str(self) -> &'static str {");
  lines.push('        match self {');
  for (const value of column.enumValues) {
    lines.push(`            Self::${rustEnumVariant(value)} => ${JSON.stringify(value)},`);
  }
  lines.push('        }');
  lines.push('    }');
  lines.push('}');
  lines.push('');
  lines.push(`impl TryFrom<&str> for ${enumName} {`);
  lines.push('    type Error = String;');
  lines.push('');
  lines.push('    fn try_from(value: &str) -> Result<Self, <Self as TryFrom<&str>>::Error> {');
  lines.push('        match value {');
  for (const value of column.enumValues) {
    lines.push(`            ${JSON.stringify(value)} => Ok(Self::${rustEnumVariant(value)}),`);
  }
  lines.push(`            _ => Err(format!("unsupported ${column.name}: {value}")),`);
  lines.push('        }');
  lines.push('    }');
  lines.push('}');
  return lines;
}

function rustType(column) {
  const baseType = rustBaseType(column);
  return column.notNull ? baseType : `Option<${baseType}>`;
}

function rustInsertType(column) {
  return `Option<${rustBaseType(column)}>`;
}

function rustBaseType(column) {
  // sqlx is strict about Postgres OIDs: a `smallint` column must decode into `i16`, not `i32`,
  // and a `real` (float4) column must decode into `f32`, not `f64`.
  if (column.sqlType === 'smallint') {
    return 'i16';
  }
  if (column.sqlType === 'real') {
    return 'f32';
  }
  switch (column.kind) {
    case 'integer':
      return 'i32';
    case 'bigint':
      return 'i64';
    case 'float':
      return 'f64';
    case 'boolean':
      return 'bool';
    case 'jsonObject':
    case 'jsonArray':
      return 'Value';
    default:
      return 'String';
  }
}

function renderRustValidationStatements(table, binding, insertMode) {
  const lines = [];
  for (const column of table.columns) {
    const validation = column.validation ?? {};
    const field = `${binding}.${column.name}`;
    const fieldName = `${table.name}.${column.name}`;
    const validationLines = rustValidationForColumn(column, validation, fieldName, 'value');

    if (insertMode) {
      if (validationLines.length === 0) {
        continue;
      }
      lines.push(`    if let Some(value) = &${field} {`);
      lines.push(...validationLines.map((line) => `        ${line}`));
      lines.push('    }');
      continue;
    }

    if (!column.notNull) {
      if (validationLines.length === 0) {
        continue;
      }
      lines.push(`    if let Some(value) = &${field} {`);
      lines.push(...validationLines.map((line) => `        ${line}`));
      lines.push('    }');
    } else {
      const requiredValidationLines = rustValidationForColumn(
        column,
        validation,
        fieldName,
        `&${field}`,
      );
      lines.push(...requiredValidationLines.map((line) => `    ${line}`));
    }
  }
  return lines;
}

function rustValidationForColumn(column, validation, fieldName, valueExpression) {
  const lines = [];

  if (validation.regex?.startsWith('^[a-z0-9]')) {
    lines.push(`validate_slug(${JSON.stringify(fieldName)}, ${valueExpression})?;`);
  } else if (column.kind === 'string') {
    const min = validation.minLength === undefined ? 'None' : `Some(${validation.minLength})`;
    const max = validation.maxLength === undefined ? 'None' : `Some(${validation.maxLength})`;
    if (validation.minLength !== undefined || validation.maxLength !== undefined) {
      lines.push(
        `validate_string_length(${JSON.stringify(fieldName)}, ${valueExpression}, ${min}, ${max})?;`,
      );
    }
  }

  if (validation.literal) {
    lines.push(
      `if (${valueExpression}).as_str() != ${JSON.stringify(validation.literal)} { return Err(${JSON.stringify(`${fieldName} must use the managed value`)}.to_string()); }`,
    );
  }
  if (validation.maxBytes) {
    lines.push(
      `if (${valueExpression}).as_bytes().len() > ${validation.maxBytes} { return Err(${JSON.stringify(`${fieldName} exceeds ${validation.maxBytes} bytes`)}.to_string()); }`,
    );
  }
  if (column.kind === 'enum') {
    lines.push(
      `if ![${column.enumValues.map((value) => JSON.stringify(value)).join(', ')}].contains(&(${valueExpression}).as_str()) { return Err(format!("unsupported ${fieldName}: {}", ${valueExpression})); }`,
    );
  }
  if (column.kind === 'integer' || column.kind === 'bigint') {
    if (validation.min !== undefined) {
      lines.push(
        `if *(${valueExpression}) < ${validation.min} { return Err(${JSON.stringify(`${fieldName} is below the minimum`)}.to_string()); }`,
      );
    }
    if (validation.max !== undefined) {
      lines.push(
        `if *(${valueExpression}) > ${validation.max} { return Err(${JSON.stringify(`${fieldName} is above the maximum`)}.to_string()); }`,
      );
    }
  }
  if (column.kind === 'jsonObject') {
    lines.push(
      `if !(${valueExpression}).is_object() { return Err(${JSON.stringify(`${fieldName} must be a JSON object`)}.to_string()); }`,
    );
  }
  if (column.kind === 'jsonArray') {
    lines.push(
      `if !(${valueExpression}).is_array() { return Err(${JSON.stringify(`${fieldName} must be a JSON array`)}.to_string()); }`,
    );
  }

  return lines;
}

function renderDieselCargo() {
  return `${[
    '[package]',
    'name = "dd-pg-defs-diesel"',
    'version = "0.1.0"',
    'edition = "2021"',
    '',
    '[dependencies]',
    'chrono = { version = "0.4", features = ["serde"] }',
    // 64-column-tables: music_songs (45 cols) and benefactor_scrape_queries (43) exceed
    // diesel's default 32-column table! limit.
    'diesel = { version = "2", features = ["postgres", "uuid", "serde_json", "chrono", "64-column-tables"] }',
    'serde = { version = "1", features = ["derive"] }',
    'serde_json = "1"',
    'uuid = { version = "1", features = ["serde"] }',
  ].join('\n')}\n`;
}

function renderDieselRust(contract) {
  const lines = [
    ...generatedNotice('//'),
    'use chrono::{DateTime, Utc};',
    'use diesel::prelude::*;',
    'use serde::{Deserialize, Serialize};',
    'use serde_json::Value;',
    'use uuid::Uuid;',
    '',
  ];

  for (const table of contract.tables) {
    const baseName = table.names?.rust ?? pascal(table.name);
    const primaryKeyColumns = table.columns
      .filter((column) => column.primaryKey)
      .map((column) => column.name);
    const primaryKey = primaryKeyColumns.length > 0 ? primaryKeyColumns.join(', ') : table.columns[0]?.name ?? 'id';
    lines.push('diesel::table! {');
    lines.push('    use diesel::sql_types::*;');
    lines.push(`    ${table.name} (${primaryKey}) {`);
    for (const column of table.columns) {
      lines.push(`        ${column.name} -> ${dieselSqlType(column)},`);
    }
    lines.push('    }');
    lines.push('}');
    lines.push('');
    lines.push('#[derive(Clone, Debug, Queryable, Selectable, Serialize, Deserialize)]');
    lines.push(`#[diesel(table_name = ${table.name})]`);
    lines.push(`pub struct ${baseName}DieselRow {`);
    for (const column of table.columns) {
      lines.push(`    pub ${column.name}: ${dieselRustType(column)},`);
    }
    lines.push('}');
    lines.push('');
    lines.push('#[derive(Clone, Debug, Insertable, AsChangeset, Serialize, Deserialize)]');
    lines.push(`#[diesel(table_name = ${table.name})]`);
    lines.push(`pub struct ${baseName}DieselInsert {`);
    for (const column of table.columns.filter((column) => !column.generated)) {
      lines.push(`    pub ${column.name}: ${dieselInsertRustType(column)},`);
    }
    lines.push('}');
    lines.push('');
  }

  return `${lines.join('\n').trimEnd()}\n`;
}

function renderSeaOrmCargo() {
  return `${[
    '[package]',
    'name = "dd-pg-defs-sea-orm"',
    'version = "0.1.0"',
    'edition = "2021"',
    '',
    '[dependencies]',
    'sea-orm = { version = "1", default-features = false, features = ["macros", "with-uuid", "with-json", "with-chrono"] }',
    'serde = { version = "1", features = ["derive"] }',
  ].join('\n')}\n`;
}

function renderSeaOrmRust(contract) {
  const lines = [
    ...generatedNotice('//'),
    'use sea_orm::entity::prelude::*;',
    'use serde::{Deserialize, Serialize};',
    '',
  ];

  for (const table of contract.tables) {
    const baseName = table.names?.rust ?? pascal(table.name);
    const moduleName = table.name;
    lines.push(`pub mod ${moduleName} {`);
    lines.push('    use super::*;');
    lines.push('');
    lines.push('#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]');
    const seaOrmTableAttr =
      table.schema && table.schema !== 'public'
        ? `#[sea_orm(schema_name = ${JSON.stringify(table.schema)}, table_name = ${JSON.stringify(table.name)})]`
        : `#[sea_orm(table_name = ${JSON.stringify(table.name)})]`;
    lines.push(seaOrmTableAttr);
    lines.push('pub struct Model {');
    const primaryKeyCount = table.columns.filter((column) => column.primaryKey).length;
    for (const column of table.columns) {
      const attrs = [];
      if (column.primaryKey) {
        attrs.push('primary_key');
        // Composite keys are never auto-increment; sea-orm defaults integer PKs to
        // auto_increment = true, which mis-models `primary key (a, b)` tables.
        if (column.sqlType === 'uuid' || primaryKeyCount > 1) {
          attrs.push('auto_increment = false');
        }
      }
      if (column.name !== camel(column.name)) {
        attrs.push(`column_name = ${JSON.stringify(column.name)}`);
      }
      if (attrs.length > 0) {
        lines.push(`    #[sea_orm(${attrs.join(', ')})]`);
      }
      lines.push(`    pub ${column.name}: ${seaOrmRustType(column)},`);
    }
    lines.push('}');
    lines.push('');
    lines.push('#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]');
    lines.push('pub enum Relation {}');
    lines.push('');
    lines.push('impl ActiveModelBehavior for ActiveModel {}');
    lines.push('');
    lines.push('}');
    lines.push('');
    lines.push(`pub use ${moduleName}::Entity as ${baseName}Entity;`);
    lines.push(`pub use ${moduleName}::Model as ${baseName}Model;`);
    lines.push('');
  }

  return `${lines.join('\n').trimEnd()}\n`;
}

function dieselSqlType(column) {
  const nullable = !column.notNull;
  let baseType;
  switch (column.sqlType) {
    case 'uuid':
      baseType = 'Uuid';
      break;
    case 'varchar':
      baseType = 'Varchar';
      break;
    case 'text':
      baseType = 'Text';
      break;
    case 'smallint':
      baseType = 'Int2';
      break;
    case 'integer':
      baseType = 'Int4';
      break;
    case 'bigint':
    case 'bigserial':
      baseType = 'Int8';
      break;
    case 'double':
      baseType = 'Double';
      break;
    case 'real':
      baseType = 'Float';
      break;
    case 'boolean':
      baseType = 'Bool';
      break;
    case 'jsonb':
      baseType = 'Jsonb';
      break;
    case 'timestamptz':
      baseType = 'Timestamptz';
      break;
    default:
      baseType = 'Text';
      break;
  }
  return nullable ? `Nullable<${baseType}>` : baseType;
}

function dieselRustType(column) {
  const baseType = rustDbType(column);
  return column.notNull ? baseType : `Option<${baseType}>`;
}

function dieselInsertRustType(column) {
  return `Option<${rustDbType(column)}>`;
}

function seaOrmRustType(column) {
  // SeaORM infers the column type from the Rust field type; i16 maps to SMALLINT and
  // f32 maps to REAL (float4).
  if (column.sqlType === 'smallint') {
    return column.notNull ? 'i16' : 'Option<i16>';
  }
  if (column.sqlType === 'real') {
    return column.notNull ? 'f32' : 'Option<f32>';
  }
  let baseType;
  switch (column.kind) {
    case 'uuid':
      baseType = 'Uuid';
      break;
    case 'integer':
      baseType = 'i32';
      break;
    case 'bigint':
      baseType = 'i64';
      break;
    case 'float':
      baseType = 'f64';
      break;
    case 'boolean':
      baseType = 'bool';
      break;
    case 'jsonObject':
    case 'jsonArray':
      baseType = 'Json';
      break;
    case 'timestamp':
      baseType = 'DateTimeWithTimeZone';
      break;
    default:
      baseType = 'String';
      break;
  }
  return column.notNull ? baseType : `Option<${baseType}>`;
}

function rustDbType(column) {
  // Diesel's Int2 SQL type binds to i16 and Float (float4) binds to f32; keep the Rust struct
  // field in lockstep with dieselSqlType.
  if (column.sqlType === 'smallint') {
    return 'i16';
  }
  if (column.sqlType === 'real') {
    return 'f32';
  }
  switch (column.kind) {
    case 'uuid':
      return 'Uuid';
    case 'integer':
      return 'i32';
    case 'bigint':
      return 'i64';
    case 'float':
      return 'f64';
    case 'boolean':
      return 'bool';
    case 'jsonObject':
    case 'jsonArray':
      return 'Value';
    case 'timestamp':
      return 'DateTime<Utc>';
    default:
      return 'String';
  }
}

function renderGleamToml() {
  // gleeunit is needed so `gleam test` can run the smoke tests under
  // `generated/gleam/test/`. Without this block the dev tooling has to
  // re-add gleeunit on every regen, which silently breaks CI's
  // `gleam-pg-defs-wiring` check the next time someone runs
  // `node remote/libs/pg-defs/src/generate.mjs`.
  return `${[
    'name = "dd_pg_defs"',
    'version = "0.1.0"',
    'target = "erlang"',
    '',
    '[dependencies]',
    'gleam_stdlib = ">= 0.68.0 and < 2.0.0"',
    '',
    '[dev-dependencies]',
    'gleeunit = ">= 1.0.0 and < 2.0.0"',
  ].join('\n')}\n`;
}

function renderGleam(contract) {
  const lines = [
    ...generatedNotice('////'),
    'import gleam/list',
    'import gleam/option.{type Option}',
    'import gleam/string',
    '',
  ];

  for (const table of contract.tables) {
    const baseName = table.names?.gleam ?? pascal(table.name);
    lines.push(`pub const ${table.name}_table = ${JSON.stringify(physicalName(table))}`);
    lines.push(
      `pub const ${table.name}_select_sql = ${JSON.stringify(renderSelectSql(table, { jsonAsText: true }))}`,
    );
    lines.push('');

    for (const column of table.columns.filter((item) => item.kind === 'enum')) {
      const typeName = `${baseName}${pascal(column.name)}`;
      lines.push(`pub type ${typeName} {`);
      for (const value of column.enumValues) {
        lines.push(`  ${gleamEnumVariantName(typeName, value)}`);
      }
      lines.push('}');
      lines.push('');
      lines.push(`pub fn ${table.name}_${column.name}_to_string(value: ${typeName}) -> String {`);
      lines.push('  case value {');
      for (const value of column.enumValues) {
        lines.push(`    ${gleamEnumVariantName(typeName, value)} -> ${JSON.stringify(value)}`);
      }
      lines.push('  }');
      lines.push('}');
      lines.push('');
      lines.push(
        `pub fn parse_${table.name}_${column.name}(value: String) -> Result(${typeName}, String) {`,
      );
      lines.push('  case value {');
      for (const value of column.enumValues) {
        lines.push(`    ${JSON.stringify(value)} -> Ok(${gleamEnumVariantName(typeName, value)})`);
      }
      lines.push(`    _ -> Error("unsupported ${table.name}.${column.name}: " <> value)`);
      lines.push('  }');
      lines.push('}');
      lines.push('');
    }

    lines.push(`pub type ${baseName}Row {`);
    lines.push(`  ${baseName}Row(`);
    for (const column of table.columns) {
      lines.push(`    ${gleamFieldName(column)}: ${gleamType(column)},`);
    }
    lines.push('  )');
    lines.push('}');
    lines.push('');

    lines.push(`pub fn validate_${table.name}_slug(value: String) -> Result(String, String) {`);
    lines.push('  let length = string.length(value)');
    lines.push('  case length >= 3 && length <= 120 && is_slug_text(value) {');
    lines.push('    True -> Ok(value)');
    lines.push(
      `    False -> Error("${table.name}.slug must be a lowercase slug 3-120 characters long")`,
    );
    lines.push('  }');
    lines.push('}');
    lines.push('');

    for (const column of table.columns.filter((item) => item.kind === 'enum')) {
      lines.push(
        `pub fn validate_${table.name}_${column.name}(value: String) -> Result(String, String) {`,
      );
      lines.push(
        `  case list.contains([${column.enumValues.map((value) => JSON.stringify(value)).join(', ')}], value) {`,
      );
      lines.push('    True -> Ok(value)');
      lines.push(`    False -> Error("unsupported ${table.name}.${column.name}: " <> value)`);
      lines.push('  }');
      lines.push('}');
      lines.push('');
    }
  }

  lines.push('fn is_slug_text(value: String) -> Bool {');
  lines.push('  let chars = string.to_graphemes(value)');
  lines.push('  case chars {');
  lines.push('    [] -> False');
  lines.push('    [first, ..rest] -> {');
  lines.push('      let last_ok = case list.last(chars) {');
  lines.push('        Ok(last) -> is_slug_edge(last)');
  lines.push('        Error(_) -> False');
  lines.push('      }');
  lines.push(
    '      is_slug_edge(first) && list.all(rest, fn(item) { is_slug_char(item) }) && last_ok',
  );
  lines.push('    }');
  lines.push('  }');
  lines.push('}');
  lines.push('');
  lines.push('fn is_slug_edge(value: String) -> Bool {');
  lines.push('  is_lower_ascii(value) || is_digit(value)');
  lines.push('}');
  lines.push('');
  lines.push('fn is_slug_char(value: String) -> Bool {');
  lines.push('  is_slug_edge(value) || value == "-"');
  lines.push('}');
  lines.push('');
  lines.push('fn is_lower_ascii(value: String) -> Bool {');
  lines.push(
    '  list.contains(["a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k", "l", "m", "n", "o", "p", "q", "r", "s", "t", "u", "v", "w", "x", "y", "z"], value)',
  );
  lines.push('}');
  lines.push('');
  lines.push('fn is_digit(value: String) -> Bool {');
  lines.push('  list.contains(["0", "1", "2", "3", "4", "5", "6", "7", "8", "9"], value)');
  lines.push('}');

  return `${lines.join('\n').trimEnd()}\n`;
}

function gleamEnumVariantName(typeName, value) {
  return `${typeName}${pascal(value)}`;
}

function gleamType(column) {
  let baseType;
  switch (column.kind) {
    case 'integer':
    case 'bigint':
      baseType = 'Int';
      break;
    case 'float':
      baseType = 'Float';
      break;
    case 'boolean':
      baseType = 'Bool';
      break;
    default:
      baseType = 'String';
      break;
  }
  return column.notNull ? baseType : `Option(${baseType})`;
}

function gleamFieldName(column) {
  if (column.kind === 'jsonObject' || column.kind === 'jsonArray') {
    return `${column.name}_json`;
  }
  return column.name;
}

function renderErlang(contract) {
  const exports = [];
  const functions = [];

  for (const table of contract.tables) {
    exports.push(`${table.name}_table/0`);
    exports.push(`${table.name}_columns/0`);
    exports.push(`${table.name}_select_sql/0`);
    functions.push(`${table.name}_table() -> ${erlangBinary(physicalName(table))}.`);
    functions.push('');
    functions.push(
      `${table.name}_columns() -> [${table.columns.map((column) => erlangBinary(column.name)).join(', ')}].`,
    );
    functions.push('');
    functions.push(
      `${table.name}_select_sql() -> ${erlangBinary(renderSelectSql(table, { jsonAsText: true }))}.`,
    );
    functions.push('');

    for (const column of table.columns.filter((item) => item.kind === 'enum')) {
      exports.push(`${table.name}_${column.name}_values/0`);
      exports.push(`validate_${table.name}_${column.name}/1`);
      functions.push(
        `${table.name}_${column.name}_values() -> [${column.enumValues.map(erlangBinary).join(', ')}].`,
      );
      functions.push('');
      functions.push(`validate_${table.name}_${column.name}(Value) when is_binary(Value) ->`);
      functions.push(`    case lists:member(Value, ${table.name}_${column.name}_values()) of`);
      functions.push('        true -> ok;');
      functions.push(
        `        false -> {error, <<\"unsupported ${table.name}.${column.name}: \"/binary, Value/binary>>}`,
      );
      functions.push('    end;');
      functions.push(`validate_${table.name}_${column.name}(Value) when is_list(Value) ->`);
      functions.push(
        `    validate_${table.name}_${column.name}(unicode:characters_to_binary(Value)).`,
      );
      functions.push('');
    }
  }

  return `${[
    ...generatedNotice('%'),
    '-module(pg_defs).',
    `-export([${exports.join(', ')}]).`,
    '',
    ...functions,
  ]
    .join('\n')
    .trimEnd()}\n`;
}

function renderMnesiaErlang(contract) {
  // Mnesia stores rows as records keyed by the first attribute. We expose:
  //   * a record(...) declaration per table
  //   * an `<table>_attributes/0` helper
  //   * an `<table>_table_def/0` helper that returns the property list expected by
  //     `mnesia:create_table/2` (attributes, type=set, disc_copies on the calling node).
  // Consumers should call the table_def helpers when bootstrapping schemas, and they
  // remain free to override copy semantics (ram_copies, disc_only_copies, fragments) at
  // runtime — we only encode safe defaults here so a downstream module that forgets to
  // pass copies still gets a valid table definition.
  const exports = [];
  const records = [];
  const functions = [];

  for (const table of contract.tables) {
    const attributeList = table.columns
      .map((column) => column.name)
      .join(', ');
    records.push(`-record(${table.name}, {${attributeList}}).`);
    exports.push(`${table.name}_attributes/0`);
    exports.push(`${table.name}_table_def/0`);
    exports.push(`${table.name}_record_info/0`);
    functions.push(
      `${table.name}_attributes() -> [${table.columns.map((column) => `'${column.name}'`).join(', ')}].`,
    );
    functions.push('');
    functions.push(`${table.name}_record_info() ->`);
    functions.push(`    {${table.name}, ${table.columns.length}, ${table.name}_attributes()}.`);
    functions.push('');
    functions.push(`${table.name}_table_def() ->`);
    functions.push('    [');
    functions.push(`        {attributes, ${table.name}_attributes()},`);
    functions.push('        {type, set},');
    functions.push(`        {record_name, ${table.name}},`);
    functions.push('        {disc_copies, [node()]}');
    functions.push('    ].');
    functions.push('');
  }

  exports.push('all_table_defs/0');
  functions.push('all_table_defs() ->');
  functions.push(
    `    [${contract.tables.map((table) => `{${table.name}, ${table.name}_table_def()}`).join(', ')}].`,
  );

  return `${[
    ...generatedNotice('%'),
    '-module(pg_defs_mnesia).',
    `-export([${exports.join(', ')}]).`,
    '',
    ...records,
    '',
    ...functions,
  ]
    .join('\n')
    .trimEnd()}\n`;
}

function renderEctoMixExs() {
  return `${[
    '# Generated by @dd/pg-defs. Do not edit by hand.',
    '# SOURCE OF TRUTH: schema/schema.sql defines the database contract.',
    '',
    'defmodule DdPgDefs.MixProject do',
    '  use Mix.Project',
    '',
    '  def project do',
    '    [',
    '      app: :dd_pg_defs,',
    '      version: "0.1.0",',
    '      elixir: "~> 1.16",',
    '      start_permanent: Mix.env() == :prod,',
    '      deps: deps()',
    '    ]',
    '  end',
    '',
    '  def application do',
    '    [extra_applications: [:logger]]',
    '  end',
    '',
    '  defp deps do',
    '    [',
    '      {:ecto, "~> 3.11"},',
    '      {:ecto_sql, "~> 3.11"},',
    '      {:postgrex, "~> 0.18"},',
    '      {:jason, "~> 1.4"}',
    '    ]',
    '  end',
    'end',
  ].join('\n')}\n`;
}

function renderEctoIndex(contract) {
  // The umbrella module re-exports the per-table schema modules and exposes a `tables/0`
  // helper so consumers can iterate every canonical table without grepping the codebase.
  const lines = [
    ...generatedNotice('#'),
    '',
    'defmodule DdPgDefs do',
    '  @moduledoc """',
    '  Canonical Ecto adapters for the remote Postgres schema. The SQL file at',
    '  `remote/libs/pg-defs/schema/schema.sql` is the source of truth; these schemas',
    '  are generated and must not be edited by hand.',
    '  """',
    '',
    '  @tables [',
  ];
  for (const table of contract.tables) {
    lines.push(`    ${ectoModuleName(table)},`);
  }
  lines.push('  ]');
  lines.push('');
  lines.push('  @spec tables() :: [module()]');
  lines.push('  def tables, do: @tables');
  lines.push('end');
  return `${lines.join('\n').trimEnd()}\n`;
}

function renderEctoSchemaFiles(contract) {
  return contract.tables.map((table) => [
    `generated/elixir/lib/dd_pg_defs/${table.name}.ex`,
    renderEctoSchemaFile(table),
  ]);
}

function renderEctoSchemaFile(table) {
  // Each table gets its own Ecto.Schema module under `DdPgDefs.<TableModule>` with a generated
  // `changeset/2` that re-applies every constraint we can statically derive from the SQL
  // (required fields, length limits, regex patterns, enum membership, byte limits).
  const moduleName = ectoModuleName(table);
  const lines = [
    ...generatedNotice('#'),
    '',
    `defmodule ${moduleName} do`,
    '  use Ecto.Schema',
    '  import Ecto.Changeset',
    '',
    `  @table ${JSON.stringify(table.name)}`,
    ...(table.schema && table.schema !== 'public'
      ? [`  @schema_prefix ${JSON.stringify(table.schema)}`]
      : []),
    '',
  ];

  // Determine primary key configuration
  const pkColumns = table.columns.filter((column) => column.primaryKey);
  const pkColumn = pkColumns[0];
  const isUuidPk = pkColumn?.sqlType === 'uuid';
  const isBigSerialPk = pkColumn?.sqlType === 'bigserial';
  if (isUuidPk) {
    lines.push('  @primary_key {:id, :binary_id, autogenerate: true}');
    lines.push('  @foreign_key_type :binary_id');
  } else if (isBigSerialPk) {
    lines.push('  @primary_key {:id, :id, autogenerate: true}');
  } else {
    lines.push('  @primary_key false');
  }
  lines.push('');

  // Field declarations
  lines.push('  schema @table do');
  for (const column of table.columns) {
    if (column.primaryKey) {
      continue;
    }
    if (column.name === 'created_at' || column.name === 'updated_at') {
      // Handled via timestamps/1 macro to keep the Ecto idiom intact
      continue;
    }
    lines.push(`    ${ectoFieldLine(column)}`);
  }
  if (
    table.columns.some((column) => column.name === 'created_at')
    && table.columns.some((column) => column.name === 'updated_at')
  ) {
    lines.push('    timestamps(inserted_at: :created_at, type: :utc_datetime_usec)');
  }
  lines.push('  end');
  lines.push('');

  const requiredFields = ectoRequiredFields(table);
  const optionalFields = ectoOptionalFields(table);

  lines.push('  @required_fields ~w(' + requiredFields.join(' ') + ')a');
  lines.push('  @optional_fields ~w(' + optionalFields.join(' ') + ')a');
  lines.push('');

  lines.push('  @doc "Builds an Ecto changeset enforcing every constraint exposed in schema.sql."');
  lines.push('  def changeset(struct, attrs) do');
  lines.push('    struct');
  lines.push('    |> cast(attrs, @required_fields ++ @optional_fields)');
  if (requiredFields.length > 0) {
    lines.push('    |> validate_required(@required_fields)');
  }
  for (const validation of ectoValidations(table)) {
    lines.push(`    ${validation}`);
  }
  lines.push('  end');
  lines.push('end');
  return `${lines.join('\n').trimEnd()}\n`;
}

function ectoModuleName(table) {
  return `DdPgDefs.${pascal(table.name)}`;
}

function ectoFieldLine(column) {
  const fieldName = column.name;
  const fieldType = ectoFieldType(column);
  const options = [];
  if (column.defaultValue !== undefined) {
    options.push(`default: ${ectoLiteral(column.defaultValue)}`);
  }
  if (column.kind === 'enum') {
    options.push('default: ' + ectoLiteral(column.defaultValue));
  }
  // Deduplicate (enum + default already pushed)
  const seen = new Set();
  const finalOptions = options.filter((item) => {
    if (seen.has(item)) {
      return false;
    }
    seen.add(item);
    return true;
  });
  const optionsSql = finalOptions.length > 0 ? `, ${finalOptions.join(', ')}` : '';
  return `field :${fieldName}, ${fieldType}${optionsSql}`;
}

function ectoFieldType(column) {
  switch (column.kind) {
    case 'uuid':
      return ':binary_id';
    case 'integer':
      return ':integer';
    case 'bigint':
      return ':integer';
    case 'float':
      return ':float';
    case 'boolean':
      return ':boolean';
    case 'timestamp':
      return ':utc_datetime_usec';
    case 'jsonObject':
      return ':map';
    case 'jsonArray':
      return '{:array, :map}';
    default:
      return ':string';
  }
}

function ectoLiteral(value) {
  if (value === undefined || value === null) {
    return 'nil';
  }
  if (typeof value === 'boolean') {
    return value ? 'true' : 'false';
  }
  if (typeof value === 'number') {
    return String(value);
  }
  if (Array.isArray(value)) {
    return '[]';
  }
  if (typeof value === 'object') {
    return '%{}';
  }
  return JSON.stringify(value);
}

function ectoRequiredFields(table) {
  const fields = [];
  for (const column of table.columns) {
    if (column.primaryKey) {
      continue;
    }
    if (column.name === 'created_at' || column.name === 'updated_at') {
      continue;
    }
    if (column.notNull && column.defaultSql === undefined && !column.generated) {
      fields.push(column.name);
    }
  }
  return fields;
}

function ectoOptionalFields(table) {
  const fields = [];
  for (const column of table.columns) {
    if (column.primaryKey) {
      continue;
    }
    if (column.name === 'created_at' || column.name === 'updated_at') {
      continue;
    }
    if (!(column.notNull && column.defaultSql === undefined && !column.generated)) {
      fields.push(column.name);
    }
  }
  return fields;
}

function ectoValidations(table) {
  const lines = [];
  for (const column of table.columns) {
    if (column.primaryKey) {
      continue;
    }
    if (column.name === 'created_at' || column.name === 'updated_at') {
      continue;
    }
    const validation = column.validation ?? {};
    if (column.kind === 'enum' && Array.isArray(column.enumValues)) {
      const values = column.enumValues.map((value) => `"${value}"`).join(', ');
      lines.push(`|> validate_inclusion(:${column.name}, [${values}])`);
    }
    if (validation.regex) {
      lines.push(
        `|> validate_format(:${column.name}, ~r/${ectoRegexBody(validation.regex)}/)`,
      );
    }
    if (validation.literal) {
      lines.push(
        `|> validate_inclusion(:${column.name}, [${JSON.stringify(validation.literal)}])`,
      );
    }
    if (column.kind === 'string') {
      const opts = [];
      if (validation.minLength !== undefined) {
        opts.push(`min: ${validation.minLength}`);
      }
      if (validation.maxLength !== undefined) {
        opts.push(`max: ${validation.maxLength}`);
      }
      if (opts.length > 0) {
        lines.push(`|> validate_length(:${column.name}, ${opts.join(', ')})`);
      }
    }
    if (column.kind === 'integer' || column.kind === 'bigint') {
      const opts = [];
      if (validation.min !== undefined) {
        opts.push(`greater_than_or_equal_to: ${validation.min}`);
      }
      if (validation.max !== undefined) {
        opts.push(`less_than_or_equal_to: ${validation.max}`);
      }
      if (opts.length > 0) {
        lines.push(`|> validate_number(:${column.name}, ${opts.join(', ')})`);
      }
    }
  }
  return lines;
}

function ectoRegexBody(pattern) {
  // Elixir sigil regex `~r/.../` requires escaping forward slashes. Keep other characters intact
  // because the source patterns are PCRE/POSIX-ERE compatible with Elixir's regex engine.
  return pattern.replace(/\//g, '\\/');
}

function renderGoEntMod() {
  return `${[
    'module dd-pg-defs-ent',
    '',
    'go 1.23',
    '',
    'require (',
    '\tentgo.io/ent v0.14.1',
    '\tgithub.com/google/uuid v1.6.0',
    ')',
  ].join('\n')}\n`;
}

function renderEntDocGo() {
  return `${[
    ...generatedNotice('//'),
    'package schema',
    '',
    'import "entgo.io/ent/schema"',
    '',
    '// Run `go generate ./ent` from a parent module to regenerate the ent client from these schemas.',
    '// Each entity below mirrors a single table from schema/schema.sql; constraints that ent cannot',
    '// fully express (partial indexes, JSONB defaults, custom CHECKs) live in the SQL contract and',
    '// are enforced by the database.',
    '',
    '// entAnnotation attaches the SQL table name without forcing a dependency on the entsql package',
    '// at codegen time. Defined in this file (rather than each schema file) so the package compiles',
    '// even when consumers add or drop tables.',
    'type entAnnotation struct {',
    '\tTable string',
    '}',
    '',
    'func (entAnnotation) Name() string { return "EntSQL" }',
    '',
    "// Compile-time guarantee that entAnnotation satisfies schema.Annotation.",
    'var _ schema.Annotation = (*entAnnotation)(nil)',
  ].join('\n')}\n`;
}

function renderEntSchemaFiles(contract) {
  return contract.tables.map((table) => [
    `generated/go/ent/schema/${entFileName(table)}.go`,
    renderEntSchemaFile(table),
  ]);
}

function entFileName(table) {
  // ent's idiomatic file name matches the schema struct in lowercase, so we prefer the singular
  // class name when metadata supplies one (e.g. `agentremotedevtask.go` ↔ `AgentRemoteDevTask`).
  // Without metadata, we fall back to a stripped table name.
  const className = table.names?.rust;
  if (className) {
    return className.toLowerCase();
  }
  return table.name.replace(/_/g, '');
}

function renderEntSchemaFile(table) {
  const className = table.names?.rust ?? pascal(table.name);
  const fieldLines = [];
  for (const column of table.columns) {
    fieldLines.push(...entFieldLines(column));
  }

  const indexLines = [];
  let hasUsableIndex = false;
  for (const tableIndex of table.indexes ?? []) {
    if (tableIndex.method || tableIndex.where) {
      indexLines.push(
        `\t\t// ${tableIndex.name} lives in schema.sql because ent cannot model ${tableIndex.method ? `${tableIndex.method} ` : ''}${tableIndex.where ? 'partial ' : ''}indexes.`,
      );
      continue;
    }
    const fields = tableIndex.columns
      .map((column) => JSON.stringify(typeof column === 'string' ? column : column.name))
      .join(', ');
    const builder = `index.Fields(${fields})`;
    indexLines.push(`\t\t${tableIndex.unique ? `${builder}.Unique(),` : `${builder},`}`);
    hasUsableIndex = true;
  }

  const usesRegexp = fieldLines.some((line) => line.includes('regexp.MustCompile'));
  const usesUuid = table.columns.some((column) => column.kind === 'uuid');
  const usesIndex = hasUsableIndex;

  // Stdlib imports (sorted) come first, then third-party. We omit "time" because field.Time uses
  // an internal type and the column-level metadata never needs `time.Time` literals here.
  const stdlibImports = [];
  if (usesRegexp) {
    stdlibImports.push('\t"regexp"');
  }

  const thirdPartyImports = [
    '\t"entgo.io/ent"',
    '\t"entgo.io/ent/schema"',
    '\t"entgo.io/ent/schema/field"',
  ];
  if (usesIndex) {
    thirdPartyImports.push('\t"entgo.io/ent/schema/index"');
  }
  if (usesUuid) {
    thirdPartyImports.push('\t"github.com/google/uuid"');
  }

  const importLines = stdlibImports.length > 0
    ? [...stdlibImports, '', ...thirdPartyImports]
    : thirdPartyImports;

  const lines = [
    ...generatedNotice('//'),
    'package schema',
    '',
    'import (',
    ...importLines,
    ')',
    '',
    `// ${className} mirrors the canonical ${table.name} table.`,
    `type ${className} struct {`,
    '\tent.Schema',
    '}',
    '',
    `func (${className}) Annotations() []schema.Annotation {`,
    '\treturn []schema.Annotation{',
    `\t\t&entAnnotation{Table: ${JSON.stringify(table.name)}},`,
    '\t}',
    '}',
    '',
    `func (${className}) Fields() []ent.Field {`,
    '\treturn []ent.Field{',
    ...fieldLines,
    '\t}',
    '}',
    '',
    `func (${className}) Indexes() []ent.Index {`,
    '\treturn []ent.Index{',
    ...indexLines,
    '\t}',
    '}',
  ];

  return `${lines.join('\n').trimEnd()}\n`;
}

function entFieldLines(column) {
  const fieldName = JSON.stringify(column.name);
  let line = `\t\t${entFieldBuilder(column, fieldName)}`;
  const validation = column.validation ?? {};
  if (column.kind === 'string') {
    if (validation.minLength !== undefined) {
      line += `.MinLen(${validation.minLength})`;
    }
    if (validation.maxLength !== undefined) {
      line += `.MaxLen(${validation.maxLength})`;
    }
    if (validation.regex) {
      line += `.Match(regexp.MustCompile(${goRawString(validation.regex)}))`;
    }
  }
  if (column.kind === 'integer' || column.kind === 'bigint' || column.kind === 'float') {
    if (validation.min !== undefined) {
      line += `.Min(${validation.min})`;
    }
    if (validation.max !== undefined) {
      line += `.Max(${validation.max})`;
    }
  }
  if (column.kind === 'enum' && Array.isArray(column.enumValues)) {
    const allowed = column.enumValues.map((value) => JSON.stringify(value)).join(', ');
    line += `.Values(${allowed})`;
  }
  if (!column.notNull) {
    line += '.Optional().Nillable()';
  }
  if (column.defaultValue !== undefined && !column.generated) {
    if (column.kind === 'string' && typeof column.defaultValue === 'string') {
      line += `.Default(${JSON.stringify(column.defaultValue)})`;
    } else if (column.kind === 'integer' || column.kind === 'bigint' || column.kind === 'float') {
      line += `.Default(${column.defaultValue})`;
    } else if (column.kind === 'boolean') {
      line += `.Default(${column.defaultValue ? 'true' : 'false'})`;
    }
  }
  line += `.StorageKey(${fieldName}),`;
  return [line];
}

function entFieldBuilder(column, fieldName) {
  switch (column.kind) {
    case 'uuid':
      return `field.UUID(${fieldName}, uuid.UUID{})`;
    case 'integer':
      return `field.Int32(${fieldName})`;
    case 'bigint':
      return `field.Int64(${fieldName})`;
    case 'float':
      return column.sqlType === 'real' ? `field.Float32(${fieldName})` : `field.Float(${fieldName})`;
    case 'boolean':
      return `field.Bool(${fieldName})`;
    case 'timestamp':
      return `field.Time(${fieldName})`;
    case 'jsonObject':
      return `field.JSON(${fieldName}, map[string]interface{}{})`;
    case 'jsonArray':
      return `field.JSON(${fieldName}, []interface{}{})`;
    case 'enum':
      return `field.Enum(${fieldName})`;
    default:
      return `field.String(${fieldName})`;
  }
}

function renderSqlcYaml() {
  return `${[
    '# Generated by @dd/pg-defs. Do not edit by hand.',
    '# SOURCE OF TRUTH: ../schema/schema.sql defines the database contract.',
    'version: "2"',
    'sql:',
    '  - schema: "schema.sql"',
    '    queries: "query.sql"',
    '    engine: "postgresql"',
    '    gen:',
    '      go:',
    '        package: "pgdefs"',
    '        out: "."',
    '        sql_package: "pgx/v5"',
    '        emit_json_tags: true',
    '        emit_prepared_queries: false',
    '        emit_pointers_for_null_types: true',
  ].join('\n')}\n`;
}

function renderSqlcSchemaSql(sourceSql) {
  // sqlc reads schema.sql as the DDL source. We mirror schema/schema.sql verbatim so that
  // running `sqlc generate` against this directory produces types that line up with what the
  // database actually enforces. NEVER apply this file directly — it is only for code generation.
  const trimmed = sourceSql.endsWith('\n') ? sourceSql : `${sourceSql}\n`;
  return `-- Generated by @dd/pg-defs. Do not edit by hand.\n-- SOURCE OF TRUTH: ../schema/schema.sql defines the database contract.\n-- WARNING: do NOT apply this file directly to any database. It exists only so that\n-- sqlc can introspect the schema for type generation. Apply migrations through the\n-- pg-defs diff workflow with explicit human review.\n\n${trimmed}`;
}

function renderSqlcQuerySql(contract) {
  // Provide a minimal, dependency-free starter query catalogue per table so consumers can run
  // `sqlc generate` immediately. Custom queries should live in the consuming service rather than
  // inside this generated file.
  const blocks = [
    '-- Generated by @dd/pg-defs. Do not edit by hand.',
    '-- SOURCE OF TRUTH: schema/schema.sql defines the database contract.',
    '',
  ];
  for (const table of contract.tables) {
    const baseName = pascal(table.name);
    const columnList = table.columns.map((column) => column.name).join(', ');
    const placeholders = table.columns.map((_, index) => `$${index + 1}`).join(', ');
    const updatableColumns = table.columns.filter(
      (column) => !column.primaryKey && column.name !== 'created_at',
    );
    const updateAssignments = updatableColumns
      .map((column, index) => `${column.name} = $${index + 2}`)
      .join(', ');

    // Use the schema-qualified physical name so generated sqlc queries target the correct schema
    // (e.g. benefactor.benefactor_leads) instead of resolving against the default search_path.
    const fromName = physicalName(table);
    blocks.push(`-- name: List${baseName} :many`);
    blocks.push(`select ${columnList} from ${fromName};`);
    blocks.push('');
    blocks.push(`-- name: Get${baseName} :one`);
    const idColumn = table.columns.find((column) => column.primaryKey)?.name ?? 'id';
    blocks.push(`select ${columnList} from ${fromName} where ${idColumn} = $1 limit 1;`);
    blocks.push('');
    blocks.push(`-- name: Create${baseName} :one`);
    blocks.push(
      `insert into ${fromName} (${columnList}) values (${placeholders}) returning ${columnList};`,
    );
    blocks.push('');
    if (updatableColumns.length > 0) {
      blocks.push(`-- name: Update${baseName} :one`);
      blocks.push(
        `update ${fromName} set ${updateAssignments} where ${idColumn} = $1 returning ${columnList};`,
      );
      blocks.push('');
    }
    blocks.push(`-- name: Delete${baseName} :exec`);
    blocks.push(`delete from ${fromName} where ${idColumn} = $1;`);
    blocks.push('');
  }
  return `${blocks.join('\n').trimEnd()}\n`;
}

function renderSqlcReadme() {
  return `# Generated sqlc adapter

This directory is a self-contained sqlc workspace. Run \`sqlc generate\` from inside this folder
to produce the typed Go bindings; the canonical DDL is mirrored from
\`remote/libs/pg-defs/schema/schema.sql\`. The query catalogue in \`query.sql\` is a starter set of
list/get/create/update/delete queries — extend it inside your service rather than here.

> Never apply \`schema.sql\` from this directory to a real database; this copy exists solely so that
> \`sqlc\` can introspect the schema offline. Use the pg-defs diff workflow for migrations.
`;
}

function renderJvmReadme() {
  return `# Generated JVM adapters

Two flavors live here:

- \`jooq/\` — A jOOQ \`Tables.java\` that can be referenced from any JVM stack (plain Java, Spring
  Boot, Vert.x, Micronaut, Scala via Java interop, Kotlin, etc.). The build script wires up the
  jOOQ runtime dependency so you can run-time \`DSL.using(...)\` immediately, and serves as a
  starting point for full \`jooq-codegen\` if you want everything — column-level constants live in
  the generated \`Tables.java\` already.
- \`hibernate/\` — One JPA-annotated entity class per canonical table. Drop these into a Spring Boot
  \`@Repository\`, a Vert.x Hibernate Reactive verticle, or any plain JPA app. Constraints that JPA
  cannot natively express (partial indexes, GIN indexes, JSONB CHECKs) are intentionally left to the
  database; this package never owns migrations.

Both directories ship a Gradle build file. Translating to Maven or sbt is mechanical: declare the
same jOOQ / Hibernate / Jakarta Persistence dependencies and point your build at
\`src/main/java\`.
`;
}

function renderJooqBuildGradle() {
  return `${[
    '// Generated by @dd/pg-defs. Do not edit by hand.',
    '// SOURCE OF TRUTH: ../../schema/schema.sql defines the database contract.',
    'plugins {',
    "    id 'java-library'",
    '}',
    '',
    "group = 'dd.pgdefs'",
    "version = '0.1.0'",
    '',
    'java {',
    '    sourceCompatibility = JavaVersion.VERSION_17',
    '    targetCompatibility = JavaVersion.VERSION_17',
    '}',
    '',
    'repositories {',
    '    mavenCentral()',
    '}',
    '',
    'dependencies {',
    "    api 'org.jooq:jooq:3.19.10'",
    "    implementation 'org.postgresql:postgresql:42.7.4'",
    '}',
  ].join('\n')}\n`;
}

function renderJooqTablesJava(contract) {
  const lines = [
    '// Generated by @dd/pg-defs. Do not edit by hand.',
    '// SOURCE OF TRUTH: schema/schema.sql defines the database contract.',
    '// Generated ORM/client code is an adapter only; do not infer migrations from it.',
    '// MIGRATION SAFETY: never run or apply migrations automatically. Require explicit human review',
    '// and approval before any database write.',
    'package dd.pgdefs.jooq;',
    '',
    'import java.time.OffsetDateTime;',
    'import java.util.UUID;',
    'import org.jooq.Field;',
    'import org.jooq.JSONB;',
    'import org.jooq.Name;',
    'import org.jooq.Table;',
    'import org.jooq.impl.DSL;',
    'import org.jooq.impl.SQLDataType;',
    '',
    '/**',
    ' * jOOQ table + column references for every canonical pg-defs table.',
    ' * <p>',
    ' * Use {@code DSL.using(connection).select(Tables.APP_CONFIG_ID).from(Tables.APP_CONFIG).fetch()}.',
    ' * Run-time jOOQ avoids the codegen Gradle plugin so this file is enough for read-side queries.',
    ' */',
    'public final class Tables {',
    '    private Tables() {',
    '    }',
    '',
  ];

  for (const table of contract.tables) {
    const tableConst = screaming(table.name);
    // DSL.name() takes the qualified name parts as varargs, so a non-public schema prepends the
    // schema segment (e.g. DSL.name("benefactor", "benefactor_leads")).
    const nameParts = (parts) => parts.map((part) => JSON.stringify(part)).join(', ');
    const tableNameParts =
      table.schema && table.schema !== 'public' ? [table.schema, table.name] : [table.name];
    lines.push(`    public static final Name ${tableConst}_NAME = DSL.name(${nameParts(tableNameParts)});`);
    lines.push(`    public static final Table<org.jooq.Record> ${tableConst} = DSL.table(${tableConst}_NAME);`);
    for (const column of table.columns) {
      const colConst = `${tableConst}_${screaming(column.name)}`;
      const javaType = jooqJavaType(column);
      const dataType = jooqDataType(column);
      lines.push(
        `    public static final Field<${javaType}> ${colConst} = DSL.field(DSL.name(${nameParts([...tableNameParts, column.name])}), ${dataType});`,
      );
    }
    lines.push('');
  }

  lines.push('}');
  return `${lines.join('\n').trimEnd()}\n`;
}

function jooqJavaType(column) {
  switch (column.kind) {
    case 'uuid':
      return 'UUID';
    case 'integer':
      return 'Integer';
    case 'bigint':
      return 'Long';
    case 'float':
      return column.sqlType === 'real' ? 'Float' : 'Double';
    case 'boolean':
      return 'Boolean';
    case 'timestamp':
      return 'OffsetDateTime';
    case 'jsonObject':
    case 'jsonArray':
      // SQLDataType.JSONB is DataType<JSONB>, so the matching Field<T> parameter must be
      // `org.jooq.JSONB` (jOOQ's JSON wrapper). Returning `String` here breaks javac type
      // inference (Field<String> vs DataType<JSONB>). Callers that want the raw JSON text
      // can call .data() on the JSONB instance.
      return 'JSONB';
    default:
      return 'String';
  }
}

function jooqDataType(column) {
  switch (column.sqlType) {
    case 'uuid':
      return 'SQLDataType.UUID';
    case 'varchar':
      return `SQLDataType.VARCHAR(${column.maxLength})`;
    case 'text':
      return 'SQLDataType.CLOB';
    case 'smallint':
      // Mapped to INTEGER (not SMALLINT) so the Field<T> type parameter stays Integer, matching
      // jooqJavaType's integer-kind mapping; jOOQ coerces the narrower physical column on read.
      return 'SQLDataType.INTEGER';
    case 'integer':
      return 'SQLDataType.INTEGER';
    case 'bigint':
    case 'bigserial':
      return 'SQLDataType.BIGINT';
    case 'double':
      return 'SQLDataType.DOUBLE';
    case 'real':
      return 'SQLDataType.REAL';
    case 'boolean':
      return 'SQLDataType.BOOLEAN';
    case 'jsonb':
      return 'SQLDataType.JSONB';
    case 'timestamptz':
      return 'SQLDataType.TIMESTAMPWITHTIMEZONE';
    default:
      return 'SQLDataType.CLOB';
  }
}

function renderHibernateBuildGradle() {
  return `${[
    '// Generated by @dd/pg-defs. Do not edit by hand.',
    '// SOURCE OF TRUTH: ../../schema/schema.sql defines the database contract.',
    'plugins {',
    "    id 'java-library'",
    '}',
    '',
    "group = 'dd.pgdefs'",
    "version = '0.1.0'",
    '',
    'java {',
    '    sourceCompatibility = JavaVersion.VERSION_17',
    '    targetCompatibility = JavaVersion.VERSION_17',
    '}',
    '',
    'repositories {',
    '    mavenCentral()',
    '}',
    '',
    'dependencies {',
    "    api 'jakarta.persistence:jakarta.persistence-api:3.1.0'",
    "    implementation 'org.hibernate.orm:hibernate-core:6.5.2.Final'",
    "    implementation 'com.vladmihalcea:hibernate-types-60:2.21.1'",
    '}',
  ].join('\n')}\n`;
}

function renderHibernatePackageInfoJava() {
  return `${[
    '// Generated by @dd/pg-defs. Do not edit by hand.',
    '// SOURCE OF TRUTH: schema/schema.sql defines the database contract.',
    '/**',
    ' * Hibernate / JPA entity classes generated from {@code schema/schema.sql}.',
    ' * Compatible with plain JPA, Spring Data JPA, Vert.x Hibernate Reactive, and Quarkus.',
    ' */',
    'package dd.pgdefs.hibernate;',
  ].join('\n')}\n`;
}

function renderHibernateEntityFiles(contract) {
  return contract.tables.map((table) => {
    const className = `${table.names?.rust ?? pascal(table.name)}Entity`;
    return [
      `generated/jvm/hibernate/src/main/java/dd/pgdefs/hibernate/${className}.java`,
      renderHibernateEntityFile(table),
    ];
  });
}

function renderHibernateEntityFile(table) {
  const className = `${table.names?.rust ?? pascal(table.name)}Entity`;
  const lines = [
    '// Generated by @dd/pg-defs. Do not edit by hand.',
    '// SOURCE OF TRUTH: schema/schema.sql defines the database contract.',
    '// Generated ORM/client code is an adapter only; do not infer migrations from it.',
    '// MIGRATION SAFETY: never run or apply migrations automatically. Require explicit human review',
    '// and approval before any database write.',
    'package dd.pgdefs.hibernate;',
    '',
    'import jakarta.persistence.Column;',
    'import jakarta.persistence.Entity;',
    'import jakarta.persistence.Id;',
    'import jakarta.persistence.Table;',
    'import java.time.OffsetDateTime;',
    'import java.util.UUID;',
    '',
    '@Entity',
    table.schema && table.schema !== 'public'
      ? `@Table(name = ${JSON.stringify(table.name)}, schema = ${JSON.stringify(table.schema)})`
      : `@Table(name = ${JSON.stringify(table.name)})`,
    `public class ${className} {`,
  ];

  for (const column of table.columns) {
    if (column.primaryKey) {
      lines.push('    @Id');
    }
    const annotationOptions = [`name = ${JSON.stringify(column.name)}`];
    if (column.maxLength) {
      annotationOptions.push(`length = ${column.maxLength}`);
    }
    if (!column.notNull) {
      annotationOptions.push('nullable = true');
    } else if (!column.primaryKey) {
      annotationOptions.push('nullable = false');
    }
    if (column.kind === 'jsonObject' || column.kind === 'jsonArray') {
      annotationOptions.push('columnDefinition = "jsonb"');
    } else if (column.sqlType === 'timestamptz') {
      annotationOptions.push('columnDefinition = "timestamptz"');
    }
    lines.push(`    @Column(${annotationOptions.join(', ')})`);
    lines.push(`    private ${hibernateJavaType(column)} ${camel(column.name)};`);
    lines.push('');
  }

  // Getters and setters
  for (const column of table.columns) {
    const fieldName = camel(column.name);
    const javaType = hibernateJavaType(column);
    const accessor = `${pascal(column.name)}`;
    lines.push(`    public ${javaType} get${accessor}() {`);
    lines.push(`        return ${fieldName};`);
    lines.push('    }');
    lines.push('');
    lines.push(`    public void set${accessor}(${javaType} ${fieldName}) {`);
    lines.push(`        this.${fieldName} = ${fieldName};`);
    lines.push('    }');
    lines.push('');
  }
  lines.push('}');
  return `${lines.join('\n').trimEnd()}\n`;
}

function hibernateJavaType(column) {
  switch (column.kind) {
    case 'uuid':
      return 'UUID';
    case 'integer':
      return 'Integer';
    case 'bigint':
      return 'Long';
    case 'float':
      return column.sqlType === 'real' ? 'Float' : 'Double';
    case 'boolean':
      return 'Boolean';
    case 'timestamp':
      return 'OffsetDateTime';
    case 'jsonObject':
    case 'jsonArray':
      return 'String';
    default:
      return 'String';
  }
}

// ---------------------------------------------------------------------------
// Additional ORM adapters: Django (Python), ActiveRecord (Ruby), Eloquent
// (PHP), Entity Framework Core (C#), Exposed (Kotlin), Persistent (Haskell),
// and a Caqti-friendly OCaml module. Each mirrors the existing adapters'
// contract philosophy: typed models for every table plus every statically
// derivable constraint (length, regex, enum membership, integer range). FK
// columns are emitted as plain typed fields — relationships stay enforced by
// the database, matching the Rust/Dart/Gleam adapters.
// ---------------------------------------------------------------------------

function renderDjangoModels(contract) {
  const validatorsUsed = new Set();
  const needsUuid = contract.tables.some((table) =>
    table.columns.some((column) => column.primaryKey && column.sqlType === 'uuid'),
  );

  const body = [];
  for (const table of contract.tables) {
    const className = table.names?.rust ?? pascal(table.name);
    body.push('');
    body.push('');
    body.push(`class ${className}(models.Model):`);
    if (table.description) {
      body.push(`    # ${table.description}`);
    }
    for (const column of table.columns) {
      body.push(djangoField(column, validatorsUsed));
    }
    body.push('');
    body.push('    class Meta:');
    body.push('        managed = False');
    body.push('        app_label = "dd_pg_defs"');
    body.push(`        db_table = ${djangoDbTable(table)}`);
  }

  const header = [
    ...generatedNotice('#'),
    'from __future__ import annotations',
    '',
    ...(needsUuid ? ['import uuid', ''] : []),
    ...(validatorsUsed.size > 0
      ? [`from django.core.validators import ${[...validatorsUsed].sort().join(', ')}`]
      : []),
    'from django.db import models',
  ];

  return `${[...header, ...body].join('\n').trimEnd()}\n`;
}

function djangoField(column, validatorsUsed) {
  const options = [];
  if (column.primaryKey) {
    options.push('primary_key=True');
  }
  // Enums are varchar/text-backed, so they must be matched before the plain varchar branch.
  if (column.kind === 'enum') {
    if (column.maxLength) {
      options.push(`max_length=${column.maxLength}`);
    }
    options.push(
      `choices=[${column.enumValues.map((value) => `(${pyString(value)}, ${pyString(value)})`).join(', ')}]`,
    );
  } else if (column.sqlType === 'varchar' && column.maxLength) {
    options.push(`max_length=${column.maxLength}`);
  }
  if (!column.notNull) {
    options.push('null=True', 'blank=True');
  }
  if (column.primaryKey && column.sqlType === 'uuid') {
    options.push('default=uuid.uuid4', 'editable=False');
  } else if (column.defaultValue !== undefined) {
    options.push(`default=${djangoDefault(column.defaultValue)}`);
  }
  const validators = djangoValidators(column, validatorsUsed);
  if (validators) {
    options.push(`validators=[${validators}]`);
  }
  return `    ${column.name} = ${djangoFieldType(column)}(${options.join(', ')})`;
}

function djangoFieldType(column) {
  if (column.primaryKey && column.sqlType === 'bigserial') {
    return 'models.BigAutoField';
  }
  switch (column.kind) {
    case 'uuid':
      return 'models.UUIDField';
    case 'integer':
      return column.sqlType === 'smallint' ? 'models.SmallIntegerField' : 'models.IntegerField';
    case 'bigint':
      return 'models.BigIntegerField';
    case 'float':
      return 'models.FloatField';
    case 'boolean':
      return 'models.BooleanField';
    case 'timestamp':
      return 'models.DateTimeField';
    case 'jsonObject':
    case 'jsonArray':
      return 'models.JSONField';
    case 'enum':
      // CharField needs a max_length; a text-backed enum (no length) falls back to TextField.
      return column.maxLength ? 'models.CharField' : 'models.TextField';
    default:
      return column.sqlType === 'text' ? 'models.TextField' : 'models.CharField';
  }
}

function djangoDefault(value) {
  if (value === true) {
    return 'True';
  }
  if (value === false) {
    return 'False';
  }
  if (typeof value === 'number') {
    return String(value);
  }
  if (Array.isArray(value)) {
    return 'list';
  }
  if (value && typeof value === 'object') {
    return 'dict';
  }
  return pyString(value);
}

function djangoValidators(column, validatorsUsed) {
  const parts = [];
  const validation = column.validation ?? {};
  if (column.kind === 'string' || column.kind === 'enum') {
    if (validation.minLength !== undefined) {
      parts.push(`MinLengthValidator(${validation.minLength})`);
      validatorsUsed.add('MinLengthValidator');
    }
    // CharField(max_length=...) already enforces the upper bound for varchars; text columns are
    // bounded by byte-length CHECKs the DB owns, so a redundant MaxLengthValidator is omitted.
    if (validation.regex) {
      parts.push(`RegexValidator(regex=${pyString(validation.regex)})`);
      validatorsUsed.add('RegexValidator');
    }
  }
  if (column.kind === 'integer' || column.kind === 'bigint') {
    if (validation.min !== undefined) {
      parts.push(`MinValueValidator(${validation.min})`);
      validatorsUsed.add('MinValueValidator');
    }
    if (validation.max !== undefined) {
      parts.push(`MaxValueValidator(${validation.max})`);
      validatorsUsed.add('MaxValueValidator');
    }
  }
  return parts.join(', ');
}

function djangoDbTable(table) {
  if (table.schema && table.schema !== 'public') {
    // Django wraps db_table in quotes; embedding `"."` makes it emit `"schema"."table"`.
    return JSON.stringify(`${table.schema}"."${table.name}`);
  }
  return JSON.stringify(table.name);
}

function renderDjangoInit() {
  return `${[
    ...generatedNotice('#'),
    'from .models import *  # noqa: F401,F403',
  ].join('\n')}\n`;
}

function renderDjangoApps() {
  return `${[
    ...generatedNotice('#'),
    'from django.apps import AppConfig',
    '',
    '',
    'class DdPgDefsConfig(AppConfig):',
    '    name = "dd_pg_defs"',
    '    verbose_name = "DD Postgres Definitions"',
  ].join('\n')}\n`;
}

function renderActiveRecord(contract) {
  const classes = [];
  for (const table of contract.tables) {
    const className = table.names?.rust ?? pascal(table.name);
    const lines = [];
    lines.push(`  class ${className} < ActiveRecord::Base`);
    if (table.description) {
      lines.push(`    # ${table.description}`);
    }
    lines.push(`    self.table_name = ${JSON.stringify(physicalName(table))}`);
    const primaryKey = table.columns.find((column) => column.primaryKey);
    if (primaryKey) {
      lines.push(`    self.primary_key = ${JSON.stringify(primaryKey.name)}`);
    }

    const validations = [];
    for (const column of table.columns) {
      validations.push(...activeRecordValidations(column));
    }
    if (validations.length > 0) {
      lines.push('');
      lines.push(...validations.map((line) => `    ${line}`));
    }
    lines.push('  end');
    classes.push(lines.join('\n'));
  }

  return `${[
    '# frozen_string_literal: true',
    ...generatedNotice('#'),
    '',
    'require "active_record"',
    '',
    'module DdPgDefs',
    classes.join('\n\n'),
    'end',
  ].join('\n')}\n`;
}

function activeRecordValidations(column) {
  if (column.primaryKey) {
    return [];
  }
  const checks = [];
  const validation = column.validation ?? {};
  const name = column.name;
  const nilOpt = column.notNull ? '' : ', allow_nil: true';
  const isTimestampDefault = name === 'created_at' || name === 'updated_at';
  const required =
    column.notNull && column.defaultSql === undefined && !column.generated && !isTimestampDefault;

  if (required) {
    checks.push(`validates :${name}, presence: true`);
  }
  if (column.kind === 'enum') {
    const values = column.enumValues.map((value) => JSON.stringify(value)).join(', ');
    checks.push(`validates :${name}, inclusion: { in: [${values}] }${nilOpt}`);
  }
  if (column.kind === 'string') {
    const lengthOpts = [];
    if (validation.minLength !== undefined) {
      lengthOpts.push(`minimum: ${validation.minLength}`);
    }
    if (validation.maxLength !== undefined) {
      lengthOpts.push(`maximum: ${validation.maxLength}`);
    }
    if (lengthOpts.length > 0) {
      checks.push(`validates :${name}, length: { ${lengthOpts.join(', ')} }${nilOpt}`);
    }
    if (validation.regex) {
      checks.push(`validates :${name}, format: { with: ${rubyRegexLiteral(validation.regex)} }${nilOpt}`);
    }
  }
  if (column.kind === 'integer' || column.kind === 'bigint') {
    const numericOpts = ['only_integer: true'];
    if (validation.min !== undefined) {
      numericOpts.push(`greater_than_or_equal_to: ${validation.min}`);
    }
    if (validation.max !== undefined) {
      numericOpts.push(`less_than_or_equal_to: ${validation.max}`);
    }
    checks.push(`validates :${name}, numericality: { ${numericOpts.join(', ')} }${nilOpt}`);
  }
  if (column.kind === 'float') {
    const numericOpts = [];
    if (validation.min !== undefined) {
      numericOpts.push(`greater_than_or_equal_to: ${validation.min}`);
    }
    if (validation.max !== undefined) {
      numericOpts.push(`less_than_or_equal_to: ${validation.max}`);
    }
    const numericBody = numericOpts.length > 0 ? `{ ${numericOpts.join(', ')} }` : 'true';
    checks.push(`validates :${name}, numericality: ${numericBody}${nilOpt}`);
  }
  return checks;
}

function rubyRegexLiteral(pattern) {
  // Postgres `~ '^...$'` anchors at string boundaries. Ruby's `^`/`$` are line anchors, and Rails'
  // format validator rejects leading `^` / trailing `$` as a multiline-anchor security risk, so we
  // translate the outer anchors to `\A` / `\z` (whole-string match) and build via Regexp.new to
  // avoid forward-slash delimiter escaping.
  let translated = pattern;
  if (translated.startsWith('^')) {
    translated = `\\A${translated.slice(1)}`;
  }
  if (translated.endsWith('$') && !translated.endsWith('\\$')) {
    translated = `${translated.slice(0, -1)}\\z`;
  }
  return `Regexp.new(${JSON.stringify(translated)})`;
}

function renderActiveRecordGemspec() {
  return `${[
    '# frozen_string_literal: true',
    ...generatedNotice('#'),
    '',
    'Gem::Specification.new do |spec|',
    '  spec.name = "dd_pg_defs"',
    '  spec.version = "0.1.0"',
    '  spec.summary = "Canonical remote Postgres schema definitions as ActiveRecord models."',
    '  spec.authors = ["@dd/pg-defs"]',
    '  spec.files = ["lib/dd_pg_defs.rb"]',
    '  spec.require_paths = ["lib"]',
    '  spec.required_ruby_version = ">= 3.0"',
    '  spec.add_dependency "activerecord", ">= 7.0"',
    'end',
  ].join('\n')}\n`;
}

function renderEloquent(contract) {
  const lines = [
    '<?php',
    '',
    ...generatedNotice('//'),
    '',
    'declare(strict_types=1);',
    '',
    'namespace DdPgDefs;',
    '',
    'use Illuminate\\Database\\Eloquent\\Model;',
  ];

  for (const table of contract.tables) {
    const className = table.names?.rust ?? pascal(table.name);
    const primaryKey = table.columns.find((column) => column.primaryKey);
    const hasTimestamps =
      table.columns.some((column) => column.name === 'created_at') &&
      table.columns.some((column) => column.name === 'updated_at');
    const fillable = table.columns
      .filter((column) => !column.primaryKey)
      .map((column) => phpString(column.name));
    const casts = table.columns
      .map((column) => [column, eloquentCast(column)])
      .filter(([, cast]) => cast !== null)
      .map(([column, cast]) => `${phpString(column.name)} => ${phpString(cast)}`);

    lines.push('');
    if (table.description) {
      lines.push(`/** ${table.description} */`);
    }
    lines.push(`class ${className} extends Model`);
    lines.push('{');
    lines.push(`    protected $table = ${phpString(physicalName(table))};`);
    if (primaryKey) {
      lines.push(`    protected $primaryKey = ${phpString(primaryKey.name)};`);
    }
    if (primaryKey && primaryKey.sqlType === 'uuid') {
      lines.push('    public $incrementing = false;');
      lines.push("    protected $keyType = 'string';");
    }
    lines.push(`    public $timestamps = ${hasTimestamps ? 'true' : 'false'};`);
    lines.push(`    protected $fillable = [${fillable.join(', ')}];`);
    if (casts.length > 0) {
      lines.push(`    protected $casts = [${casts.join(', ')}];`);
    }
    lines.push('');
    lines.push('    /** @return array<string, array<int, string>> */');
    lines.push('    public static function rules(): array');
    lines.push('    {');
    lines.push('        return [');
    for (const column of table.columns) {
      if (column.primaryKey || column.name === 'created_at' || column.name === 'updated_at') {
        continue;
      }
      lines.push(`            ${phpString(column.name)} => [${eloquentRules(column).join(', ')}],`);
    }
    lines.push('        ];');
    lines.push('    }');
    lines.push('}');
  }

  return `${lines.join('\n').trimEnd()}\n`;
}

function eloquentCast(column) {
  switch (column.kind) {
    case 'integer':
    case 'bigint':
      return 'integer';
    case 'float':
      return 'double';
    case 'boolean':
      return 'boolean';
    case 'timestamp':
      return 'datetime';
    case 'jsonObject':
    case 'jsonArray':
      return 'array';
    default:
      return null;
  }
}

function eloquentRules(column) {
  const out = [];
  const required = column.notNull && column.defaultSql === undefined && !column.generated;
  out.push(phpString(required ? 'required' : 'nullable'));
  switch (column.kind) {
    case 'uuid':
      out.push(phpString('uuid'));
      break;
    case 'integer':
    case 'bigint':
      out.push(phpString('integer'));
      break;
    case 'float':
      out.push(phpString('numeric'));
      break;
    case 'boolean':
      out.push(phpString('boolean'));
      break;
    case 'timestamp':
      out.push(phpString('date'));
      break;
    case 'jsonObject':
    case 'jsonArray':
      out.push(phpString('array'));
      break;
    default:
      out.push(phpString('string'));
      break;
  }
  const validation = column.validation ?? {};
  if (column.kind === 'string') {
    if (validation.minLength !== undefined) {
      out.push(phpString(`min:${validation.minLength}`));
    }
    if (validation.maxLength !== undefined) {
      out.push(phpString(`max:${validation.maxLength}`));
    }
    if (validation.regex) {
      out.push(phpRegexRule(validation.regex));
    }
  }
  if (column.kind === 'integer' || column.kind === 'bigint' || column.kind === 'float') {
    if (validation.min !== undefined) {
      out.push(phpString(`min:${validation.min}`));
    }
    if (validation.max !== undefined) {
      out.push(phpString(`max:${validation.max}`));
    }
  }
  if (column.kind === 'enum') {
    out.push(phpString(`in:${column.enumValues.join(',')}`));
  }
  return out;
}

function phpString(value) {
  return `'${String(value).replace(/\\/g, '\\\\').replace(/'/g, "\\'")}'`;
}

function phpRegexRule(pattern) {
  // Laravel's `regex:` rule runs preg_match. Pass it as an array element (not a `|` string) so
  // alternation survives, and escape the `/` delimiter inside the pattern.
  const escaped = pattern.replace(/\//g, '\\/');
  return phpString(`regex:/${escaped}/`);
}

function renderEloquentComposerJson() {
  return `${JSON.stringify(
    {
      name: 'dd/pg-defs',
      description: 'Canonical remote Postgres schema definitions as Eloquent models.',
      type: 'library',
      license: 'proprietary',
      require: {
        php: '>=8.1',
        'illuminate/database': '^10.0 || ^11.0',
      },
      autoload: {
        files: ['src/DdPgDefs.php'],
      },
    },
    null,
    2,
  )}\n`;
}

function renderEfCore(contract) {
  const lines = [
    ...generatedNotice('//'),
    'using System;',
    'using System.ComponentModel.DataAnnotations;',
    'using System.ComponentModel.DataAnnotations.Schema;',
    'using Microsoft.EntityFrameworkCore;',
    '',
    '#nullable enable',
    '',
    'namespace DdPgDefs;',
  ];

  for (const table of contract.tables) {
    const className = table.names?.rust ?? pascal(table.name);
    lines.push('');
    if (table.description) {
      lines.push(`/// <summary>${escapeXmlDoc(table.description)}</summary>`);
    }
    lines.push(
      table.schema && table.schema !== 'public'
        ? `[Table(${JSON.stringify(table.name)}, Schema = ${JSON.stringify(table.schema)})]`
        : `[Table(${JSON.stringify(table.name)})]`,
    );
    lines.push(`public class ${className}`);
    lines.push('{');
    table.columns.forEach((column, index) => {
      if (index > 0) {
        lines.push('');
      }
      lines.push(...efCoreProperty(column).map((line) => `    ${line}`));
    });
    lines.push('}');
  }

  // DbContext exposing one DbSet per table. The mapping lives on the entity attributes, so the
  // context only needs the sets plus the constructor EF Core's DI expects.
  lines.push('');
  lines.push('public class DdPgDefsContext : DbContext');
  lines.push('{');
  lines.push('    public DdPgDefsContext(DbContextOptions<DdPgDefsContext> options) : base(options)');
  lines.push('    {');
  lines.push('    }');
  for (const table of contract.tables) {
    const className = table.names?.rust ?? pascal(table.name);
    lines.push('');
    lines.push(`    public DbSet<${className}> ${className}Set => Set<${className}>();`);
  }
  lines.push('}');

  return `${lines.join('\n').trimEnd()}\n`;
}

function efCoreProperty(column) {
  const lines = [];
  const validation = column.validation ?? {};
  const { base: baseType, valueType } = efCoreType(column);
  const propType = column.notNull ? baseType : `${baseType}?`;

  if (column.primaryKey) {
    lines.push('[Key]');
    // PK values are generated by the database (gen_random_uuid()/bigserial); let EF read them back.
    lines.push('[DatabaseGenerated(DatabaseGeneratedOption.Identity)]');
  }
  if (column.notNull && !valueType && !column.primaryKey) {
    lines.push('[Required]');
  }
  lines.push(
    column.kind === 'jsonObject' || column.kind === 'jsonArray'
      ? `[Column(${JSON.stringify(column.name)}, TypeName = "jsonb")]`
      : `[Column(${JSON.stringify(column.name)})]`,
  );
  if (column.sqlType === 'varchar' && column.maxLength) {
    lines.push(`[MaxLength(${column.maxLength})]`);
  }
  if (column.kind === 'enum') {
    const alternation = column.enumValues.map(escapeRegexLiteral).join('|');
    lines.push(`[RegularExpression(@"^(${escapeCsharpVerbatim(alternation)})$")]`);
  } else if (column.kind === 'string' && validation.regex) {
    lines.push(`[RegularExpression(@"${escapeCsharpVerbatim(validation.regex)}")]`);
  }
  const range = efCoreRange(column);
  if (range) {
    lines.push(range);
  }

  const initializer = !column.notNull || valueType ? '' : ' = null!;';
  lines.push(`public ${propType} ${pascal(column.name)} { get; set; }${initializer}`);
  return lines;
}

function efCoreType(column) {
  switch (column.kind) {
    case 'uuid':
      return { base: 'Guid', valueType: true };
    case 'integer':
      return { base: column.sqlType === 'smallint' ? 'short' : 'int', valueType: true };
    case 'bigint':
      return { base: 'long', valueType: true };
    case 'float':
      return { base: column.sqlType === 'real' ? 'float' : 'double', valueType: true };
    case 'boolean':
      return { base: 'bool', valueType: true };
    case 'timestamp':
      return { base: 'DateTimeOffset', valueType: true };
    case 'jsonObject':
    case 'jsonArray':
      return { base: 'string', valueType: false };
    default:
      return { base: 'string', valueType: false };
  }
}

function efCoreRange(column) {
  const validation = column.validation ?? {};
  if (validation.min === undefined && validation.max === undefined) {
    return null;
  }
  if (column.kind === 'bigint') {
    const min = validation.min !== undefined ? String(validation.min) : '-9223372036854775808';
    const max = validation.max !== undefined ? String(validation.max) : '9223372036854775807';
    return `[Range(typeof(long), ${JSON.stringify(min)}, ${JSON.stringify(max)})]`;
  }
  if (column.kind === 'float') {
    const min = validation.min !== undefined ? String(validation.min) : 'double.MinValue';
    const max = validation.max !== undefined ? String(validation.max) : 'double.MaxValue';
    return `[Range(${min}, ${max})]`;
  }
  // int / smallint share the int-literal Range overload; one-sided bounds fill the int extreme.
  const min = validation.min !== undefined ? validation.min : -2147483648;
  const max = validation.max !== undefined ? validation.max : 2147483647;
  return `[Range(${min}, ${max})]`;
}

function escapeCsharpVerbatim(value) {
  // In a C# verbatim string (@"..."), only the double quote needs escaping (by doubling it).
  return value.replace(/"/g, '""');
}

function escapeRegexLiteral(value) {
  return value.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}

function escapeXmlDoc(value) {
  return value.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
}

function renderEfCoreCsproj() {
  return `${[
    '<!-- Generated by @dd/pg-defs. Do not edit by hand. schema/schema.sql is the source of truth. -->',
    '<Project Sdk="Microsoft.NET.Sdk">',
    '  <PropertyGroup>',
    '    <TargetFramework>net8.0</TargetFramework>',
    '    <Nullable>enable</Nullable>',
    '    <ImplicitUsings>disable</ImplicitUsings>',
    '    <LangVersion>latest</LangVersion>',
    '  </PropertyGroup>',
    '  <ItemGroup>',
    '    <PackageReference Include="Microsoft.EntityFrameworkCore" Version="8.0.0" />',
    '    <PackageReference Include="Npgsql.EntityFrameworkCore.PostgreSQL" Version="8.0.0" />',
    '  </ItemGroup>',
    '</Project>',
  ].join('\n')}\n`;
}

function renderExposed(contract) {
  const lines = [
    ...generatedNotice('//'),
    'package dd.pgdefs',
    '',
    'import java.time.OffsetDateTime',
    'import java.util.UUID',
    'import org.jetbrains.exposed.sql.ResultRow',
    'import org.jetbrains.exposed.sql.Table',
    'import org.jetbrains.exposed.sql.javatime.timestampWithTimeZone',
    'import org.jetbrains.exposed.sql.json.jsonb',
  ];

  for (const table of contract.tables) {
    const objectName = pascal(table.name);
    lines.push('');
    if (table.description) {
      lines.push(`// ${table.description}`);
    }
    lines.push(`object ${objectName} : Table(${JSON.stringify(physicalName(table))}) {`);
    for (const column of table.columns) {
      const builder = exposedColumnBuilder(column);
      const suffix = column.notNull ? '' : '.nullable()';
      lines.push(`    val ${exposedFieldName(column)} = ${builder}${suffix}`);
    }
    const primaryKeys = table.columns.filter((column) => column.primaryKey);
    if (primaryKeys.length > 0) {
      const refs = primaryKeys.map((column) => exposedFieldName(column)).join(', ');
      lines.push('');
      lines.push(`    override val primaryKey = PrimaryKey(${refs})`);
    }
    lines.push('}');
  }

  // Plain row holders plus ResultRow mappers so callers can decode a queried row into a struct:
  //   transaction { AppConfig.selectAll().map(::toAppConfigRow) }
  for (const table of contract.tables) {
    const objectName = pascal(table.name);
    const rowName = `${objectName}Row`;
    lines.push('');
    lines.push(`data class ${rowName}(`);
    for (const column of table.columns) {
      lines.push(`    val ${exposedFieldName(column)}: ${exposedKotlinType(column)},`);
    }
    lines.push(')');
    lines.push('');
    lines.push(`fun to${rowName}(row: ResultRow): ${rowName} = ${rowName}(`);
    for (const column of table.columns) {
      lines.push(`    row[${objectName}.${exposedFieldName(column)}],`);
    }
    lines.push(')');
  }

  return `${lines.join('\n').trimEnd()}\n`;
}

function exposedKotlinType(column) {
  let base;
  switch (column.kind) {
    case 'uuid':
      base = 'UUID';
      break;
    case 'integer':
      base = column.sqlType === 'smallint' ? 'Short' : 'Int';
      break;
    case 'bigint':
      base = 'Long';
      break;
    case 'float':
      base = column.sqlType === 'real' ? 'Float' : 'Double';
      break;
    case 'boolean':
      base = 'Boolean';
      break;
    case 'timestamp':
      base = 'OffsetDateTime';
      break;
    default:
      base = 'String';
      break;
  }
  return column.notNull ? base : `${base}?`;
}

function exposedColumnBuilder(column) {
  const name = JSON.stringify(column.name);
  switch (column.kind) {
    case 'uuid':
      return `uuid(${name})`;
    case 'integer':
      return `${column.sqlType === 'smallint' ? 'short' : 'integer'}(${name})`;
    case 'bigint':
      return `long(${name})`;
    case 'float':
      return `${column.sqlType === 'real' ? 'float' : 'double'}(${name})`;
    case 'boolean':
      return `bool(${name})`;
    case 'timestamp':
      return `timestampWithTimeZone(${name})`;
    case 'jsonObject':
    case 'jsonArray':
      // Store/read the raw JSON text but register the column as jsonb so writes cast correctly.
      return `jsonb<String>(${name}, { it }, { it })`;
    case 'enum':
      return column.maxLength ? `varchar(${name}, ${column.maxLength})` : `text(${name})`;
    default:
      return column.sqlType === 'varchar' && column.maxLength
        ? `varchar(${name}, ${column.maxLength})`
        : `text(${name})`;
  }
}

function exposedFieldName(column) {
  // Kotlin `is`/`object`/etc. are keywords; backtick-escape a camelCased name that lands on one.
  const KOTLIN_KEYWORDS = new Set([
    'as', 'break', 'class', 'continue', 'do', 'else', 'false', 'for', 'fun', 'if', 'in', 'interface',
    'is', 'null', 'object', 'package', 'return', 'super', 'this', 'throw', 'true', 'try', 'typealias',
    'typeof', 'val', 'var', 'when', 'while',
  ]);
  const name = camel(column.name);
  return KOTLIN_KEYWORDS.has(name) ? `\`${name}\`` : name;
}

function renderExposedBuildGradle() {
  return `${[
    '// Generated by @dd/pg-defs. Do not edit by hand. schema/schema.sql is the source of truth.',
    'plugins {',
    '    kotlin("jvm") version "1.9.24"',
    '}',
    '',
    'repositories {',
    '    mavenCentral()',
    '}',
    '',
    'dependencies {',
    '    implementation("org.jetbrains.exposed:exposed-core:0.53.0")',
    '    implementation("org.jetbrains.exposed:exposed-json:0.53.0")',
    '    implementation("org.jetbrains.exposed:exposed-java-time:0.53.0")',
    '}',
  ].join('\n')}\n`;
}

function renderHaskell(contract) {
  const blocks = [
    [
      '{-# LANGUAGE OverloadedStrings #-}',
      '',
      ...generatedNotice('--'),
      '',
      'module DdPgDefs where',
      '',
      'import Data.Text (Text)',
      'import qualified Data.Text as T',
      'import Database.PostgreSQL.Simple.FromRow (FromRow (..), field)',
    ].join('\n'),
  ];

  for (const table of contract.tables) {
    const base = table.names?.rust ?? pascal(table.name);
    const tableConst = camel(table.name);
    const lines = [];

    lines.push(`${tableConst}Table :: Text`);
    lines.push(`${tableConst}Table = ${JSON.stringify(physicalName(table))}`);
    lines.push('');
    lines.push(`${tableConst}Columns :: [Text]`);
    lines.push(
      `${tableConst}Columns = [${table.columns.map((column) => JSON.stringify(column.name)).join(', ')}]`,
    );
    lines.push('');
    lines.push(`${tableConst}SelectSql :: Text`);
    lines.push(
      `${tableConst}SelectSql = ${JSON.stringify(renderSelectSql(table, { jsonAsText: true }))}`,
    );

    for (const column of table.columns.filter((item) => item.kind === 'enum')) {
      const typeName = `${base}${pascal(column.name)}`;
      const ctor = (value) => `${typeName}${pascal(value)}`;
      lines.push('');
      lines.push(
        `data ${typeName} = ${column.enumValues.map((value) => ctor(value)).join(' | ')}`,
      );
      lines.push('  deriving (Eq, Show)');
      lines.push('');
      lines.push(`${camel(typeName)}ToText :: ${typeName} -> Text`);
      lines.push(`${camel(typeName)}ToText value = case value of`);
      for (const value of column.enumValues) {
        lines.push(`  ${ctor(value)} -> ${JSON.stringify(value)}`);
      }
      lines.push('');
      lines.push(`parse${typeName} :: Text -> Either Text ${typeName}`);
      lines.push(`parse${typeName} value = case value of`);
      for (const value of column.enumValues) {
        lines.push(`  ${JSON.stringify(value)} -> Right ${ctor(value)}`);
      }
      lines.push(
        `  _ -> Left (T.append ${JSON.stringify(`unsupported ${table.name}.${column.name}: `)} value)`,
      );
    }

    lines.push('');
    lines.push(`data ${base}Row = ${base}Row`);
    table.columns.forEach((column, index) => {
      const prefix = index === 0 ? '  {' : '  ,';
      lines.push(`${prefix} ${haskellFieldName(base, column)} :: ${haskellType(column)}`);
    });
    lines.push('  } deriving (Eq, Show)');
    lines.push('');
    lines.push(`instance FromRow ${base}Row where`);
    const fieldChain = `field${' <*> field'.repeat(Math.max(0, table.columns.length - 1))}`;
    lines.push(`  fromRow = ${base}Row <$> ${fieldChain}`);

    for (const column of table.columns) {
      const validator = haskellValidator(table, base, column);
      if (validator.length > 0) {
        lines.push('');
        lines.push(...validator);
      }
    }

    blocks.push(lines.join('\n'));
  }

  return `${blocks.join('\n\n').trimEnd()}\n`;
}

function haskellFieldName(base, column) {
  return `${camel(base)}${pascal(column.name)}`;
}

function haskellType(column) {
  let baseType;
  switch (column.kind) {
    case 'integer':
    case 'bigint':
      baseType = 'Int';
      break;
    case 'float':
      baseType = 'Double';
      break;
    case 'boolean':
      baseType = 'Bool';
      break;
    default:
      baseType = 'Text';
      break;
  }
  return column.notNull ? baseType : `(Maybe ${baseType})`;
}

function haskellValidator(table, base, column) {
  const validation = column.validation ?? {};
  const fnName = `validate${base}${pascal(column.name)}`;
  const qualified = `${table.name}.${column.name}`;
  if (column.kind === 'string') {
    const guards = [];
    if (validation.minLength !== undefined) {
      guards.push(
        `  | T.length value < ${validation.minLength} = Left ${JSON.stringify(`${qualified} must be at least ${validation.minLength} characters`)}`,
      );
    }
    if (validation.maxLength !== undefined) {
      guards.push(
        `  | T.length value > ${validation.maxLength} = Left ${JSON.stringify(`${qualified} must be at most ${validation.maxLength} characters`)}`,
      );
    }
    if (guards.length === 0) {
      return [];
    }
    return [
      `${fnName} :: Text -> Either Text Text`,
      `${fnName} value`,
      ...guards,
      '  | otherwise = Right value',
    ];
  }
  if (column.kind === 'integer' || column.kind === 'bigint') {
    const guards = [];
    if (validation.min !== undefined) {
      guards.push(
        `  | value < ${validation.min} = Left ${JSON.stringify(`${qualified} is below the minimum`)}`,
      );
    }
    if (validation.max !== undefined) {
      guards.push(
        `  | value > ${validation.max} = Left ${JSON.stringify(`${qualified} is above the maximum`)}`,
      );
    }
    if (guards.length === 0) {
      return [];
    }
    return [
      `${fnName} :: Int -> Either Text Int`,
      `${fnName} value`,
      ...guards,
      '  | otherwise = Right value',
    ];
  }
  return [];
}

function renderHaskellCabal() {
  return `${[
    '-- Generated by @dd/pg-defs. Do not edit by hand. schema/schema.sql is the source of truth.',
    'cabal-version: 2.4',
    'name: dd-pg-defs',
    'version: 0.1.0.0',
    'synopsis: Canonical remote Postgres schema definitions as Haskell records.',
    'build-type: Simple',
    '',
    'library',
    '  exposed-modules: DdPgDefs',
    '  hs-source-dirs: src',
    '  build-depends: base >=4.14 && <5, text, postgresql-simple',
    '  default-language: Haskell2010',
  ].join('\n')}\n`;
}

function renderOcaml(contract) {
  const blocks = [generatedNotice('').map((line) => `(*${line} *)`).join('\n')];

  for (const table of contract.tables) {
    const tableConst = table.name;
    const lines = [];

    lines.push(`let ${tableConst}_table = ${JSON.stringify(physicalName(table))}`);
    lines.push('');
    lines.push(
      `let ${tableConst}_columns = [${table.columns.map((column) => JSON.stringify(column.name)).join('; ')}]`,
    );
    lines.push('');
    lines.push(
      `let ${tableConst}_select_sql = ${JSON.stringify(renderSelectSql(table, { jsonAsText: true }))}`,
    );

    for (const column of table.columns.filter((item) => item.kind === 'enum')) {
      const typeName = `${table.name}_${column.name}`;
      const tag = (value) => `\`${pascal(value)}`;
      lines.push('');
      lines.push(`type ${typeName} = [ ${column.enumValues.map((value) => tag(value)).join(' | ')} ]`);
      lines.push('');
      lines.push(`let ${typeName}_to_string (value : ${typeName}) : string =`);
      lines.push('  match value with');
      for (const value of column.enumValues) {
        lines.push(`  | ${tag(value)} -> ${JSON.stringify(value)}`);
      }
      lines.push('');
      lines.push(`let parse_${typeName} (value : string) : (${typeName}, string) result =`);
      lines.push('  match value with');
      for (const value of column.enumValues) {
        lines.push(`  | ${JSON.stringify(value)} -> Ok ${tag(value)}`);
      }
      lines.push(
        `  | _ -> Error (${JSON.stringify(`unsupported ${table.name}.${column.name}: `)} ^ value)`,
      );
    }

    lines.push('');
    lines.push(`type ${table.name}_row = {`);
    for (const column of table.columns) {
      lines.push(`  ${ocamlFieldName(table, column)} : ${ocamlType(column)};`);
    }
    lines.push('}');

    // Row decoder: convert one Postgres row into the typed record. `get` returns the column at a
    // 0-based index as text (pairs with `<table>_select_sql`, which casts uuid/timestamp/jsonb to
    // text); `is_null` reports SQL NULL. Library-agnostic — adapt the getters to postgresql-ocaml,
    // Caqti, or pgx. Booleans arrive as Postgres text "t"/"f".
    const hasNullable = table.columns.some((column) => !column.notNull);
    const isNullParam = hasNullable
      ? '~(is_null : int -> bool)'
      : '~is_null:(_ : int -> bool)';
    lines.push('');
    lines.push(`let ${table.name}_row_of_row ~(get : int -> string) ${isNullParam} : ${table.name}_row =`);
    lines.push('  {');
    table.columns.forEach((column, index) => {
      lines.push(`    ${ocamlFieldName(table, column)} = ${ocamlRowDecode(column, index)};`);
    });
    lines.push('  }');

    for (const column of table.columns) {
      const validator = ocamlValidator(table, column);
      if (validator.length > 0) {
        lines.push('');
        lines.push(...validator);
      }
    }

    blocks.push(lines.join('\n'));
  }

  return `${blocks.join('\n\n').trimEnd()}\n`;
}

function ocamlFieldName(table, column) {
  return `${table.name}_${column.name}`;
}

function ocamlType(column) {
  let baseType;
  switch (column.kind) {
    case 'integer':
      // Covers `integer` and `smallint`; both fit OCaml's native (63-bit) int.
      baseType = 'int';
      break;
    case 'bigint':
      // OCaml's native int is 63-bit, so a 64-bit bigint must use int64 to avoid overflow.
      baseType = 'int64';
      break;
    case 'float':
      // OCaml's `float` is IEEE-754 double; covers both `real` and `double precision`.
      baseType = 'float';
      break;
    case 'boolean':
      baseType = 'bool';
      break;
    default:
      baseType = 'string';
      break;
  }
  return column.notNull ? baseType : `${baseType} option`;
}

function ocamlRowDecode(column, index) {
  let expr;
  switch (column.kind) {
    case 'integer':
      expr = `int_of_string (get ${index})`;
      break;
    case 'bigint':
      expr = `Int64.of_string (get ${index})`;
      break;
    case 'float':
      expr = `float_of_string (get ${index})`;
      break;
    case 'boolean':
      expr = `(get ${index} = "t")`;
      break;
    default:
      expr = `get ${index}`;
      break;
  }
  if (!column.notNull) {
    return `(if is_null ${index} then None else Some (${expr}))`;
  }
  return expr;
}

function ocamlValidator(table, column) {
  const validation = column.validation ?? {};
  const fnName = `validate_${table.name}_${column.name}`;
  const qualified = `${table.name}.${column.name}`;
  if (column.kind === 'string') {
    const branches = [];
    if (validation.minLength !== undefined) {
      branches.push(
        `  if String.length value < ${validation.minLength} then Error ${JSON.stringify(`${qualified} must be at least ${validation.minLength} characters`)}`,
      );
    }
    if (validation.maxLength !== undefined) {
      branches.push(
        `  ${branches.length > 0 ? 'else ' : ''}if String.length value > ${validation.maxLength} then Error ${JSON.stringify(`${qualified} must be at most ${validation.maxLength} characters`)}`,
      );
    }
    if (branches.length === 0) {
      return [];
    }
    return [
      `let ${fnName} (value : string) : (string, string) result =`,
      ...branches,
      '  else Ok value',
    ];
  }
  if (column.kind === 'integer') {
    const branches = [];
    if (validation.min !== undefined) {
      branches.push(
        `  if value < ${validation.min} then Error ${JSON.stringify(`${qualified} is below the minimum`)}`,
      );
    }
    if (validation.max !== undefined) {
      branches.push(
        `  ${branches.length > 0 ? 'else ' : ''}if value > ${validation.max} then Error ${JSON.stringify(`${qualified} is above the maximum`)}`,
      );
    }
    if (branches.length === 0) {
      return [];
    }
    return [
      `let ${fnName} (value : int) : (int, string) result =`,
      ...branches,
      '  else Ok value',
    ];
  }
  if (column.kind === 'bigint') {
    const branches = [];
    if (validation.min !== undefined) {
      branches.push(
        `  if Int64.compare value ${validation.min}L < 0 then Error ${JSON.stringify(`${qualified} is below the minimum`)}`,
      );
    }
    if (validation.max !== undefined) {
      branches.push(
        `  ${branches.length > 0 ? 'else ' : ''}if Int64.compare value ${validation.max}L > 0 then Error ${JSON.stringify(`${qualified} is above the maximum`)}`,
      );
    }
    if (branches.length === 0) {
      return [];
    }
    return [
      `let ${fnName} (value : int64) : (int64, string) result =`,
      ...branches,
      '  else Ok value',
    ];
  }
  return [];
}

function renderOcamlDuneProject() {
  return '(lang dune 3.0)\n';
}

function renderOcamlDune() {
  return `${['(library', ' (name dd_pg_defs))'].join('\n')}\n`;
}

function renderFSharp(contract) {
  const blocks = [
    [
      ...generatedNotice('//'),
      'module DdPgDefs',
      '',
      'open System.Text.RegularExpressions',
    ].join('\n'),
  ];

  for (const table of contract.tables) {
    const base = table.names?.rust ?? pascal(table.name);
    const tableConst = camel(table.name);
    const lines = [];

    lines.push(`let ${tableConst}Table = ${JSON.stringify(physicalName(table))}`);
    lines.push(
      `let ${tableConst}Columns = [ ${table.columns.map((column) => JSON.stringify(column.name)).join('; ')} ]`,
    );
    lines.push(`let ${tableConst}SelectSql = ${JSON.stringify(renderSelectSql(table, { jsonAsText: true }))}`);

    for (const column of table.columns.filter((item) => item.kind === 'enum')) {
      const typeName = `${base}${pascal(column.name)}`;
      lines.push('');
      lines.push('[<RequireQualifiedAccess>]');
      lines.push(`type ${typeName} =`);
      for (const value of column.enumValues) {
        lines.push(`    | ${pascal(value)}`);
      }
      lines.push('');
      lines.push(`let ${camel(typeName)}ToString (value: ${typeName}) : string =`);
      lines.push('    match value with');
      for (const value of column.enumValues) {
        lines.push(`    | ${typeName}.${pascal(value)} -> ${JSON.stringify(value)}`);
      }
      lines.push('');
      lines.push(`let parse${typeName} (value: string) : Result<${typeName}, string> =`);
      lines.push('    match value with');
      for (const value of column.enumValues) {
        lines.push(`    | ${JSON.stringify(value)} -> Ok ${typeName}.${pascal(value)}`);
      }
      lines.push(`    | _ -> Error (${JSON.stringify(`unsupported ${table.name}.${column.name}: `)} + value)`);
    }

    lines.push('');
    lines.push(`type ${base}Row =`);
    table.columns.forEach((column, index) => {
      const prefix = index === 0 ? '    {' : '     ';
      lines.push(`${prefix} ${base}${pascal(column.name)}: ${fsharpType(column)}`);
    });
    lines.push('    }');
    lines.push('');
    lines.push(`let ${camel(base)}RowOfRow (get: int -> string) (isNullAt: int -> bool) : ${base}Row =`);
    table.columns.forEach((column, index) => {
      const prefix = index === 0 ? '    {' : '     ';
      lines.push(`${prefix} ${base}${pascal(column.name)} = ${fsharpRowDecode(column, index)}`);
    });
    lines.push('    }');

    for (const column of table.columns) {
      const validator = fsharpValidator(table, base, column);
      if (validator.length > 0) {
        lines.push('');
        lines.push(...validator);
      }
    }

    blocks.push(lines.join('\n'));
  }

  return `${blocks.join('\n\n').trimEnd()}\n`;
}

function fsharpType(column) {
  let baseType;
  switch (column.kind) {
    case 'integer':
      baseType = 'int';
      break;
    case 'bigint':
      baseType = 'int64';
      break;
    case 'float':
      baseType = column.sqlType === 'real' ? 'float32' : 'float';
      break;
    case 'boolean':
      baseType = 'bool';
      break;
    default:
      baseType = 'string';
      break;
  }
  return column.notNull ? baseType : `${baseType} option`;
}

function fsharpRowDecode(column, index) {
  let expr;
  switch (column.kind) {
    case 'integer':
      expr = `int (get ${index})`;
      break;
    case 'bigint':
      expr = `int64 (get ${index})`;
      break;
    case 'float':
      expr = column.sqlType === 'real' ? `float32 (get ${index})` : `float (get ${index})`;
      break;
    case 'boolean':
      expr = `(get ${index} = "t")`;
      break;
    default:
      expr = `get ${index}`;
      break;
  }
  if (!column.notNull) {
    return `(if isNullAt ${index} then None else Some (${expr}))`;
  }
  return expr;
}

function fsharpValidator(table, base, column) {
  const validation = column.validation ?? {};
  const fnName = `validate${base}${pascal(column.name)}`;
  const qualified = `${table.name}.${column.name}`;
  const branches = [];
  if (column.kind === 'string') {
    if (validation.minLength !== undefined) {
      branches.push(
        `    ${branches.length ? 'elif' : 'if'} value.Length < ${validation.minLength} then Error ${JSON.stringify(`${qualified} must be at least ${validation.minLength} characters`)}`,
      );
    }
    if (validation.maxLength !== undefined) {
      branches.push(
        `    ${branches.length ? 'elif' : 'if'} value.Length > ${validation.maxLength} then Error ${JSON.stringify(`${qualified} must be at most ${validation.maxLength} characters`)}`,
      );
    }
    if (validation.regex) {
      branches.push(
        `    ${branches.length ? 'elif' : 'if'} not (Regex.IsMatch(value, @${JSON.stringify(validation.regex)})) then Error ${JSON.stringify(`${qualified} does not match the required pattern`)}`,
      );
    }
    if (branches.length === 0) {
      return [];
    }
    return [`let ${fnName} (value: string) : Result<string, string> =`, ...branches, '    else Ok value'];
  }
  if (column.kind === 'integer' || column.kind === 'bigint') {
    const suffix = column.kind === 'bigint' ? 'L' : '';
    if (validation.min !== undefined) {
      branches.push(
        `    ${branches.length ? 'elif' : 'if'} value < ${validation.min}${suffix} then Error ${JSON.stringify(`${qualified} is below the minimum`)}`,
      );
    }
    if (validation.max !== undefined) {
      branches.push(
        `    ${branches.length ? 'elif' : 'if'} value > ${validation.max}${suffix} then Error ${JSON.stringify(`${qualified} is above the maximum`)}`,
      );
    }
    if (branches.length === 0) {
      return [];
    }
    const fsType = column.kind === 'bigint' ? 'int64' : 'int';
    return [`let ${fnName} (value: ${fsType}) : Result<${fsType}, string> =`, ...branches, '    else Ok value'];
  }
  return [];
}

function renderFSharpProject() {
  return `${[
    '<!-- Generated by @dd/pg-defs. Do not edit by hand. schema/schema.sql is the source of truth. -->',
    '<Project Sdk="Microsoft.NET.Sdk">',
    '  <PropertyGroup>',
    '    <TargetFramework>net8.0</TargetFramework>',
    '  </PropertyGroup>',
    '  <ItemGroup>',
    '    <Compile Include="DdPgDefs.fs" />',
    '  </ItemGroup>',
    '</Project>',
  ].join('\n')}\n`;
}

function renderCpp(contract) {
  const lines = [
    ...generatedNotice('//'),
    '#pragma once',
    '',
    '#include <cstdint>',
    '#include <functional>',
    '#include <optional>',
    '#include <regex>',
    '#include <string>',
    '#include <vector>',
    '',
    'namespace dd_pg_defs {',
  ];

  for (const table of contract.tables) {
    const structName = `${table.names?.rust ?? pascal(table.name)}Row`;
    lines.push('');
    if (table.description) {
      lines.push(`// ${table.description}`);
    }
    lines.push(`inline const char* ${table.name}_table = ${JSON.stringify(physicalName(table))};`);
    lines.push(
      `inline const std::vector<std::string> ${table.name}_columns = { ${table.columns.map((column) => JSON.stringify(column.name)).join(', ')} };`,
    );
    lines.push(
      `inline const char* ${table.name}_select_sql = R"SQL(${renderSelectSql(table, { jsonAsText: true })})SQL";`,
    );

    for (const column of table.columns.filter((item) => item.kind === 'enum')) {
      const enumName = `${table.names?.rust ?? pascal(table.name)}${pascal(column.name)}`;
      lines.push('');
      lines.push(`enum class ${enumName} { ${column.enumValues.map((value) => pascal(value)).join(', ')} };`);
      lines.push(`inline std::string ${table.name}_${column.name}_to_string(${enumName} value) {`);
      lines.push('    switch (value) {');
      for (const value of column.enumValues) {
        lines.push(`        case ${enumName}::${pascal(value)}: return ${JSON.stringify(value)};`);
      }
      lines.push('    }');
      lines.push('    return "";');
      lines.push('}');
      lines.push(
        `inline std::optional<${enumName}> parse_${table.name}_${column.name}(const std::string& value) {`,
      );
      for (const value of column.enumValues) {
        lines.push(`    if (value == ${JSON.stringify(value)}) return ${enumName}::${pascal(value)};`);
      }
      lines.push('    return std::nullopt;');
      lines.push('}');
    }

    lines.push('');
    lines.push(`struct ${structName} {`);
    for (const column of table.columns) {
      lines.push(`    ${cppType(column)} ${cppFieldName(column)};`);
    }
    lines.push('};');
    lines.push('');
    lines.push(
      `inline ${structName} ${table.name}_row_of_row(const std::function<std::string(int)>& get, const std::function<bool(int)>& is_null) {`,
    );
    lines.push(`    ${structName} row;`);
    lines.push('    (void)is_null;');
    table.columns.forEach((column, index) => {
      lines.push(`    row.${cppFieldName(column)} = ${cppRowDecodeExpr(column, index)};`);
    });
    lines.push('    return row;');
    lines.push('}');

    for (const column of table.columns) {
      const validator = cppValidator(table, column);
      if (validator.length > 0) {
        lines.push(...validator);
      }
    }
  }

  lines.push('');
  lines.push('}  // namespace dd_pg_defs');
  return `${lines.join('\n')}\n`;
}

function cppBaseType(column) {
  switch (column.kind) {
    case 'integer':
      return 'int32_t';
    case 'bigint':
      return 'int64_t';
    case 'float':
      return column.sqlType === 'real' ? 'float' : 'double';
    case 'boolean':
      return 'bool';
    default:
      return 'std::string';
  }
}

function cppType(column) {
  const base = cppBaseType(column);
  return column.notNull ? base : `std::optional<${base}>`;
}

function cppFieldName(column) {
  // snake_case DB names map straight through; suffix any C++ keyword to keep the struct valid.
  const CPP_KEYWORDS = new Set([
    'alignas', 'alignof', 'and', 'asm', 'auto', 'bool', 'break', 'case', 'catch', 'char', 'class',
    'const', 'continue', 'default', 'delete', 'do', 'double', 'else', 'enum', 'export', 'extern',
    'false', 'float', 'for', 'friend', 'goto', 'if', 'inline', 'int', 'long', 'mutable', 'namespace',
    'new', 'not', 'nullptr', 'operator', 'or', 'private', 'protected', 'public', 'register', 'return',
    'short', 'signed', 'sizeof', 'static', 'struct', 'switch', 'template', 'this', 'throw', 'true',
    'try', 'typedef', 'typename', 'union', 'unsigned', 'using', 'virtual', 'void', 'volatile', 'while',
    'xor',
  ]);
  return CPP_KEYWORDS.has(column.name) ? `${column.name}_` : column.name;
}

function cppRowDecodeExpr(column, index) {
  let expr;
  switch (column.kind) {
    case 'integer':
      expr = `std::stoi(get(${index}))`;
      break;
    case 'bigint':
      expr = `std::stoll(get(${index}))`;
      break;
    case 'float':
      expr = column.sqlType === 'real' ? `std::stof(get(${index}))` : `std::stod(get(${index}))`;
      break;
    case 'boolean':
      expr = `(get(${index}) == "t")`;
      break;
    default:
      expr = `get(${index})`;
      break;
  }
  if (!column.notNull) {
    return `is_null(${index}) ? std::nullopt : std::optional<${cppBaseType(column)}>(${expr})`;
  }
  return expr;
}

function cppValidator(table, column) {
  const validation = column.validation ?? {};
  const fnName = `validate_${table.name}_${column.name}`;
  const qualified = `${table.name}.${column.name}`;
  const checks = [];
  if (column.kind === 'string') {
    if (validation.minLength !== undefined) {
      checks.push(
        `    if (value.size() < ${validation.minLength}) return std::string(${JSON.stringify(`${qualified} must be at least ${validation.minLength} characters`)});`,
      );
    }
    if (validation.maxLength !== undefined) {
      checks.push(
        `    if (value.size() > ${validation.maxLength}) return std::string(${JSON.stringify(`${qualified} must be at most ${validation.maxLength} characters`)});`,
      );
    }
    if (validation.regex) {
      checks.push(
        `    if (!std::regex_match(value, std::regex(R"RX(${validation.regex})RX"))) return std::string(${JSON.stringify(`${qualified} does not match the required pattern`)});`,
      );
    }
    if (checks.length === 0) {
      return [];
    }
    return [
      `inline std::optional<std::string> ${fnName}(const std::string& value) {`,
      ...checks,
      '    return std::nullopt;',
      '}',
    ];
  }
  if (column.kind === 'integer' || column.kind === 'bigint') {
    if (validation.min !== undefined) {
      checks.push(
        `    if (value < ${validation.min}) return std::string(${JSON.stringify(`${qualified} is below the minimum`)});`,
      );
    }
    if (validation.max !== undefined) {
      checks.push(
        `    if (value > ${validation.max}) return std::string(${JSON.stringify(`${qualified} is above the maximum`)});`,
      );
    }
    if (checks.length === 0) {
      return [];
    }
    return [
      `inline std::optional<std::string> ${fnName}(${cppBaseType(column)} value) {`,
      ...checks,
      '    return std::nullopt;',
      '}',
    ];
  }
  return [];
}

function renderCppCMake() {
  return `${[
    '# Generated by @dd/pg-defs. Do not edit by hand. schema/schema.sql is the source of truth.',
    'cmake_minimum_required(VERSION 3.14)',
    'project(dd_pg_defs CXX)',
    'add_library(dd_pg_defs INTERFACE)',
    'target_include_directories(dd_pg_defs INTERFACE ${CMAKE_CURRENT_SOURCE_DIR})',
    'target_compile_features(dd_pg_defs INTERFACE cxx_std_17)',
  ].join('\n')}\n`;
}

function renderZig(contract) {
  const lines = [
    ...generatedNotice('//'),
    'const std = @import("std");',
    '',
    '// A row reader the caller wires to its Postgres driver. `text`/`int`/`boolean` return the',
    '// column at a 0-based index (pairs with `<table>_select_sql`, which casts uuid/timestamp/jsonb',
    '// to text); `is_null` reports SQL NULL. Booleans arrive as Postgres text "t"/"f".',
    'pub const RowReader = struct {',
    '    text: *const fn (usize) []const u8,',
    '    int: *const fn (usize) i64,',
    '    float: *const fn (usize) f64,',
    '    boolean: *const fn (usize) bool,',
    '    is_null: *const fn (usize) bool,',
    '};',
  ];

  for (const table of contract.tables) {
    const structName = `${table.names?.rust ?? pascal(table.name)}Row`;
    const fnBase = camel(table.name);
    lines.push('');
    if (table.description) {
      lines.push(`// ${table.description}`);
    }
    lines.push(`pub const ${table.name}_table: []const u8 = ${JSON.stringify(physicalName(table))};`);
    lines.push(
      `pub const ${table.name}_columns = [_][]const u8{ ${table.columns.map((column) => JSON.stringify(column.name)).join(', ')} };`,
    );
    lines.push(`pub const ${table.name}_select_sql: []const u8 = ${JSON.stringify(renderSelectSql(table, { jsonAsText: true }))};`);

    for (const column of table.columns.filter((item) => item.kind === 'enum')) {
      const enumName = `${table.names?.rust ?? pascal(table.name)}${pascal(column.name)}`;
      lines.push('');
      lines.push(`pub const ${enumName} = enum {`);
      for (const value of column.enumValues) {
        lines.push(`    ${zigIdent(value)},`);
      }
      lines.push('');
      lines.push(`    pub fn toString(self: ${enumName}) []const u8 {`);
      lines.push('        return switch (self) {');
      for (const value of column.enumValues) {
        lines.push(`            .${zigIdent(value)} => ${JSON.stringify(value)},`);
      }
      lines.push('        };');
      lines.push('    }');
      lines.push('');
      lines.push(`    pub fn parse(value: []const u8) ?${enumName} {`);
      for (const value of column.enumValues) {
        lines.push(`        if (std.mem.eql(u8, value, ${JSON.stringify(value)})) return .${zigIdent(value)};`);
      }
      lines.push('        return null;');
      lines.push('    }');
      lines.push('};');
    }

    lines.push('');
    lines.push(`pub const ${structName} = struct {`);
    for (const column of table.columns) {
      lines.push(`    ${zigIdent(column.name)}: ${zigType(column)},`);
    }
    lines.push('');
    lines.push(`    pub fn fromRow(reader: RowReader) ${structName} {`);
    lines.push(`        return ${structName}{`);
    table.columns.forEach((column, index) => {
      lines.push(`            .${zigIdent(column.name)} = ${zigRowDecode(column, index)},`);
    });
    lines.push('        };');
    lines.push('    }');
    lines.push('};');

    for (const column of table.columns) {
      const validator = zigValidator(table, fnBase, column);
      if (validator.length > 0) {
        lines.push('');
        lines.push(...validator);
      }
    }
  }

  return `${lines.join('\n').trimEnd()}\n`;
}

const ZIG_KEYWORDS = new Set([
  'addrspace', 'align', 'allowzero', 'and', 'anyframe', 'anytype', 'asm', 'async', 'await', 'break',
  'callconv', 'catch', 'comptime', 'const', 'continue', 'defer', 'else', 'enum', 'errdefer', 'error',
  'export', 'extern', 'fn', 'for', 'if', 'inline', 'linksection', 'noalias', 'noinline', 'nosuspend',
  'opaque', 'or', 'orelse', 'packed', 'pub', 'resume', 'return', 'struct', 'suspend', 'switch', 'test',
  'threadlocal', 'try', 'union', 'unreachable', 'usingnamespace', 'var', 'volatile', 'while',
]);

function zigIdent(name) {
  return /^[A-Za-z_][A-Za-z0-9_]*$/.test(name) && !ZIG_KEYWORDS.has(name)
    ? name
    : `@${JSON.stringify(name)}`;
}

function zigType(column) {
  let base;
  switch (column.kind) {
    case 'integer':
      base = 'i32';
      break;
    case 'bigint':
      base = 'i64';
      break;
    case 'float':
      base = column.sqlType === 'real' ? 'f32' : 'f64';
      break;
    case 'boolean':
      base = 'bool';
      break;
    default:
      base = '[]const u8';
      break;
  }
  return column.notNull ? base : `?${base}`;
}

function zigRowDecode(column, index) {
  let expr;
  switch (column.kind) {
    case 'integer':
      expr = `@as(i32, @intCast(reader.int(${index})))`;
      break;
    case 'bigint':
      expr = `reader.int(${index})`;
      break;
    case 'float':
      expr = column.sqlType === 'real'
        ? `@as(f32, @floatCast(reader.float(${index})))`
        : `reader.float(${index})`;
      break;
    case 'boolean':
      expr = `reader.boolean(${index})`;
      break;
    default:
      expr = `reader.text(${index})`;
      break;
  }
  if (!column.notNull) {
    return `if (reader.is_null(${index})) null else ${expr}`;
  }
  return expr;
}

function zigValidator(table, fnBase, column) {
  const validation = column.validation ?? {};
  const fnName = `validate${pascal(fnBase)}${pascal(column.name)}`;
  const qualified = `${table.name}.${column.name}`;
  const checks = [];
  if (column.kind === 'string') {
    if (validation.minLength !== undefined) {
      checks.push(
        `    if (value.len < ${validation.minLength}) return ${JSON.stringify(`${qualified} must be at least ${validation.minLength} characters`)};`,
      );
    }
    if (validation.maxLength !== undefined) {
      checks.push(
        `    if (value.len > ${validation.maxLength}) return ${JSON.stringify(`${qualified} must be at most ${validation.maxLength} characters`)};`,
      );
    }
    if (checks.length === 0) {
      return [];
    }
    return [`pub fn ${fnName}(value: []const u8) ?[]const u8 {`, ...checks, '    return null;', '}'];
  }
  if (column.kind === 'integer' || column.kind === 'bigint') {
    const zigInt = column.kind === 'bigint' ? 'i64' : 'i32';
    if (validation.min !== undefined) {
      checks.push(
        `    if (value < ${validation.min}) return ${JSON.stringify(`${qualified} is below the minimum`)};`,
      );
    }
    if (validation.max !== undefined) {
      checks.push(
        `    if (value > ${validation.max}) return ${JSON.stringify(`${qualified} is above the maximum`)};`,
      );
    }
    if (checks.length === 0) {
      return [];
    }
    return [`pub fn ${fnName}(value: ${zigInt}) ?[]const u8 {`, ...checks, '    return null;', '}'];
  }
  return [];
}

function renderZigBuild() {
  return `${[
    '// Generated by @dd/pg-defs. Do not edit by hand. schema/schema.sql is the source of truth.',
    'const std = @import("std");',
    '',
    'pub fn build(b: *std.Build) void {',
    '    const target = b.standardTargetOptions(.{});',
    '    const optimize = b.standardOptimizeOption(.{});',
    '    _ = b.addModule("dd_pg_defs", .{',
    '        .root_source_file = b.path("dd_pg_defs.zig"),',
    '        .target = target,',
    '        .optimize = optimize,',
    '    });',
    '}',
  ].join('\n')}\n`;
}

function renderSequelize(contract) {
  const lines = [
    ...generatedNotice('//'),
    'import { DataTypes, Sequelize } from "sequelize";',
    '',
    'export function defineDdModels(sequelize: Sequelize) {',
  ];
  const modelNames = [];
  for (const table of contract.tables) {
    const modelName = table.names?.rust ?? pascal(table.name);
    modelNames.push(modelName);
    lines.push('');
    if (table.description) {
      lines.push(`  // ${table.description}`);
    }
    lines.push(`  const ${modelName} = sequelize.define(${JSON.stringify(modelName)}, {`);
    for (const column of table.columns) {
      lines.push(`    ${sequelizeAttribute(column)}`);
    }
    const options = [`tableName: ${JSON.stringify(table.name)}`];
    if (table.schema && table.schema !== 'public') {
      options.push(`schema: ${JSON.stringify(table.schema)}`);
    }
    options.push('timestamps: false', 'freezeTableName: true');
    lines.push(`  }, { ${options.join(', ')} });`);
  }
  lines.push('');
  lines.push(`  return { ${modelNames.join(', ')} };`);
  lines.push('}');
  return `${lines.join('\n')}\n`;
}

function sequelizeAttribute(column) {
  const parts = [`type: ${sequelizeDataType(column)}`, `allowNull: ${column.notNull ? 'false' : 'true'}`];
  if (column.primaryKey) {
    parts.push('primaryKey: true');
  }
  if (column.primaryKey && column.sqlType === 'uuid') {
    parts.push('defaultValue: DataTypes.UUIDV4');
  } else if (column.defaultValue !== undefined) {
    parts.push(`defaultValue: ${sequelizeDefault(column.defaultValue)}`);
  }
  const validate = sequelizeValidate(column);
  if (validate) {
    parts.push(`validate: { ${validate} }`);
  }
  return `${column.name}: { ${parts.join(', ')} },`;
}

function sequelizeDataType(column) {
  switch (column.kind) {
    case 'uuid':
      return 'DataTypes.UUID';
    case 'integer':
      return column.sqlType === 'smallint' ? 'DataTypes.SMALLINT' : 'DataTypes.INTEGER';
    case 'bigint':
      return 'DataTypes.BIGINT';
    case 'float':
      return column.sqlType === 'real' ? 'DataTypes.REAL' : 'DataTypes.DOUBLE';
    case 'boolean':
      return 'DataTypes.BOOLEAN';
    case 'timestamp':
      return 'DataTypes.DATE';
    case 'jsonObject':
    case 'jsonArray':
      return 'DataTypes.JSONB';
    case 'enum':
      return column.maxLength ? `DataTypes.STRING(${column.maxLength})` : 'DataTypes.TEXT';
    default:
      return column.sqlType === 'varchar' && column.maxLength
        ? `DataTypes.STRING(${column.maxLength})`
        : 'DataTypes.TEXT';
  }
}

function sequelizeDefault(value) {
  if (value === true) return 'true';
  if (value === false) return 'false';
  if (typeof value === 'number') return String(value);
  if (Array.isArray(value)) return '[]';
  if (value && typeof value === 'object') return '{}';
  return JSON.stringify(value);
}

function sequelizeValidate(column) {
  const validation = column.validation ?? {};
  const items = [];
  if (column.kind === 'enum') {
    items.push(`isIn: [[${column.enumValues.map((value) => JSON.stringify(value)).join(', ')}]]`);
  }
  if (column.kind === 'string') {
    if (validation.maxLength !== undefined) {
      items.push(`len: [${validation.minLength ?? 0}, ${validation.maxLength}]`);
    }
    if (validation.regex) {
      items.push(`is: new RegExp(${JSON.stringify(validation.regex)})`);
    }
  }
  if (column.kind === 'integer' || column.kind === 'bigint') {
    if (validation.min !== undefined) {
      items.push(`min: ${validation.min}`);
    }
    if (validation.max !== undefined) {
      items.push(`max: ${validation.max}`);
    }
  }
  return items.join(', ');
}

function renderPeewee(contract) {
  const lines = [
    ...generatedNotice('#'),
    'from peewee import (',
    '    AutoField,',
    '    BigAutoField,',
    '    BigIntegerField,',
    '    BooleanField,',
    '    CharField,',
    '    DatabaseProxy,',
    '    DateTimeField,',
    '    IntegerField,',
    '    Model,',
    '    SmallIntegerField,',
    '    TextField,',
    '    UUIDField,',
    ')',
    'from playhouse.postgres_ext import BinaryJSONField',
    '',
    'database = DatabaseProxy()',
    '',
    '',
    'class BaseModel(Model):',
    '    class Meta:',
    '        database = database',
  ];

  for (const table of contract.tables) {
    const className = table.names?.rust ?? pascal(table.name);
    lines.push('');
    lines.push('');
    lines.push(`class ${className}(BaseModel):`);
    if (table.description) {
      lines.push(`    # ${table.description}`);
    }
    for (const column of table.columns) {
      lines.push(`    ${peeweeField(column)}`);
    }
    lines.push('');
    lines.push('    class Meta:');
    lines.push(`        table_name = ${pyString(table.name)}`);
    if (table.schema && table.schema !== 'public') {
      lines.push(`        schema = ${pyString(table.schema)}`);
    }
  }

  return `${lines.join('\n').trimEnd()}\n`;
}

function peeweeField(column) {
  const args = [];
  let fieldClass;
  if (column.primaryKey && column.sqlType === 'bigserial') {
    fieldClass = 'BigAutoField';
  } else {
    fieldClass = peeweeFieldClass(column);
  }
  if (fieldClass === 'CharField' && column.maxLength) {
    args.push(`max_length=${column.maxLength}`);
  }
  if (column.primaryKey) {
    args.push('primary_key=True');
  }
  if (!column.notNull) {
    args.push('null=True');
  }
  return `${column.name} = ${fieldClass}(${args.join(', ')})`;
}

function peeweeFieldClass(column) {
  switch (column.kind) {
    case 'uuid':
      return 'UUIDField';
    case 'integer':
      return column.sqlType === 'smallint' ? 'SmallIntegerField' : 'IntegerField';
    case 'bigint':
      return 'BigIntegerField';
    case 'float':
      return column.sqlType === 'real' ? 'FloatField' : 'DoubleField';
    case 'boolean':
      return 'BooleanField';
    case 'timestamp':
      return 'DateTimeField';
    case 'jsonObject':
    case 'jsonArray':
      return 'BinaryJSONField';
    case 'enum':
      return column.maxLength ? 'CharField' : 'TextField';
    default:
      return column.sqlType === 'varchar' && column.maxLength ? 'CharField' : 'TextField';
  }
}

function renderPeeweeInit() {
  return `${[...generatedNotice('#'), 'from .models import *  # noqa: F401,F403'].join('\n')}\n`;
}

function renderDoctrineEntityFiles(contract) {
  return contract.tables.map((table) => {
    const className = table.names?.rust ?? pascal(table.name);
    return [`generated/php-doctrine/src/${className}.php`, renderDoctrineEntityFile(table)];
  });
}

function renderDoctrineEntityFile(table) {
  const className = table.names?.rust ?? pascal(table.name);
  const lines = [
    '<?php',
    '',
    ...generatedNotice('//'),
    '',
    'declare(strict_types=1);',
    '',
    'namespace DdPgDefs\\Doctrine;',
    '',
    'use Doctrine\\ORM\\Mapping as ORM;',
    '',
  ];
  if (table.description) {
    lines.push(`/** ${table.description} */`);
  }
  lines.push('#[ORM\\Entity]');
  lines.push(
    table.schema && table.schema !== 'public'
      ? `#[ORM\\Table(name: ${phpString(table.name)}, schema: ${phpString(table.schema)})]`
      : `#[ORM\\Table(name: ${phpString(table.name)})]`,
  );
  lines.push(`class ${className}`);
  lines.push('{');
  table.columns.forEach((column, index) => {
    if (index > 0) {
      lines.push('');
    }
    lines.push(...doctrineProperty(column).map((line) => `    ${line}`));
  });
  lines.push('}');
  return `${lines.join('\n')}\n`;
}

function doctrineProperty(column) {
  const lines = [];
  const prop = camel(column.name);
  const columnOptions = [`type: ${phpString(doctrineType(column))}`];
  if ((column.sqlType === 'varchar' || column.kind === 'enum') && column.maxLength) {
    columnOptions.push(`length: ${column.maxLength}`);
  }
  if (prop !== column.name) {
    columnOptions.push(`name: ${phpString(column.name)}`);
  }
  if (!column.notNull) {
    columnOptions.push('nullable: true');
  }
  if (column.primaryKey) {
    lines.push('#[ORM\\Id]');
    lines.push(
      `#[ORM\\GeneratedValue(strategy: ${phpString(column.sqlType === 'bigserial' ? 'IDENTITY' : 'NONE')})]`,
    );
  }
  lines.push(`#[ORM\\Column(${columnOptions.join(', ')})]`);
  const phpType = column.notNull ? doctrinePhpType(column) : `?${doctrinePhpType(column)}`;
  lines.push(`public ${phpType} $${prop};`);
  return lines;
}

function doctrineType(column) {
  switch (column.kind) {
    case 'uuid':
      return 'guid';
    case 'integer':
      return column.sqlType === 'smallint' ? 'smallint' : 'integer';
    case 'bigint':
      return 'bigint';
    case 'float':
      return 'float';
    case 'boolean':
      return 'boolean';
    case 'timestamp':
      return 'datetimetz_immutable';
    case 'jsonObject':
    case 'jsonArray':
      return 'json';
    case 'enum':
      return 'string';
    default:
      return column.sqlType === 'text' ? 'text' : 'string';
  }
}

function doctrinePhpType(column) {
  switch (column.kind) {
    case 'integer':
      return 'int';
    case 'bigint':
      // Doctrine maps bigint to string in PHP to avoid 64-bit precision loss on 32-bit platforms.
      return 'string';
    case 'float':
      return 'float';
    case 'boolean':
      return 'bool';
    case 'timestamp':
      return '\\DateTimeImmutable';
    case 'jsonObject':
    case 'jsonArray':
      return 'array';
    default:
      return 'string';
  }
}

function renderDoctrineComposerJson() {
  return `${JSON.stringify(
    {
      name: 'dd/pg-defs-doctrine',
      description: 'Canonical remote Postgres schema definitions as Doctrine ORM entities.',
      type: 'library',
      license: 'proprietary',
      require: {
        php: '>=8.1',
        'doctrine/orm': '^2.15 || ^3.0',
      },
      autoload: {
        'psr-4': { 'DdPgDefs\\Doctrine\\': 'src/' },
      },
    },
    null,
    2,
  )}\n`;
}

function renderDdl(contract) {
  const blocks = [];
  for (const table of contract.tables) {
    blocks.push(renderCreateTable(table));
    for (const tableIndex of table.indexes ?? []) {
      blocks.push(renderCreateIndex(table, tableIndex));
    }
  }
  return `${blocks.join('\n\n')}\n`;
}

function renderCreateTable(table) {
  const definitions = [];
  for (const column of table.columns) {
    definitions.push(
      `  ${column.name} ${columnTypeSql(column)}${column.primaryKey ? ' primary key' : ''}${column.defaultSql ? ` default ${column.defaultSql}` : ''}${column.notNull && !column.primaryKey ? ' not null' : ''}`,
    );
  }
  for (const checkConstraint of table.checks ?? []) {
    definitions.push(`  constraint ${checkConstraint.name}\n    check (${checkConstraint.sql})`);
  }
  return `create table if not exists ${physicalName(table)} (\n${definitions.join(',\n')}\n);`;
}

function renderCreateIndex(table, tableIndex) {
  const unique = tableIndex.unique ? 'unique ' : '';
  const method = tableIndex.method ? ` using ${tableIndex.method}` : '';
  const columns = tableIndex.columns.map((column) => {
    if (typeof column === 'string') {
      return column;
    }
    return `${column.name}${column.order ? ` ${column.order}` : ''}`;
  });
  const where = tableIndex.where ? `\n  where ${tableIndex.where}` : '';
  return `create ${unique}index if not exists ${tableIndex.name}\n  on ${physicalName(table)}${method} (${columns.join(', ')})${where};`;
}

// Schema-qualified physical table name (e.g. `benefactor.benefactor_leads`). For the default
// `public` schema this returns the bare name, so existing tables emit byte-identically.
function physicalName(table) {
  return table.schema && table.schema !== 'public' ? `${table.schema}.${table.name}` : table.name;
}

function columnTypeSql(column) {
  if (column.sqlType === 'varchar') {
    return `varchar(${column.maxLength})`;
  }
  if (column.sqlType === 'double') {
    // The parser collapses `double precision` to the `double` token; re-expand for valid SQL.
    return 'double precision';
  }
  return column.sqlType;
}

function renderSelectSql(table, options = {}) {
  const columns = table.columns
    .map((column) => `      ${selectExpression(column, options)}`)
    .join(',\n');
  return `select\n${columns}\n    from ${physicalName(table)}`;
}

function selectExpression(column, options = {}) {
  if (column.kind === 'uuid') {
    return `${column.name}::text as ${column.name}`;
  }
  if (column.kind === 'timestamp') {
    return `to_char(${column.name} at time zone 'utc', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') as ${column.name}`;
  }
  if (column.kind === 'jsonObject' || column.kind === 'jsonArray') {
    if (options.jsonAsText) {
      return `${column.name}::text as ${column.name}_json`;
    }
    return `${column.name}`;
  }
  return column.name;
}

function camel(value) {
  const pascalValue = pascal(value);
  return `${pascalValue.charAt(0).toLowerCase()}${pascalValue.slice(1)}`;
}

function pascal(value) {
  return value
    .split(/[_-]/)
    .filter(Boolean)
    .map((part) => `${part.charAt(0).toUpperCase()}${part.slice(1)}`)
    .join('');
}

function screaming(value) {
  return value
    .replace(/-/g, '_')
    .replace(/([a-z])([A-Z])/g, '$1_$2')
    .toUpperCase();
}

function escapeTemplate(value) {
  // Backslashes first — a trailing `\` would otherwise escape the closing
  // backtick of the emitted template literal.
  return value
    .replace(/\\/g, '\\\\')
    .replace(/`/g, '\\`')
    .replace(/\$\{/g, '\\${');
}

function rustRawString(value) {
  // Grow the fence until it cannot appear in the value, so no input can
  // terminate the raw string early (same trick as dartRawRegexString).
  let fence = '###';
  while (value.includes(`"${fence}`)) {
    fence += '#';
  }
  return `r${fence}"${value}"${fence}`;
}

function goRawString(value) {
  if (!value.includes('`')) {
    return `\`${value}\``;
  }
  return JSON.stringify(value);
}

function pyString(value) {
  return JSON.stringify(value);
}

function pyValue(value) {
  if (value === null) {
    return 'None';
  }
  if (Array.isArray(value)) {
    return `[${value.map(pyValue).join(', ')}]`;
  }
  if (typeof value === 'object') {
    return `{${Object.entries(value)
      .map(([key, item]) => `${pyString(key)}: ${pyValue(item)}`)
      .join(', ')}}`;
  }
  if (typeof value === 'boolean') {
    return value ? 'True' : 'False';
  }
  if (typeof value === 'number') {
    return String(value);
  }
  return pyString(value);
}

function erlangBinary(value) {
  return `<<"${value.replace(/\\/g, '\\\\').replace(/"/g, '\\"')}">>`;
}

await main();
