#!/usr/bin/env node
import { spawnSync } from 'node:child_process';
import { existsSync } from 'node:fs';
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

import { loadSqlContract } from '../../../remote/libs/pg-defs/src/sql-contract.mjs';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = findRepoRoot();

const DEFAULT_SCHEMAS = ['public'];
const DATABASE_URL_ENV_KEYS = [
  'AGENT_TASKS_RDS_DATABASE_URL',
  'RDS_DATABASE_URL',
  'AGENT_TASKS_DATABASE_URL',
  'DATABASE_URL',
];

async function main() {
  const args = parseArgs(process.argv.slice(2));
  if (args.help) {
    process.stdout.write(helpText());
    return;
  }

  const schemas = parseSchemas(args.schemas);
  const live = args.fromLiveJson
    ? JSON.parse(await readFile(path.resolve(repoRoot, args.fromLiveJson), 'utf8'))
    : introspectLiveRds(databaseUrl(args), schemas);

  const pgDefsRoot = path.resolve(repoRoot, 'remote/libs/pg-defs');
  const { contract, schemaPath } = await loadSqlContract(pgDefsRoot);
  const report = buildReport({
    desired: contract,
    live,
    schemaPath: path.relative(repoRoot, schemaPath).split(path.sep).join('/'),
    schemas,
  });

  const format = args.format ?? 'text';
  const output = renderReport(report, format);
  await writeOutput(args.output, output);

  if (args.check && !report.ok) {
    process.exitCode = 1;
  }
}

function findRepoRoot() {
  for (const candidate of [process.cwd(), path.resolve(__dirname, '../../..')]) {
    if (existsSync(path.resolve(candidate, 'remote/libs/pg-defs/schema/schema.sql'))) {
      return candidate;
    }
  }
  throw new Error(`Unable to locate repo root from ${process.cwd()}`);
}

function parseArgs(rawArgs) {
  const args = {
    schemas: DEFAULT_SCHEMAS,
    format: 'text',
    output: '-',
    check: false,
    help: false,
  };
  for (let index = 0; index < rawArgs.length; index += 1) {
    const arg = rawArgs[index];
    const [flag, inlineValue] = arg.split('=', 2);
    const nextValue = () => inlineValue ?? rawArgs[++index];
    switch (flag) {
      case '--database-url':
        args.databaseUrl = nextValue();
        break;
      case '--from-live-json':
        args.fromLiveJson = nextValue();
        break;
      case '--schema':
        args.schemas = nextValue();
        break;
      case '--format':
        args.format = nextValue();
        break;
      case '--output':
        args.output = nextValue();
        break;
      case '--check':
        args.check = true;
        break;
      case '--help':
      case '-h':
        args.help = true;
        break;
      default:
        throw new Error(`Unknown argument: ${arg}`);
    }
  }
  return args;
}

function parseSchemas(value) {
  const schemas = (Array.isArray(value) ? value : String(value ?? 'public').split(','))
    .map((item) => String(item).trim())
    .filter(Boolean);
  if (schemas.length === 0) {
    throw new Error('At least one schema must be selected.');
  }
  for (const schema of schemas) {
    if (!/^[A-Za-z_][A-Za-z0-9_]*$/.test(schema)) {
      throw new Error(`Unsafe schema identifier: ${schema}`);
    }
  }
  return schemas;
}

function databaseUrl(args) {
  if (args.databaseUrl) {
    return args.databaseUrl;
  }
  for (const key of DATABASE_URL_ENV_KEYS) {
    const value = process.env[key]?.trim();
    if (value) {
      return value;
    }
  }
  throw new Error(
    `Missing database URL. Set one of ${DATABASE_URL_ENV_KEYS.join(', ')} or pass --database-url.`,
  );
}

