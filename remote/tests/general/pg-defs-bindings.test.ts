import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { existsSync } from 'node:fs';
import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import test from 'node:test';

function findRepoRoot(): string {
  for (const candidate of [process.cwd(), resolve(process.cwd(), '..', '..')]) {
    if (existsSync(resolve(candidate, 'remote/libs/pg-defs/src/generate.mjs'))) {
      return candidate;
    }
  }

  throw new Error(`Unable to locate repo root from ${process.cwd()}`);
}

const repoRoot = findRepoRoot();
const generatorPath = resolve(repoRoot, 'remote/libs/pg-defs/src/generate.mjs');

async function readRepoFile(relativePath: string): Promise<string> {
  return readFile(resolve(repoRoot, relativePath), 'utf8');
}

const EXPECTED_OUTPUTS: ReadonlyArray<string> = [
  'remote/libs/pg-defs/generated/typescript/index.ts',
  'remote/libs/pg-defs/generated/typescript/drizzle.ts',
  'remote/libs/pg-defs/generated/typescript/typeorm.ts',
  'remote/libs/pg-defs/generated/prisma/schema.prisma',
  'remote/libs/pg-defs/generated/python/sqlalchemy_models.py',
  'remote/libs/pg-defs/generated/go/gorm/go.mod',
  'remote/libs/pg-defs/generated/go/gorm/pg_defs.go',
  'remote/libs/pg-defs/generated/go/bun/go.mod',
  'remote/libs/pg-defs/generated/go/bun/pg_defs.go',
  'remote/libs/pg-defs/generated/dart/pubspec.yaml',
  'remote/libs/pg-defs/generated/dart/lib/pg_defs.dart',
  'remote/libs/pg-defs/generated/rust/Cargo.toml',
  'remote/libs/pg-defs/generated/rust/src/lib.rs',
  'remote/libs/pg-defs/generated/rust/diesel/Cargo.toml',
  'remote/libs/pg-defs/generated/rust/diesel/src/lib.rs',
  'remote/libs/pg-defs/generated/rust/sea-orm/Cargo.toml',
  'remote/libs/pg-defs/generated/rust/sea-orm/src/lib.rs',
  'remote/libs/pg-defs/generated/gleam/gleam.toml',
  'remote/libs/pg-defs/generated/gleam/src/pg_defs.gleam',
  'remote/libs/pg-defs/generated/erlang/src/pg_defs.erl',
];

test('pg-defs ships generated bindings for every supported runtime', () => {
  for (const relativePath of EXPECTED_OUTPUTS) {
    const absolutePath = resolve(repoRoot, relativePath);
    assert.ok(
      existsSync(absolutePath),
      `expected pg-defs generated binding to exist at ${relativePath}; ` +
        `run \`node remote/libs/pg-defs/src/generate.mjs\` to regenerate.`,
    );
  }
});

test('pg-defs generator emits the exact set of files expected by the workflow', async () => {
  const generatorSource = await readFile(generatorPath, 'utf8');

  for (const relativePath of EXPECTED_OUTPUTS) {
    const tail = relativePath.slice('remote/libs/pg-defs/'.length);
    assert.ok(
      generatorSource.includes(`'${tail}'`),
      `generator does not emit ${tail}; renderOutputs() in ` +
        `remote/libs/pg-defs/src/generate.mjs must list every expected ` +
        `binding so the CI guard catches drift.`,
    );
  }

  for (const flavor of [
    'renderDrizzleTypeScript',
    'renderTypeOrmTypeScript',
    'renderPrisma',
    'renderPythonSqlAlchemy',
    'renderGoGorm',
    'renderGoBun',
    'renderDart',
    'renderRust',
    'renderDieselRust',
    'renderSeaOrmRust',
    'renderGleam',
    'renderErlang',
  ]) {
    assert.ok(
      generatorSource.includes(`function ${flavor}(`),
      `pg-defs generator is missing a renderer for ${flavor}`,
    );
  }
});

test('pg-defs --check passes (generated files in sync with schema.sql)', () => {
  // Re-runs the generator in memory and compares against the committed files.
  // Mirrors the .github/workflows/pg-defs-check.yml guard so the same drift
  // shows up locally before CI.
  const stdout = execFileSync('node', [generatorPath, '--check'], {
    cwd: repoRoot,
    encoding: 'utf8',
    stdio: ['ignore', 'pipe', 'pipe'],
  });
  assert.match(stdout, /pg-defs generated outputs are up to date\./);
});

test('generated Gleam package keeps the path-dep contract used by all gleam services', async () => {
  const gleamToml = await readRepoFile('remote/libs/pg-defs/generated/gleam/gleam.toml');
  assert.match(gleamToml, /name = "dd_pg_defs"/);
  assert.match(gleamToml, /target = "erlang"/);

  const consumers = [
    'remote/deployments/gleam-lambda-runner/gleam.toml',
    'remote/deployments/gleam-mcp-server/gleam.toml',
    'remote/deployments/gleamlang-server/gleam.toml',
  ];
  for (const consumer of consumers) {
    const toml = await readRepoFile(consumer);
    assert.match(
      toml,
      /dd_pg_defs = \{ path = "\.\.\/\.\.\/libs\/pg-defs\/generated\/gleam" \}/,
      `${consumer} must consume dd_pg_defs via the local path dep so the gleam package compiles inside the EC2 repo mount.`,
    );
  }
});

