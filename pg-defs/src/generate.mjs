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

  if (args.has('--check')) {
    const stale = [];
    for (const [relativePath, contents] of outputs) {
      const absolutePath = path.join(packageRoot, relativePath);
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
    const absolutePath = path.join(packageRoot, relativePath);
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
  pgCoreImports.push('pgTable', 'text', 'timestamp', 'uniqueIndex', 'uuid', 'varchar');

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

    lines.push(`@Entity({ name: ${JSON.stringify(table.name)} })`);
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
  return column.sqlType;
}

function typeScriptType(column) {
  let baseType;
  switch (column.kind) {
    case 'integer':
    case 'bigint':
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
    'from sqlalchemy import BigInteger, Boolean, CheckConstraint, DateTime, Index, Integer, String, Text, text',
    'from sqlalchemy.dialects.postgresql import JSONB, UUID as PgUUID',
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
  if (column.kind === 'integer' || column.kind === 'bigint') {
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
    case 'integer':
      builder = `integer(${JSON.stringify(column.name)})`;
      break;
    case 'bigint':
      builder = `bigint(${JSON.stringify(column.name)}, { mode: "number" })`;
      break;
    case 'bigserial':
      builder = `bigserial(${JSON.stringify(column.name)}, { mode: "number" })`;
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
    case 'integer':
      return 'Integer()';
    case 'bigint':
    case 'bigserial':
      return 'BigInteger()';
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
  if (column.kind === 'integer') {
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
    if (column.kind === 'integer' || column.kind === 'bigint') {
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
    if (column.kind === 'boolean') {
      return `_readOptionalBool(json, ${key})`;
    }
    return `_readOptionalString(json, ${key})`;
  }
  switch (column.kind) {
    case 'integer':
    case 'bigint':
      return `_readRequiredInt(json, ${key})`;
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

function renderRustEnum(baseName, column) {
  const enumName = `${baseName}${pascal(column.name)}`;
  const lines = [
    '#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]',
    '#[serde(rename_all = "lowercase")]',
    `pub enum ${enumName} {`,
  ];
  for (const value of column.enumValues) {
    lines.push(`    ${pascal(value)},`);
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
    lines.push(`            Self::${pascal(value)} => ${JSON.stringify(value)},`);
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
    lines.push(`            ${JSON.stringify(value)} => Ok(Self::${pascal(value)}),`);
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
  switch (column.kind) {
    case 'integer':
      return 'i32';
    case 'bigint':
      return 'i64';
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
    'diesel = { version = "2", features = ["postgres", "uuid", "serde_json", "chrono"] }',
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
    'sea-orm = { version = "1", features = ["macros", "with-uuid", "with-json", "with-chrono"] }',
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
    lines.push(`#[sea_orm(table_name = ${JSON.stringify(table.name)})]`);
    lines.push('pub struct Model {');
    for (const column of table.columns) {
      const attrs = [];
      if (column.primaryKey) {
        attrs.push('primary_key');
        if (column.sqlType === 'uuid') {
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
    case 'integer':
      baseType = 'Int4';
      break;
    case 'bigint':
    case 'bigserial':
      baseType = 'Int8';
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
  switch (column.kind) {
    case 'uuid':
      return 'Uuid';
    case 'integer':
      return 'i32';
    case 'bigint':
      return 'i64';
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
  if (column.kind === 'integer' || column.kind === 'bigint') {
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
    } else if (column.kind === 'integer' || column.kind === 'bigint') {
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

    blocks.push(`-- name: List${baseName} :many`);
    blocks.push(`select ${columnList} from ${table.name};`);
    blocks.push('');
    blocks.push(`-- name: Get${baseName} :one`);
    const idColumn = table.columns.find((column) => column.primaryKey)?.name ?? 'id';
    blocks.push(`select ${columnList} from ${table.name} where ${idColumn} = $1 limit 1;`);
    blocks.push('');
    blocks.push(`-- name: Create${baseName} :one`);
    blocks.push(
      `insert into ${table.name} (${columnList}) values (${placeholders}) returning ${columnList};`,
    );
    blocks.push('');
    if (updatableColumns.length > 0) {
      blocks.push(`-- name: Update${baseName} :one`);
      blocks.push(
        `update ${table.name} set ${updateAssignments} where ${idColumn} = $1 returning ${columnList};`,
      );
      blocks.push('');
    }
    blocks.push(`-- name: Delete${baseName} :exec`);
    blocks.push(`delete from ${table.name} where ${idColumn} = $1;`);
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
    lines.push(`    public static final Name ${tableConst}_NAME = DSL.name(${JSON.stringify(table.name)});`);
    lines.push(`    public static final Table<org.jooq.Record> ${tableConst} = DSL.table(${tableConst}_NAME);`);
    for (const column of table.columns) {
      const colConst = `${tableConst}_${screaming(column.name)}`;
      const javaType = jooqJavaType(column);
      const dataType = jooqDataType(column);
      lines.push(
        `    public static final Field<${javaType}> ${colConst} = DSL.field(DSL.name(${JSON.stringify(table.name)}, ${JSON.stringify(column.name)}), ${dataType});`,
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
    case 'integer':
      return 'SQLDataType.INTEGER';
    case 'bigint':
    case 'bigserial':
      return 'SQLDataType.BIGINT';
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
    `@Table(name = ${JSON.stringify(table.name)})`,
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
  return value.replace(/`/g, '\\`').replace(/\$\{/g, '\\${');
}

function rustRawString(value) {
  return `r###"${value}"###`;
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