function introspectLiveRds(url, schemas) {
  const result = spawnSync(
    'psql',
    [
      url,
      '-X',
      '-q',
      '-t',
      '-A',
      '-v',
      'ON_ERROR_STOP=1',
      '-P',
      'pager=off',
      '-c',
      introspectionSql(schemas),
    ],
    {
      cwd: repoRoot,
      encoding: 'utf8',
      timeout: 60_000,
      maxBuffer: 20 * 1024 * 1024,
    },
  );
  if (result.error) {
    throw new Error(`Failed to launch psql for read-only RDS introspection: ${result.error.message}`);
  }
  if (result.status !== 0) {
    throw new Error(
      `Read-only RDS introspection failed with exit ${result.status}.\n${result.stderr.trim()}`,
    );
  }
  try {
    return JSON.parse(result.stdout.trim());
  } catch (error) {
    throw new Error(`RDS introspection did not return JSON: ${error.message}`);
  }
}

function introspectionSql(schemas) {
  const schemaArray = schemas.map((schema) => sqlString(schema)).join(', ');
  return `
with selected_schemas as (
  select unnest(array[${schemaArray}]::text[]) as schema_name
),
tables as (
  select
    t.table_schema,
    t.table_name,
    t.table_type,
    coalesce((
      select jsonb_agg(jsonb_build_object(
        'name', c.column_name,
        'ordinalPosition', c.ordinal_position,
        'dataType', c.data_type,
        'udtName', c.udt_name,
        'maxLength', c.character_maximum_length,
        'isNullable', c.is_nullable = 'YES',
        'default', c.column_default
      ) order by c.ordinal_position)
      from information_schema.columns c
      where c.table_schema = t.table_schema and c.table_name = t.table_name
    ), '[]'::jsonb) as columns,
    coalesce((
      select jsonb_agg(a.attname order by a.attnum)
      from pg_index i
      join pg_class cls on cls.oid = i.indrelid
      join pg_namespace ns on ns.oid = cls.relnamespace
      join pg_attribute a on a.attrelid = cls.oid and a.attnum = any(i.indkey)
      where ns.nspname = t.table_schema and cls.relname = t.table_name and i.indisprimary
    ), '[]'::jsonb) as primary_key,
    coalesce((
      select jsonb_agg(jsonb_build_object(
        'name', indexname,
        'definition', indexdef
      ) order by indexname)
      from pg_indexes i
      where i.schemaname = t.table_schema and i.tablename = t.table_name
    ), '[]'::jsonb) as indexes
  from information_schema.tables t
  join selected_schemas s on s.schema_name = t.table_schema
  where t.table_type in ('BASE TABLE', 'VIEW')
),
routines as (
  select
    n.nspname as schema_name,
    p.proname as routine_name,
    pg_get_function_identity_arguments(p.oid) as identity_arguments,
    md5(pg_get_functiondef(p.oid)) as definition_md5
  from pg_proc p
  join pg_namespace n on n.oid = p.pronamespace
  join selected_schemas s on s.schema_name = n.nspname
),
triggers as (
  select
    event_object_schema as schema_name,
    event_object_table as table_name,
    trigger_name,
    action_timing,
    event_manipulation,
    action_orientation
  from information_schema.triggers t
  join selected_schemas s on s.schema_name = t.event_object_schema
)
select jsonb_build_object(
  'source', 'rds-postgres',
  'schemas', (select jsonb_agg(schema_name order by schema_name) from selected_schemas),
  'generatedAt', now(),
  'tables', coalesce((select jsonb_agg(jsonb_build_object(
    'schema', table_schema,
    'name', table_name,
    'tableType', table_type,
    'columns', columns,
    'primaryKey', primary_key,
    'indexes', indexes
  ) order by table_schema, table_name) from tables), '[]'::jsonb),
  'routines', coalesce((select jsonb_agg(jsonb_build_object(
    'schema', schema_name,
    'name', routine_name,
    'identityArguments', identity_arguments,
    'definitionMd5', definition_md5
  ) order by schema_name, routine_name, identity_arguments) from routines), '[]'::jsonb),
  'triggers', coalesce((select jsonb_agg(jsonb_build_object(
    'schema', schema_name,
    'table', table_name,
    'name', trigger_name,
    'timing', lower(action_timing),
    'event', lower(event_manipulation),
    'orientation', lower(action_orientation)
  ) order by schema_name, table_name, trigger_name) from triggers), '[]'::jsonb)
)::text;
`;
}

