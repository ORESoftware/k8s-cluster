import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

const packageRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');

test('runtime-config schema opts generated Rust wire types into OpenAPI', async () => {
  const schema = JSON.parse(await readFile(path.join(packageRoot, 'schema', 'runtime-config.schema.json'), 'utf8'));
  const cargo = await readFile(path.join(packageRoot, 'generated', 'rust', 'Cargo.toml'), 'utf8');
  const rust = await readFile(path.join(packageRoot, 'generated', 'rust', 'src', 'lib.rs'), 'utf8');

  assert.equal(schema['x-rust-openapi'], true);
  assert.ok(cargo.includes('[features]'));
  assert.ok(cargo.includes('openapi = ["dep:utoipa"]'));
  assert.ok(cargo.includes('utoipa = { version = "=5.5.0", optional = true }'));

  const runtimeTypes = [
    ['enum', 'RuntimeConfigApplyReason'],
    ['struct', 'RuntimeConfigApplyRequest'],
    ['struct', 'RuntimeConfigApplyResponse'],
    ['struct', 'RuntimeConfigEntry'],
    ['enum', 'RuntimeConfigEnv'],
    ['struct', 'RuntimeConfigRegisterRequest'],
    ['struct', 'RuntimeConfigSnapshot'],
    ['struct', 'RuntimeConfigSubscriber'],
    ['struct', 'RuntimeConfigUpsertRequest'],
  ];
  const schemaAttribute = '#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]';
  for (const [kind, name] of runtimeTypes) {
    const marker = `pub ${kind} ${name} {`;
    const index = rust.indexOf(marker);
    assert.notEqual(index, -1, `missing generated Rust type ${name}`);
    const prefix = rust.slice(Math.max(0, index - 500), index);
    assert.ok(prefix.includes(schemaAttribute), `${name} must derive ToSchema only when openapi is enabled`);
  }

  const unrelatedIndex = rust.indexOf('pub struct AgentTaskQueueMessage {');
  assert.notEqual(unrelatedIndex, -1);
  const unrelatedPrefix = rust.slice(Math.max(0, unrelatedIndex - 500), unrelatedIndex);
  assert.equal(unrelatedPrefix.includes(schemaAttribute), false, 'unrelated schema families must not inherit Utoipa');
});
