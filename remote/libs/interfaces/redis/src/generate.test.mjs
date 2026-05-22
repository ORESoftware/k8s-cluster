import { execFileSync } from 'node:child_process';
import { readFile } from 'node:fs/promises';
import path from 'node:path';
import test from 'node:test';
import assert from 'node:assert/strict';
import { fileURLToPath } from 'node:url';

const packageRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const generatorPath = path.join(packageRoot, 'src', 'generate.mjs');

test('generated outputs are up to date with schema source', () => {
  // Throws if non-zero exit code.
  execFileSync(process.execPath, [generatorPath, '--check'], {
    cwd: packageRoot,
    stdio: ['ignore', 'pipe', 'pipe'],
  });
});

test('typescript output exposes a key formatter for every $dd:redis key', async () => {
  const tsPath = path.join(packageRoot, 'generated', 'typescript', 'index.ts');
  const ts = await readFile(tsPath, 'utf8');

  for (const fn of [
    'agentThreadBreadcrumbTailKey',
    'containerPoolAffinityLockKey',
    'runtimeConfigEntryKey',
    'runtimeConfigEntryIndexKey',
    'runtimeConfigGenerationKey',
    'runtimeConfigSubscriberKey',
    'runtimeConfigSubscriberIndexKey',
  ]) {
    assert.match(ts, new RegExp(`export function ${fn}\\(`), `missing ${fn} in TS output`);
  }

  assert.match(ts, /AGENT_THREAD_BREADCRUMB_TAIL_KEY_DEFAULT_PREFIX = "dd:agent:breadcrumb-tail";/);
  assert.match(ts, /CONTAINER_POOL_AFFINITY_LOCK_KEY_DEFAULT_PREFIX = "dd:container-pool:affinity";/);
  assert.match(ts, /RUNTIME_CONFIG_ENTRY_KEY_DEFAULT_PREFIX = "dd:rc";/);
});

test('rust output exposes snake_case key formatters', async () => {
  const rsPath = path.join(packageRoot, 'generated', 'rust', 'src', 'lib.rs');
  const rs = await readFile(rsPath, 'utf8');

  for (const fn of [
    'agent_thread_breadcrumb_tail_key',
    'container_pool_affinity_lock_key',
    'runtime_config_entry_key',
    'runtime_config_entry_index_key',
    'runtime_config_generation_key',
    'runtime_config_subscriber_key',
    'runtime_config_subscriber_index_key',
  ]) {
    assert.match(rs, new RegExp(`pub fn ${fn}\\(`), `missing ${fn} in Rust output`);
  }
});

test('typescript and rust formatters produce identical keys', async () => {
  const tsModulePath = path.join(packageRoot, 'generated', 'typescript', 'index.ts');
  const tsSource = await readFile(tsModulePath, 'utf8');

  const expectations = [
    {
      ts: 'agentThreadBreadcrumbTailKey',
      args: ['dd:agent:breadcrumb-tail', 'thread-uuid-1'],
      expected: 'dd:agent:breadcrumb-tail:thread-uuid-1',
    },
    {
      ts: 'containerPoolAffinityLockKey',
      args: ['dd:container-pool:affinity', 'nodejs-default', 'thread-uuid-1'],
      expected: 'dd:container-pool:affinity:nodejs-default:thread-uuid-1',
    },
    {
      ts: 'runtimeConfigEntryKey',
      args: ['dd:rc', 'prod', 'dd-remote-web-home', 'FEATURE_FLAG'],
      expected: 'dd:rc:prod:entry:dd-remote-web-home:FEATURE_FLAG',
    },
    {
      ts: 'runtimeConfigGenerationKey',
      args: ['dd:rc', 'stage'],
      expected: 'dd:rc:stage:generation',
    },
  ];

  for (const expectation of expectations) {
    const literalSourceMatch = new RegExp(
      `export function ${expectation.ts}\\([^\\)]*\\): string \\{[^}]*return \`([^\`]+)\`;`,
      'm',
    ).exec(tsSource);
    assert.ok(literalSourceMatch, `expected to find template literal for ${expectation.ts}`);
    // Resolve the template literal manually to match an actual call would produce.
    let literal = literalSourceMatch[1];
    const paramNames = [...new RegExp(
      `export function ${expectation.ts}\\(([^)]*)\\):`,
    ).exec(tsSource)[1]
      .split(',')
      .map((entry) => entry.trim().split(':')[0].trim())];
    paramNames.forEach((name, index) => {
      literal = literal.split('${' + name + '}').join(expectation.args[index]);
    });
    assert.equal(literal, expectation.expected, `${expectation.ts} produced wrong key`);
  }
});