function buildReport({ desired, live, schemaPath, schemas }) {
  const desiredTables = new Map(
    desired.tables.map((table) => [tableKey('public', table.name), normalizeDesiredTable(table)]),
  );
  const liveTables = new Map(
    (live.tables ?? []).map((table) => [tableKey(table.schema ?? 'public', table.name), normalizeLiveTable(table)]),
  );

  const diff = {
    missingTables: [],
    liveOnlyTables: [],
    missingColumns: [],
    liveOnlyColumns: [],
    columnDrift: [],
    primaryKeyDrift: [],
    missingIndexes: [],
    liveOnlyIndexes: [],
    missingRoutines: [],
    liveOnlyRoutines: [],
    missingTriggers: [],
    liveOnlyTriggers: [],
  };

  for (const [key, table] of desiredTables) {
    if (!schemas.includes(table.schema)) {
      continue;
    }
    const liveTable = liveTables.get(key);
    if (!liveTable) {
      diff.missingTables.push({ schema: table.schema, table: table.name });
      continue;
    }
    compareTable(table, liveTable, diff);
  }

  for (const [key, table] of liveTables) {
    if (!desiredTables.has(key)) {
      diff.liveOnlyTables.push({ schema: table.schema, table: table.name, tableType: table.tableType });
    }
  }

  compareNamedArtifacts({
    desiredItems: desired.routines ?? [],
    liveItems: live.routines ?? [],
    keyFn: (item) => `${item.schema ?? 'public'}.${item.name}(${item.identityArguments ?? item.identityArgumentsSql ?? item.identityArguments ?? ''})`,
    missingSink: diff.missingRoutines,
    liveOnlySink: diff.liveOnlyRoutines,
    labelFn: (item) => ({ schema: item.schema ?? 'public', name: item.name, identityArguments: item.identityArguments ?? '' }),
  });

  compareNamedArtifacts({
    desiredItems: desired.triggers ?? [],
    liveItems: live.triggers ?? [],
    keyFn: (item) => `${item.schema ?? 'public'}.${item.tableName ?? item.table}.${item.name}`,
    missingSink: diff.missingTriggers,
    liveOnlySink: diff.liveOnlyTriggers,
    labelFn: (item) => ({ schema: item.schema ?? 'public', table: item.tableName ?? item.table, name: item.name }),
  });

  const counts = Object.fromEntries(Object.entries(diff).map(([key, value]) => [key, value.length]));
  const driftCount = Object.values(counts).reduce((sum, value) => sum + value, 0);
  return {
    ok: driftCount === 0,
    generatedBy: 'scripts/pg/diff/rds-vs-pg-defs.mjs',
    policy: {
      desiredSource: schemaPath,
      actualSource: 'read-only RDS catalog introspection',
      generatesSql: false,
      generatedMigrationFiles: false,
      applyRequiresHumanOwnedManualMigration: true,
    },
    schemas,
    liveSource: live.source ?? 'rds-postgres',
    counts,
    diff,
  };
}

function normalizeDesiredTable(table) {
  return {
    schema: 'public',
    name: table.name,
    columns: new Map(table.columns.map((column) => [column.name, normalizeDesiredColumn(column)])),
    primaryKey: table.columns.filter((column) => column.primaryKey).map((column) => column.name).sort(),
    indexes: new Map((table.indexes ?? []).map((index) => [index.name, index])),
    tableType: 'BASE TABLE',
  };
}

function normalizeLiveTable(table) {
  return {
    schema: table.schema ?? 'public',
    name: table.name,
    columns: new Map((table.columns ?? []).map((column) => [column.name, normalizeLiveColumn(column)])),
    primaryKey: [...(table.primaryKey ?? table.primary_key ?? [])].sort(),
    indexes: new Map((table.indexes ?? []).map((index) => [index.name, index])),
    tableType: table.tableType ?? table.table_type ?? 'BASE TABLE',
  };
}