test('gleam services expose a single pg_contract module rather than hand-rolling SQL', async () => {
  const mcpContract = await readRepoFile(
    'remote/deployments/gleam-mcp-server/src/gleam_mcp_server/pg_contract.gleam',
  );
  assert.match(mcpContract, /import pg_defs/);
  assert.match(mcpContract, /pg_defs\.app_config_select_sql/);
  assert.match(mcpContract, /pg_defs\.lambda_functions_select_sql/);

  const mcpMain = await readRepoFile('remote/deployments/gleam-mcp-server/src/gleam_mcp_server.gleam');
  assert.match(mcpMain, /import gleam_mcp_server\/pg_contract/);
  assert.match(mcpMain, /pg_contract\.app_config_table\(\)/);

  const wsContract = await readRepoFile(
    'remote/deployments/gleamlang-server/src/gleamlang_server/pg_contract.gleam',
  );
  assert.match(wsContract, /import pg_defs/);
  assert.match(wsContract, /pg_defs\.app_config_select_sql/);

  const wsMain = await readRepoFile('remote/deployments/gleamlang-server/src/gleamlang_server.gleam');
  assert.match(wsMain, /import gleamlang_server\/pg_contract/);
  assert.match(wsMain, /pg_contract\.app_config_table\(\)/);

  const lambdaContract = await readRepoFile(
    'remote/deployments/gleam-lambda-runner/src/gleam_lambda_runner/pg_contract.gleam',
  );
  assert.match(lambdaContract, /import pg_defs/);
  assert.match(lambdaContract, /pg_defs\.lambda_functions_select_sql/);
});

test('every gleam runtime deployment can reach RDS Postgres via an optional secret env', async () => {
  const services: ReadonlyArray<{
    name: string;
    ec2: string;
    secret: string;
    primaryUrlKey: string;
  }> = [
    {
      name: 'dd-gleam-lambda-runner',
      ec2: 'remote/deployments/gleam-lambda-runner/k8s/ec2/dd-gleam-lambda-runner.deployment.yaml',
      secret: 'dd-gleam-lambda-runner-secrets',
      primaryUrlKey: 'LAMBDA_DATABASE_URL',
    },
    {
      name: 'dd-gleam-mcp-server',
      ec2: 'remote/deployments/gleam-mcp-server/k8s/ec2/dd-gleam-mcp-server.deployment.yaml',
      secret: 'dd-gleam-mcp-server-secrets',
      primaryUrlKey: 'RDS_DATABASE_URL',
    },
    {
      name: 'dd-gleamlang-server',
      ec2: 'remote/deployments/gleamlang-server/k8s/ec2/dd-gleamlang-server.deployment.yaml',
      secret: 'dd-gleamlang-server-secrets',
      primaryUrlKey: 'RDS_DATABASE_URL',
    },
  ];

  for (const service of services) {
    const ec2 = await readRepoFile(service.ec2);
    assert.match(
      ec2,
      new RegExp(`name:\\s*${service.primaryUrlKey}[\\s\\S]*key:\\s*${service.primaryUrlKey}`),
      `${service.name} ec2 deployment must wire ${service.primaryUrlKey} from ${service.secret}`,
    );
    assert.match(
      ec2,
      new RegExp(`secretRef:[\\s\\S]*name:\\s*${service.secret}[\\s\\S]*optional:\\s*true`),
      `${service.name} ec2 deployment must envFrom secret ${service.secret} (optional)`,
    );
    if (service.name === 'dd-gleamlang-server') {
      assert.match(
        ec2,
        /name:\s*PG_DATABASE_URL[\s\S]*name:\s*dd-gleamlang-server-secrets[\s\S]*key:\s*RDS_DATABASE_URL/,
        'dd-gleamlang-server ec2 deployment must map PG_DATABASE_URL from RDS_DATABASE_URL for sharded LISTEN/NOTIFY',
      );
      assert.match(ec2, /name:\s*PRESENCE_NOTIFY_SHARDS[\s\S]*value:\s*"256"/);
      assert.match(ec2, /name:\s*PRESENCE_WAL_ENABLED[\s\S]*value:\s*"true"/);
    }
  }
});

test('each gleam runtime secret has an ExternalSecret backing it', async () => {
  const externalSecrets = await readRepoFile('remote/argocd/secrets/external-secrets.yaml');
  for (const secret of [
    'dd-gleam-lambda-runner-secrets',
    'dd-gleam-mcp-server-secrets',
    'dd-gleamlang-server-secrets',
  ]) {
    assert.match(
      externalSecrets,
      new RegExp(`name:\\s*${secret}`),
      `${secret} must be created by an ExternalSecret in remote/argocd/secrets/external-secrets.yaml so RDS_DATABASE_URL et al. flow from AWS Secrets Manager.`,
    );
  }
});

test('presence LISTEN/NOTIFY deployment receives a narrow RDS URL secret', async () => {
  const statefulSet = await readRepoFile(
    'remote/deployments/gleamlang-presence-server/k8s/40-statefulset.yaml',
  );
  const externalSecret = await readRepoFile(
    'remote/deployments/gleamlang-presence-server/k8s/25-postgres-externalsecret.yaml',
  );

  assert.match(statefulSet, /name:\s*PG_DATABASE_URL[\s\S]*name:\s*presence-pg[\s\S]*key:\s*url/);
  assert.match(statefulSet, /name:\s*PRESENCE_NOTIFY_SHARDS[\s\S]*value:\s*"256"/);
  assert.match(externalSecret, /kind:\s*ExternalSecret/);
  assert.match(externalSecret, /name:\s*presence-pg/);
  assert.match(externalSecret, /namespace:\s*presence/);
  assert.match(externalSecret, /secretKey:\s*url/);
  assert.match(externalSecret, /key:\s*dd\/remote-dev\/rest-api-secrets/);
  assert.match(externalSecret, /property:\s*AGENT_TASKS_RDS_DATABASE_URL/);
});