function normalizeDesiredColumn(column) {
  return {
    name: column.name,
    type: normalizeType({ sqlType: column.sqlType, maxLength: column.maxLength }),
    notNull: Boolean(column.notNull),
    default: normalizeDefault(column.defaultSql),
  };
}

function normalizeLiveColumn(column) {
  return {
    name: column.name,
    type: normalizeType({
      dataType: column.dataType ?? column.data_type,
      udtName: column.udtName ?? column.udt_name,
      maxLength: column.maxLength ?? column.characterMaximumLength ?? column.character_maximum_length,
    }),
    notNull: !Boolean(column.isNullable ?? column.is_nullable),
    default: normalizeDefault(column.default ?? column.columnDefault ?? column.column_default),
  };
}

function compareTable(desired, live, diff) {
  for (const [columnName, column] of desired.columns) {
    const liveColumn = live.columns.get(columnName);
    if (!liveColumn) {
      diff.missingColumns.push({ schema: desired.schema, table: desired.name, column: columnName });
      continue;
    }
    const drift = {};
    if (column.type !== liveColumn.type) {
      drift.type = { desired: column.type, live: liveColumn.type };
    }
    if (column.notNull !== liveColumn.notNull) {
      drift.notNull = { desired: column.notNull, live: liveColumn.notNull };
    }
    if (column.default !== liveColumn.default) {
      drift.default = { desired: column.default, live: liveColumn.default };
    }
    if (Object.keys(drift).length > 0) {
      diff.columnDrift.push({ schema: desired.schema, table: desired.name, column: columnName, drift });
    }
  }

  for (const columnName of live.columns.keys()) {
    if (!desired.columns.has(columnName)) {
      diff.liveOnlyColumns.push({ schema: live.schema, table: live.name, column: columnName });
    }
  }

  if (desired.primaryKey.join(',') !== live.primaryKey.join(',')) {
    diff.primaryKeyDrift.push({
      schema: desired.schema,
      table: desired.name,
      desired: desired.primaryKey,
      live: live.primaryKey,
    });
  }

  for (const indexName of desired.indexes.keys()) {
    if (!live.indexes.has(indexName)) {
      diff.missingIndexes.push({ schema: desired.schema, table: desired.name, index: indexName });
    }
  }
  for (const indexName of live.indexes.keys()) {
    if (!desired.indexes.has(indexName)) {
      diff.liveOnlyIndexes.push({ schema: live.schema, table: live.name, index: indexName });
    }
  }
}

function compareNamedArtifacts({ desiredItems, liveItems, keyFn, missingSink, liveOnlySink, labelFn }) {
  const desiredByKey = new Map(desiredItems.map((item) => [keyFn(item), item]));
  const liveByKey = new Map(liveItems.map((item) => [keyFn(item), item]));
  for (const [key, item] of desiredByKey) {
    if (!liveByKey.has(key)) {
      missingSink.push(labelFn(item));
    }
  }
  for (const [key, item] of liveByKey) {
    if (!desiredByKey.has(key)) {
      liveOnlySink.push(labelFn(item));
    }
  }
}

function normalizeType({ sqlType, dataType, udtName, maxLength }) {
  const raw = String(sqlType ?? dataType ?? udtName ?? '').toLowerCase().replace(/\s+/g, ' ').trim();
  if (!raw) {
    return '';
  }
  if (raw === 'character varying' || raw === 'varchar') {
    return maxLength ? `varchar(${maxLength})` : 'varchar';
  }
  if (raw === 'timestamp with time zone') {
    return 'timestamptz';
  }
  if (raw === 'timestamp without time zone') {
    return 'timestamp';
  }
  if (raw === 'int4') {
    return 'integer';
  }
  if (raw === 'int8') {
    return 'bigint';
  }
  if (raw === 'bool') {
    return 'boolean';
  }
  return raw.replace(/\s*\(\s*/g, '(').replace(/\s*\)\s*/g, ')');
}

function normalizeDefault(value) {
  if (value === null || value === undefined || value === '') {
    return null;
  }
  return String(value)
    .toLowerCase()
    .replace(/::[\w\s."]+/g, '')
    .replace(/\s+/g, ' ')
    .trim();
}

function tableKey(schema, table) {
  return `${schema}.${table}`;
}

function renderReport(report, format) {
  if (format === 'json') {
    return `${JSON.stringify(report, null, 2)}\n`;
  }
  if (format === 'markdown' || format === 'md') {
    return renderMarkdown(report);
  }
  if (format !== 'text') {
    throw new Error(`Unsupported --format ${format}; use text, json, or markdown.`);
  }
  return renderText(report);
}

function renderText(report) {
  const lines = [
    `RDS vs pg-defs drift: ${report.ok ? 'clean' : 'drift detected'}`,
    `Desired: ${report.policy.desiredSource}`,
    'Actual: read-only RDS catalog introspection',
    'Policy: this tool does not generate SQL migration files.',
    '',
  ];
  for (const [key, count] of Object.entries(report.counts)) {
    lines.push(`${key}: ${count}`);
  }
  lines.push('');
  appendTextItems(lines, report.diff);
  return `${lines.join('\n')}\n`;
}

function renderMarkdown(report) {
  const lines = [
    `# RDS vs pg-defs drift report`,
    '',
    `- Status: **${report.ok ? 'clean' : 'drift detected'}**`,
    `- Desired: \`${report.policy.desiredSource}\``,
    '- Actual: read-only RDS catalog introspection',
    '- Policy: this tool does **not** generate SQL migration files.',
    '',
    '## Counts',
    '',
    ...Object.entries(report.counts).map(([key, count]) => `- ${key}: ${count}`),
    '',
    '## Details',
    '',
  ];
  appendTextItems(lines, report.diff);
  return `${lines.join('\n')}\n`;
}

function appendTextItems(lines, diff) {
  let wroteAny = false;
  for (const [section, items] of Object.entries(diff)) {
    if (items.length === 0) {
      continue;
    }
    wroteAny = true;
    lines.push(`${section}:`);
    for (const item of items.slice(0, 50)) {
      lines.push(`  - ${JSON.stringify(item)}`);
    }
    if (items.length > 50) {
      lines.push(`  - ... ${items.length - 50} more`);
    }
    lines.push('');
  }
  if (!wroteAny) {
    lines.push('No drift detected.');
  }
}

async function writeOutput(outputPath, contents) {
  if (!outputPath || outputPath === '-') {
    process.stdout.write(contents);
    return;
  }
  if (outputPath.toLowerCase().endsWith('.sql')) {
    throw new Error('Refusing to write a .sql file. This diff tool emits reports only, not migrations.');
  }
  const absolutePath = path.resolve(repoRoot, outputPath);
  await mkdir(path.dirname(absolutePath), { recursive: true });
  await writeFile(absolutePath, contents);
}

function sqlString(value) {
  return `'${String(value).replaceAll("'", "''")}'`;
}

function helpText() {
  return `Usage: node scripts/pg/diff/rds-vs-pg-defs.mjs [options]

Compare live RDS catalog state against remote/libs/pg-defs/schema/schema.sql.
This is declarative drift reporting only: it never generates .sql migration files.

Options:
  --database-url <url>     RDS Postgres URL. Defaults to AGENT_TASKS_RDS_DATABASE_URL,
                           RDS_DATABASE_URL, AGENT_TASKS_DATABASE_URL, or DATABASE_URL.
  --from-live-json <path>  Use a captured live catalog JSON fixture instead of connecting to RDS.
  --schema <names>         Comma-separated schema list. Default: public.
  --format <name>          text, json, or markdown. Default: text.
  --output <path|- >       Report output path. Refuses .sql. Default: stdout.
  --check                  Exit 1 when drift is detected.
  --help                   Show this help.
`;
}

main().catch((error) => {
  console.error(error instanceof Error ? error.message : error);
  process.exitCode = 1;
});
